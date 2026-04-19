//! Concatenate a list of planned segments into a single output video+audio
//! using `ffmpeg`.
//!
//! Input clips from Meta Ray-Ban glasses (and most phones) are portrait
//! but often mixed resolutions across a single shoot. The concat filter
//! handles this by scaling + padding every clip to a common target
//! (1080x1920) before concatenation. Audio is resampled to a common
//! 48kHz stereo AAC stream.
//!
//! Each segment may trim its source clip via `start_s`/`end_s`; those
//! trims are materialized into a tempdir before concat.
//!
//! Steps:
//! 1. Validate the segment list is non-empty.
//! 2. Verify each segment's source is portrait (height > width).
//! 3. Ensure all sources share the same aspect ratio (width, height).
//! 4. For segments with trims, produce a short temp clip via `-ss` / `-to`.
//! 5. Build an ffmpeg `-filter_complex` graph that scales + pads each
//!    input to 1080x1920, normalizes audio, and concatenates.

use crate::plan::Segment;
use crate::process::{self, ProcessError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Target resolution for all clips in the output vlog.
const TARGET_W: u32 = 1080;
const TARGET_H: u32 = 1920;

/// Errors from the concat stage.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConcatError {
    #[error(transparent)]
    Process(#[from] ProcessError),

    #[error(
        "clip {} is {w}x{h} (landscape or square), but clipcast only supports portrait.\n\
         Re-record in portrait mode or remove this clip from the plan.",
        path.display()
    )]
    ClipNotPortrait { path: PathBuf, w: u32, h: u32 },

    #[error(
        "aspect-ratio mismatch: first segment {} is {first_dims:?}, \
         but segment {} is {offender_dims:?}. Drop one from the plan or re-encode.",
        first.display(), offender.display()
    )]
    AspectMismatch {
        first: PathBuf,
        first_dims: (u32, u32),
        offender: PathBuf,
        offender_dims: (u32, u32),
    },

    #[error("no segments to concat")]
    NoSegments,

    #[error("missing metadata for segment source {}", .0.display())]
    MissingMeta(PathBuf),

    #[error("ffmpeg trim failed for {}: {message}", source_path.display())]
    Trim {
        source_path: PathBuf,
        message: String,
    },

    #[error("tempdir creation failed: {0}")]
    TempDir(#[source] std::io::Error),

    #[error("ffmpeg concat failed: {source}")]
    ConcatFailed {
        #[source]
        source: ProcessError,
    },
}

/// Concat a list of planned segments to `output_path`.
///
/// Each segment may optionally specify in/out points within its source clip.
/// Segments are concatenated in `order` ascending (caller should pass them
/// in that order, but we re-sort defensively).
pub(crate) async fn run_segments(
    segments: &[Segment],
    metas_by_path: &HashMap<PathBuf, (u32, u32)>,
    output_path: &Path,
) -> Result<(), ConcatError> {
    if segments.is_empty() {
        return Err(ConcatError::NoSegments);
    }

    let mut ordered: Vec<&Segment> = segments.iter().collect();
    ordered.sort_by_key(|s| s.order);

    // Portrait + aspect-ratio check on each segment's source.
    let (w0, h0) = metas_by_path
        .get(&ordered[0].source)
        .copied()
        .ok_or_else(|| ConcatError::MissingMeta(ordered[0].source.clone()))?;
    if !is_portrait(w0, h0) {
        return Err(ConcatError::ClipNotPortrait {
            path: ordered[0].source.clone(),
            w: w0,
            h: h0,
        });
    }
    for s in ordered.iter().skip(1) {
        let (w, h) = metas_by_path
            .get(&s.source)
            .copied()
            .ok_or_else(|| ConcatError::MissingMeta(s.source.clone()))?;
        if !is_portrait(w, h) {
            return Err(ConcatError::ClipNotPortrait {
                path: s.source.clone(),
                w,
                h,
            });
        }
        if (w, h) != (w0, h0) {
            return Err(ConcatError::AspectMismatch {
                first: ordered[0].source.clone(),
                first_dims: (w0, h0),
                offender: s.source.clone(),
                offender_dims: (w, h),
            });
        }
    }

    // Materialize trimmed temp files where needed.
    let tempdir = tempfile::TempDir::new().map_err(ConcatError::TempDir)?;
    let mut paths_for_concat: Vec<PathBuf> = Vec::with_capacity(ordered.len());
    for (i, s) in ordered.iter().enumerate() {
        match (s.start_s, s.end_s) {
            (None, None) => paths_for_concat.push(s.source.clone()),
            (start, end) => {
                let trimmed = tempdir.path().join(format!("seg-{i:04}.mp4"));
                trim_source(&s.source, start, end, &trimmed).await?;
                paths_for_concat.push(trimmed);
            }
        }
    }

    let args = build_ffmpeg_args(&paths_for_concat, output_path);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    process::run("ffmpeg", arg_refs, std::iter::empty::<(&str, &str)>(), None)
        .await
        .map_err(|source| ConcatError::ConcatFailed { source })?;

    Ok(())
}

/// Build the ffmpeg command line that would be used for a concat pass.
/// Exposed for `--dry-run` callers that want to show what would run.
pub(crate) fn dry_run_commands(segments: &[Segment], output_path: &Path) -> Vec<Vec<String>> {
    let mut ordered: Vec<&Segment> = segments.iter().collect();
    ordered.sort_by_key(|s| s.order);
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut concat_paths: Vec<PathBuf> = Vec::with_capacity(ordered.len());
    for (i, s) in ordered.iter().enumerate() {
        match (s.start_s, s.end_s) {
            (None, None) => concat_paths.push(s.source.clone()),
            (start, end) => {
                let trimmed = PathBuf::from(format!("<tempdir>/seg-{i:04}.mp4"));
                out.push(trim_args(&s.source, start, end, &trimmed));
                concat_paths.push(trimmed);
            }
        }
    }
    out.push(build_ffmpeg_args(&concat_paths, output_path));
    out
}

fn is_portrait(w: u32, h: u32) -> bool {
    w > 0 && h > 0 && h > w
}

async fn trim_source(
    src: &Path,
    start: Option<f64>,
    end: Option<f64>,
    out: &Path,
) -> Result<(), ConcatError> {
    let args = trim_args(src, start, end, out);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    process::run("ffmpeg", arg_refs, std::iter::empty::<(&str, &str)>(), None)
        .await
        .map_err(|e| ConcatError::Trim {
            source_path: src.to_path_buf(),
            message: e.to_string(),
        })?;
    Ok(())
}

fn trim_args(src: &Path, start: Option<f64>, end: Option<f64>, out: &Path) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-nostdin".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
    ];
    if let Some(ss) = start {
        args.push("-ss".to_string());
        args.push(format!("{ss}"));
    }
    args.push("-i".to_string());
    args.push(src.to_string_lossy().into_owned());
    if let Some(ee) = end {
        let t = ee - start.unwrap_or(0.0);
        args.push("-t".to_string());
        args.push(format!("{t}"));
    }
    args.push("-c".to_string());
    args.push("copy".to_string());
    args.push(out.to_string_lossy().into_owned());
    args
}

fn build_ffmpeg_args(inputs: &[PathBuf], output_path: &Path) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-nostdin".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
    ];

    for path in inputs {
        args.push("-i".to_string());
        args.push(path.to_string_lossy().into_owned());
    }

    args.push("-filter_complex".to_string());
    args.push(build_filter_graph(inputs.len()));

    args.push("-map".to_string());
    args.push("[outv]".to_string());
    args.push("-map".to_string());
    args.push("[outa]".to_string());
    args.push("-c:v".to_string());
    args.push("libx264".to_string());
    args.push("-preset".to_string());
    args.push("medium".to_string());
    args.push("-crf".to_string());
    args.push("23".to_string());
    args.push("-c:a".to_string());
    args.push("aac".to_string());
    args.push("-b:a".to_string());
    args.push("192k".to_string());
    args.push("-movflags".to_string());
    args.push("+faststart".to_string());
    args.push(output_path.to_string_lossy().into_owned());

    args
}

fn build_filter_graph(n: usize) -> String {
    let mut filter = String::new();
    for i in 0..n {
        filter.push_str(&format!(
            "[{i}:v]scale={TARGET_W}:{TARGET_H}:force_original_aspect_ratio=decrease,\
             pad={TARGET_W}:{TARGET_H}:(ow-iw)/2:(oh-ih)/2,setsar=1[v{i}];"
        ));
        filter.push_str(&format!(
            "[{i}:a]aresample=48000,aformat=channel_layouts=stereo[a{i}];"
        ));
    }
    for i in 0..n {
        filter.push_str(&format!("[v{i}][a{i}]"));
    }
    filter.push_str(&format!("concat=n={n}:v=1:a=1[outv][outa]"));
    filter
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn seg(name: &str, order: u32, start: Option<f64>, end: Option<f64>) -> Segment {
        Segment {
            order,
            source: PathBuf::from(name),
            start_s: start,
            end_s: end,
            duration_s: 1.0,
            title: String::new(),
            rationale: String::new(),
            trim_reason: None,
        }
    }

    #[test]
    fn portrait_detection() {
        assert!(is_portrait(1080, 1920));
        assert!(is_portrait(720, 1280));
        assert!(is_portrait(1376, 1824));
        assert!(!is_portrait(1920, 1080));
        assert!(!is_portrait(1080, 1080));
        assert!(!is_portrait(0, 1920));
        assert!(!is_portrait(1080, 0));
    }

    #[test]
    fn filter_graph_shape() {
        let graph = build_filter_graph(3);
        assert!(graph.contains("[0:v]scale=1080:1920"));
        assert!(graph.contains("[1:v]scale=1080:1920"));
        assert!(graph.contains("[2:v]scale=1080:1920"));
        assert!(graph.contains("[0:a]aresample=48000"));
        assert!(graph.contains("[v0][a0][v1][a1][v2][a2]concat=n=3:v=1:a=1[outv][outa]"));
    }

    #[test]
    fn ffmpeg_args_include_inputs_and_output() {
        let inputs = vec![PathBuf::from("/tmp/a.mov"), PathBuf::from("/tmp/b.mov")];
        let args = build_ffmpeg_args(&inputs, Path::new("/tmp/out.mp4"));
        assert!(args.contains(&"/tmp/a.mov".to_string()));
        assert!(args.contains(&"/tmp/b.mov".to_string()));
        assert!(args.contains(&"/tmp/out.mp4".to_string()));
        assert!(args.iter().any(|a| a == "-filter_complex"));
    }

    #[test]
    fn trim_args_omits_flags_when_none() {
        let args = trim_args(
            Path::new("/tmp/a.mov"),
            None,
            None,
            Path::new("/tmp/trim.mp4"),
        );
        assert!(!args.iter().any(|a| a == "-ss"));
        assert!(!args.iter().any(|a| a == "-t"));
    }

    #[test]
    fn trim_args_emits_ss_and_t_when_both_set() -> TestResult {
        let args = trim_args(
            Path::new("/tmp/a.mov"),
            Some(2.0),
            Some(5.5),
            Path::new("/tmp/trim.mp4"),
        );
        let ss_idx = args.iter().position(|a| a == "-ss").ok_or("missing -ss")?;
        assert_eq!(args[ss_idx + 1], "2");
        let t_idx = args.iter().position(|a| a == "-t").ok_or("missing -t")?;
        assert_eq!(args[t_idx + 1], "3.5");
        Ok(())
    }

    #[tokio::test]
    async fn run_segments_errors_when_empty() -> TestResult {
        let metas = HashMap::new();
        let dir = tempfile::TempDir::new()?;
        let out = dir.path().join("out.mp4");
        let result = run_segments(&[], &metas, &out).await;
        if !matches!(result, Err(ConcatError::NoSegments)) {
            return Err(format!("wrong result: {result:?}").into());
        }
        Ok(())
    }

    #[tokio::test]
    async fn run_segments_detects_aspect_mismatch() -> TestResult {
        let segments = vec![seg("a.mp4", 1, None, None), seg("b.mp4", 2, None, None)];
        let mut metas = HashMap::new();
        metas.insert(PathBuf::from("a.mp4"), (1080u32, 1920u32));
        metas.insert(PathBuf::from("b.mp4"), (720u32, 1280u32));
        let dir = tempfile::TempDir::new()?;
        let result = run_segments(&segments, &metas, &dir.path().join("out.mp4")).await;
        if !matches!(result, Err(ConcatError::AspectMismatch { .. })) {
            return Err(format!("wrong result: {result:?}").into());
        }
        Ok(())
    }

    #[tokio::test]
    async fn run_segments_rejects_non_portrait() -> TestResult {
        let segments = vec![seg("a.mp4", 1, None, None)];
        let mut metas = HashMap::new();
        metas.insert(PathBuf::from("a.mp4"), (1920u32, 1080u32));
        let dir = tempfile::TempDir::new()?;
        let result = run_segments(&segments, &metas, &dir.path().join("out.mp4")).await;
        if !matches!(result, Err(ConcatError::ClipNotPortrait { .. })) {
            return Err(format!("wrong result: {result:?}").into());
        }
        Ok(())
    }

    #[test]
    fn dry_run_commands_emits_trim_then_concat() {
        let segments = vec![
            seg("a.mp4", 1, None, None),
            seg("b.mp4", 2, Some(1.0), Some(4.0)),
        ];
        let cmds = dry_run_commands(&segments, Path::new("/tmp/out.mp4"));
        // one trim (for b) + one concat = 2 commands
        assert_eq!(cmds.len(), 2);
        assert!(cmds[0].iter().any(|a| a == "-ss"));
        assert!(cmds[1].iter().any(|a| a == "-filter_complex"));
    }
}
