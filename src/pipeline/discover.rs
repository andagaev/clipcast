//! Discover clips in an input directory and read their metadata via `ffprobe`.
//!
//! For each `.mp4` / `.mov` file found, runs `ffprobe -v quiet -print_format json
//! -show_format -show_streams <path>` and parses:
//! - `format.duration` → duration in seconds
//! - `streams[video].width` / `height` → resolution
//! - `format.tags.creation_time` → container timestamp (if present)
//!
//! Timestamp resolution is three-tier:
//! 1. `creation_time` from ffprobe
//! 2. Filename pattern (`IMG_YYYY-MM-DD_HH-MM-SS`, `IMG_YYYYMMDD_HHMMSS`, etc.)
//! 3. File mtime fallback

use crate::clip::{Clip, ClipMeta, TimestampSource};
use crate::process::{self, ProcessError};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

/// Errors from the discover stage.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DiscoverError {
    #[error("ffprobe failed on {}: {source}", path.display())]
    FfprobeFailed {
        path: PathBuf,
        #[source]
        source: ProcessError,
    },

    #[error("ffprobe returned unparseable JSON for {}: {details}", path.display())]
    FfprobeParseFailed { path: PathBuf, details: String },

    #[error("clip {} has no video stream", path.display())]
    NoVideoStream { path: PathBuf },

    #[error("failed to read directory {}: {source}", path.display())]
    ReadDirFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to read file metadata for {}: {source}", path.display())]
    MetadataFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Walk `dir` (optionally recursively), find `.mp4` / `.mov` files, run ffprobe,
/// and build a `Vec<Clip>` sorted by resolved timestamp ascending.
pub(crate) async fn run(dir: &Path, recursive: bool) -> Result<Vec<Clip>, DiscoverError> {
    let paths = collect_clip_paths(dir, recursive)?;
    let mut clips = Vec::with_capacity(paths.len());
    for path in paths {
        let clip = describe(&path).await?;
        clips.push(clip);
    }
    clips.sort_by_key(|c| c.meta.timestamp);
    Ok(clips)
}

fn collect_clip_paths(dir: &Path, recursive: bool) -> Result<Vec<PathBuf>, DiscoverError> {
    let mut out = Vec::new();
    collect_clips_into(dir, recursive, &mut out)?;
    Ok(out)
}

fn collect_clips_into(
    dir: &Path,
    recursive: bool,
    out: &mut Vec<PathBuf>,
) -> Result<(), DiscoverError> {
    let entries = std::fs::read_dir(dir).map_err(|source| DiscoverError::ReadDirFailed {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| DiscoverError::ReadDirFailed {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_file() && is_video_clip(&path) {
            out.push(path);
        } else if recursive && path.is_dir() {
            collect_clips_into(&path, true, out)?;
        }
    }
    Ok(())
}

/// Accepted input extensions: `.mp4` and `.mov` (case-insensitive).
fn is_video_clip(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mp4") || e.eq_ignore_ascii_case("mov"))
}

async fn describe(path: &Path) -> Result<Clip, DiscoverError> {
    let path_str = path.to_string_lossy().into_owned();
    let output = process::run(
        "ffprobe",
        [
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            path_str.as_str(),
        ],
        std::iter::empty::<(&str, &str)>(),
        None,
    )
    .await
    .map_err(|source| DiscoverError::FfprobeFailed {
        path: path.to_path_buf(),
        source,
    })?;

    let parsed: FfprobeOutput =
        serde_json::from_slice(&output.stdout).map_err(|e| DiscoverError::FfprobeParseFailed {
            path: path.to_path_buf(),
            details: e.to_string(),
        })?;

    let video = parsed
        .streams
        .iter()
        .find(|s| s.codec_type == "video")
        .ok_or_else(|| DiscoverError::NoVideoStream {
            path: path.to_path_buf(),
        })?;

    let duration_s: f64 = parsed
        .format
        .duration
        .as_deref()
        .and_then(|d| d.parse().ok())
        .unwrap_or(0.0);

    let (timestamp, timestamp_source) = resolve_timestamp(path, parsed.format.tags.as_ref())?;

    Ok(Clip {
        path: path.to_path_buf(),
        meta: ClipMeta {
            duration_s,
            width: video.width,
            height: video.height,
            timestamp,
            timestamp_source,
        },
    })
}

fn resolve_timestamp(
    path: &Path,
    tags: Option<&FfprobeTags>,
) -> Result<(DateTime<Utc>, TimestampSource), DiscoverError> {
    if let Some(tags) = tags {
        if let Some(raw) = tags.creation_time.as_deref() {
            if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
                return Ok((dt.with_timezone(&Utc), TimestampSource::CreationTime));
            }
        }
    }

    if let Some(dt) = parse_filename_timestamp(path) {
        return Ok((dt, TimestampSource::FilenamePattern));
    }

    let metadata = std::fs::metadata(path).map_err(|source| DiscoverError::MetadataFailed {
        path: path.to_path_buf(),
        source,
    })?;
    let mtime = metadata
        .modified()
        .map_err(|source| DiscoverError::MetadataFailed {
            path: path.to_path_buf(),
            source,
        })?;
    let dt: DateTime<Utc> = mtime.into();
    Ok((dt, TimestampSource::FileMtime))
}

/// Try to parse a timestamp out of common filename patterns.
///
/// Supported:
/// - `IMG_YYYY-MM-DD_HH-MM-SS.mp4` (Meta Ray-Ban export)
/// - `IMG_YYYYMMDD_HHMMSS.mp4` (iOS-ish)
/// - `PXL_YYYYMMDD_HHMMSSsss.mp4` (Pixel)
/// - `VID_YYYYMMDD_HHMMSS.mp4` (Android)
fn parse_filename_timestamp(path: &Path) -> Option<DateTime<Utc>> {
    let stem = path.file_stem()?.to_str()?;

    let remainder = stem
        .strip_prefix("IMG_")
        .or_else(|| stem.strip_prefix("PXL_"))
        .or_else(|| stem.strip_prefix("VID_"))
        .or_else(|| stem.strip_prefix("CLIP_"))
        .unwrap_or(stem);

    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(remainder, "%Y-%m-%d_%H-%M-%S") {
        return Some(naive.and_utc());
    }

    let digits: String = remainder
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '_')
        .collect();
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(&digits, "%Y%m%d_%H%M%S") {
        return Some(naive.and_utc());
    }

    None
}

#[derive(Debug, serde::Deserialize)]
struct FfprobeOutput {
    format: FfprobeFormat,
    streams: Vec<FfprobeStream>,
}

#[derive(Debug, serde::Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
    tags: Option<FfprobeTags>,
}

#[derive(Debug, serde::Deserialize)]
struct FfprobeTags {
    creation_time: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct FfprobeStream {
    codec_type: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn utc_dt(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Option<DateTime<Utc>> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s).single()
    }

    #[test]
    fn parse_filename_meta_ray_ban() -> TestResult {
        let dt = parse_filename_timestamp(Path::new("IMG_2026-04-12_14-23-45.mp4"))
            .ok_or("expected parse")?;
        assert_eq!(dt, utc_dt(2026, 4, 12, 14, 23, 45).ok_or("bad")?);
        Ok(())
    }

    #[test]
    fn parse_filename_ios_style() -> TestResult {
        let dt = parse_filename_timestamp(Path::new("IMG_20260412_142345.mp4"))
            .ok_or("expected parse")?;
        assert_eq!(dt, utc_dt(2026, 4, 12, 14, 23, 45).ok_or("bad")?);
        Ok(())
    }

    #[test]
    fn parse_filename_pixel_style() -> TestResult {
        let dt = parse_filename_timestamp(Path::new("PXL_20260412_142345.mp4"))
            .ok_or("expected parse")?;
        assert_eq!(dt, utc_dt(2026, 4, 12, 14, 23, 45).ok_or("bad")?);
        Ok(())
    }

    #[test]
    fn parse_filename_android_vid() -> TestResult {
        let dt = parse_filename_timestamp(Path::new("VID_20260412_142345.mp4"))
            .ok_or("expected parse")?;
        assert_eq!(dt, utc_dt(2026, 4, 12, 14, 23, 45).ok_or("bad")?);
        Ok(())
    }

    #[test]
    fn parse_filename_returns_none_for_garbage() {
        assert!(parse_filename_timestamp(Path::new("random-name.mp4")).is_none());
        assert!(parse_filename_timestamp(Path::new("my-vlog.mp4")).is_none());
    }
}
