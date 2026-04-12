//! Concatenate the kept clips into a single output video using `ffmpeg`.
//!
//! Steps:
//! 1. Filter to kept clips with `keep = true`.
//! 2. Verify each is 9:16 (portrait). Error on mismatch.
//! 3. Sort kept clips by timestamp ascending.
//! 4. Write a concat manifest file (one `file '<abs path>'` line per clip).
//! 5. Run `ffmpeg -f concat -safe 0 -i <manifest> -c copy <output>` first.
//!    If that fails, retry with a full re-encode.

use crate::clip::ClipVerdict;
use crate::process::{self, ProcessError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Errors from the concat stage.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConcatError {
    #[error(transparent)]
    Process(#[from] ProcessError),

    #[error(
        "clip {} is {w}x{h}, but v1 only supports 9:16 (portrait).\n\
         Either re-record in portrait mode, remove this clip from the input\n\
         directory, or wait for aspect normalization in v2.",
        path.display()
    )]
    ClipWrongAspect { path: PathBuf, w: u32, h: u32 },

    #[error("no kept clips to concat")]
    NothingToKeep,

    #[error("failed to write concat manifest: {0}")]
    ManifestWrite(#[source] std::io::Error),

    #[error("ffmpeg concat failed even with re-encode fallback: {source}")]
    ConcatFailed {
        #[source]
        source: ProcessError,
    },
}

/// Run the concat stage: aspect check → manifest → ffmpeg.
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
        if !is_nine_sixteen(w, h) {
            return Err(ConcatError::ClipWrongAspect {
                path: v.path.clone(),
                w,
                h,
            });
        }
    }

    kept.sort_by_key(|v| v.timestamp);

    let manifest_path = output_path.with_extension("concat-manifest.txt");
    let manifest_body = build_manifest(&kept);
    std::fs::write(&manifest_path, manifest_body).map_err(ConcatError::ManifestWrite)?;

    let manifest_str = manifest_path.to_string_lossy().into_owned();
    let out_str = output_path.to_string_lossy().into_owned();
    let copy_result = process::run(
        "ffmpeg",
        [
            "-nostdin",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            manifest_str.as_str(),
            "-c",
            "copy",
            "-movflags",
            "+faststart",
            out_str.as_str(),
        ],
        std::iter::empty::<(&str, &str)>(),
        None,
    )
    .await;

    if copy_result.is_ok() {
        let _ = std::fs::remove_file(&manifest_path);
        return Ok(());
    }

    let reencode_result = process::run(
        "ffmpeg",
        [
            "-nostdin",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            manifest_str.as_str(),
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-crf",
            "23",
            "-preset",
            "medium",
            "-movflags",
            "+faststart",
            out_str.as_str(),
        ],
        std::iter::empty::<(&str, &str)>(),
        None,
    )
    .await;

    let _ = std::fs::remove_file(&manifest_path);

    reencode_result.map_err(|source| ConcatError::ConcatFailed { source })?;
    Ok(())
}

fn is_nine_sixteen(w: u32, h: u32) -> bool {
    if w == 0 || h == 0 {
        return false;
    }
    let ratio = f64::from(w) / f64::from(h);
    (ratio - 0.5625).abs() < 0.01
}

fn build_manifest(kept: &[&ClipVerdict]) -> String {
    let mut out = String::new();
    for v in kept {
        let path = v.path.to_string_lossy();
        let escaped = path.replace('\'', r"'\''");
        out.push_str(&format!("file '{escaped}'\n"));
    }
    out
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
    fn nine_sixteen_detection() {
        assert!(is_nine_sixteen(1080, 1920));
        assert!(is_nine_sixteen(720, 1280));
        assert!(!is_nine_sixteen(1920, 1080));
        assert!(!is_nine_sixteen(1080, 1080));
        assert!(!is_nine_sixteen(0, 1920));
        assert!(!is_nine_sixteen(1080, 0));
    }

    #[test]
    fn build_manifest_shape() -> TestResult {
        let v1 = verdict("/tmp/a.mp4", 10.0, true)?;
        let v2 = verdict("/tmp/b.mp4", 10.0, true)?;
        let manifest = build_manifest(&[&v1, &v2]);
        assert!(manifest.contains("file '/tmp/a.mp4'"));
        assert!(manifest.contains("file '/tmp/b.mp4'"));
        Ok(())
    }

    #[test]
    fn build_manifest_escapes_single_quotes() -> TestResult {
        let v = verdict("/tmp/it's-a-clip.mp4", 10.0, true)?;
        let manifest = build_manifest(&[&v]);
        assert!(manifest.contains(r"'\''"));
        Ok(())
    }
}
