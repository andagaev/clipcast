//! Concatenate the kept clips into a single output video+audio using `ffmpeg`.
//!
//! Input clips from Meta Ray-Ban glasses (and most phones) are portrait
//! but often mixed resolutions across a single shoot. The concat filter
//! handles this by scaling + padding every clip to a common target
//! (1080x1920) before concatenation. Audio is resampled to a common
//! 48kHz stereo AAC stream.
//!
//! Steps:
//! 1. Filter to kept clips with `keep = true`.
//! 2. Verify each is portrait (height > width). Landscape clips error out.
//! 3. Sort kept clips by timestamp ascending.
//! 4. Build an ffmpeg `-filter_complex` graph that scales + pads each
//!    clip's video to 1080x1920, normalizes audio, and concatenates.

use crate::clip::ClipVerdict;
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
         Re-record in portrait mode or remove this clip from the input directory.",
        path.display()
    )]
    ClipNotPortrait { path: PathBuf, w: u32, h: u32 },

    #[error("no kept clips to concat")]
    NothingToKeep,

    #[error("ffmpeg concat failed: {source}")]
    ConcatFailed {
        #[source]
        source: ProcessError,
    },
}

/// Run the concat stage: portrait check → ffmpeg concat filter.
pub(crate) async fn run(
    verdicts: &[ClipVerdict],
    metas_by_path: &HashMap<PathBuf, (u32, u32)>,
    output_path: &Path,
) -> Result<(), ConcatError> {
    let mut kept: Vec<&ClipVerdict> = verdicts.iter().filter(|v| v.keep).collect();
    if kept.is_empty() {
        return Err(ConcatError::NothingToKeep);
    }

    for v in &kept {
        let (w, h) = metas_by_path.get(&v.path).copied().unwrap_or((0, 0));
        if !is_portrait(w, h) {
            return Err(ConcatError::ClipNotPortrait {
                path: v.path.clone(),
                w,
                h,
            });
        }
    }

    kept.sort_by_key(|v| v.timestamp);

    let args = build_ffmpeg_args(&kept, output_path);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    process::run("ffmpeg", arg_refs, std::iter::empty::<(&str, &str)>(), None)
        .await
        .map_err(|source| ConcatError::ConcatFailed { source })?;

    Ok(())
}

fn is_portrait(w: u32, h: u32) -> bool {
    w > 0 && h > 0 && h > w
}

fn build_ffmpeg_args(kept: &[&ClipVerdict], output_path: &Path) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-nostdin".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
    ];

    for v in kept {
        args.push("-i".to_string());
        args.push(v.path.to_string_lossy().into_owned());
    }

    args.push("-filter_complex".to_string());
    args.push(build_filter_graph(kept.len()));

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
    use crate::clip::TimestampSource;
    use chrono::{TimeZone, Utc};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn verdict(
        name: &str,
        duration: f64,
        keep: bool,
    ) -> Result<ClipVerdict, Box<dyn std::error::Error>> {
        let ts = Utc
            .with_ymd_and_hms(2026, 4, 12, 14, 0, 0)
            .single()
            .ok_or("bad timestamp")?;
        Ok(ClipVerdict {
            path: PathBuf::from(name),
            duration_s: duration,
            timestamp: ts,
            timestamp_source: TimestampSource::CreationTime,
            score: Some(5),
            reason: None,
            error: None,
            keep,
        })
    }

    #[test]
    fn portrait_detection() {
        assert!(is_portrait(1080, 1920));
        assert!(is_portrait(720, 1280));
        assert!(is_portrait(1376, 1824));
        assert!(is_portrait(1552, 2064));
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
    fn ffmpeg_args_include_inputs_and_output() -> TestResult {
        let v1 = verdict("/tmp/a.mov", 10.0, true)?;
        let v2 = verdict("/tmp/b.mov", 10.0, true)?;
        let args = build_ffmpeg_args(&[&v1, &v2], Path::new("/tmp/out.mp4"));
        assert!(args.contains(&"/tmp/a.mov".to_string()));
        assert!(args.contains(&"/tmp/b.mov".to_string()));
        assert!(args.contains(&"/tmp/out.mp4".to_string()));
        assert!(args.iter().any(|a| a == "-filter_complex"));
        Ok(())
    }
}
