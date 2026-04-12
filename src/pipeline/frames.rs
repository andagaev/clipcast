//! Extract 5 frames per clip using `ffmpeg`, downscaled to 512px wide.
//!
//! Frames are written to a subdirectory of a shared `TempDir` named
//! after the clip's stem, as JPEGs named `frame_0.jpg` ... `frame_4.jpg`.
//! The `TempDir` is returned alongside the frame paths and must be
//! kept alive until the analyze stage finishes — dropping it deletes
//! all frames.

use crate::clip::Clip;
use crate::process::{self, ProcessError};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Number of frames extracted per clip.
pub(crate) const FRAMES_PER_CLIP: usize = 5;

/// Max width for extracted frames (preserving aspect).
pub(crate) const FRAME_MAX_WIDTH: u32 = 512;

/// Frames extracted for a single clip.
#[derive(Debug)]
pub(crate) struct ClipFrames {
    pub(crate) clip: Clip,
    /// Absolute paths to the 5 JPEG frames.
    pub(crate) frame_paths: Vec<PathBuf>,
}

/// Errors from the frame extraction stage.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FramesError {
    #[error(transparent)]
    Process(#[from] ProcessError),

    #[error(
        "ffmpeg could not extract frame at {timestamp_s:.2}s from {}",
        path.display()
    )]
    ExtractionFailed { path: PathBuf, timestamp_s: f64 },

    #[error("failed to create tempdir: {0}")]
    Tempdir(#[source] std::io::Error),

    #[error("failed to create clip subdir in tempdir: {0}")]
    ClipDir(#[source] std::io::Error),
}

/// Create a shared tempdir and extract frames for every clip sequentially.
///
/// Returns the tempdir (kept alive) and a `Vec<ClipFrames>` with frame paths.
pub(crate) async fn run(clips: Vec<Clip>) -> Result<(TempDir, Vec<ClipFrames>), FramesError> {
    let tempdir = TempDir::new().map_err(FramesError::Tempdir)?;
    let mut out = Vec::with_capacity(clips.len());

    for (idx, clip) in clips.into_iter().enumerate() {
        let clip_dir = tempdir.path().join(format!("clip_{idx:04}"));
        std::fs::create_dir(&clip_dir).map_err(FramesError::ClipDir)?;

        let frame_paths = extract_frames(&clip, &clip_dir).await?;
        out.push(ClipFrames { clip, frame_paths });
    }

    Ok((tempdir, out))
}

async fn extract_frames(clip: &Clip, dir: &Path) -> Result<Vec<PathBuf>, FramesError> {
    let duration = clip.meta.duration_s.max(0.1);
    let timestamps = compute_timestamps(duration);
    let mut frame_paths = Vec::with_capacity(FRAMES_PER_CLIP);

    for (i, t) in timestamps.iter().enumerate() {
        let out_path = dir.join(format!("frame_{i}.jpg"));
        extract_one(&clip.path, *t, &out_path).await?;
        if !out_path.exists() {
            return Err(FramesError::ExtractionFailed {
                path: clip.path.clone(),
                timestamp_s: *t,
            });
        }
        frame_paths.push(out_path);
    }
    Ok(frame_paths)
}

/// Compute 5 timestamps at 0%, 25%, 50%, 75%, and just-before-the-end.
fn compute_timestamps(duration_s: f64) -> Vec<f64> {
    let epsilon = 0.1;
    let end = (duration_s - epsilon).max(0.0);
    let mut out = Vec::with_capacity(FRAMES_PER_CLIP);
    for i in 0..FRAMES_PER_CLIP {
        let frac = i as f64 / (FRAMES_PER_CLIP - 1) as f64;
        out.push((frac * end).min(end));
    }
    out
}

async fn extract_one(
    clip_path: &Path,
    timestamp_s: f64,
    out_path: &Path,
) -> Result<(), FramesError> {
    let clip_str = clip_path.to_string_lossy().into_owned();
    let out_str = out_path.to_string_lossy().into_owned();
    let ts_str = format!("{timestamp_s:.3}");
    let scale_str = format!("scale={FRAME_MAX_WIDTH}:-1");

    process::run(
        "ffmpeg",
        [
            "-nostdin",
            "-loglevel",
            "error",
            "-ss",
            ts_str.as_str(),
            "-i",
            clip_str.as_str(),
            "-frames:v",
            "1",
            "-update",
            "1",
            "-vf",
            scale_str.as_str(),
            "-q:v",
            "5",
            "-y",
            out_str.as_str(),
        ],
        std::iter::empty::<(&str, &str)>(),
        None,
    )
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_timestamps_for_10s_clip() {
        let ts = compute_timestamps(10.0);
        assert_eq!(ts.len(), 5);
        assert!((ts[0] - 0.0).abs() < 0.001);
        assert!(ts[4] < 10.0);
        assert!(ts[4] > ts[3]);
        assert!(ts[3] > ts[2]);
        assert!(ts[2] > ts[1]);
    }

    #[test]
    fn compute_timestamps_for_very_short_clip() {
        let ts = compute_timestamps(0.5);
        assert_eq!(ts.len(), 5);
        for t in &ts {
            assert!(*t >= 0.0);
            assert!(*t <= 0.5);
        }
    }

    #[test]
    fn compute_timestamps_never_panics_on_zero() {
        let ts = compute_timestamps(0.0);
        assert_eq!(ts.len(), 5);
        for t in &ts {
            assert!(*t >= 0.0);
        }
    }
}
