//! Core data types for clips and LLM verdicts.

use chrono::{DateTime, Utc};
use std::path::PathBuf;

/// A video clip discovered in the input directory with all metadata read.
#[derive(Debug, Clone)]
pub(crate) struct Clip {
    pub(crate) path: PathBuf,
    pub(crate) meta: ClipMeta,
    /// Transcript extracted from the clip's audio via whisper.cpp.
    /// `None` means transcription was skipped (no model) or produced nothing.
    pub(crate) transcript: Option<String>,
}

/// Metadata extracted from a clip via `ffprobe` + timestamp resolution.
#[derive(Debug, Clone)]
pub(crate) struct ClipMeta {
    pub(crate) duration_s: f64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) timestamp: DateTime<Utc>,
    pub(crate) timestamp_source: TimestampSource,
}

/// Where the clip's timestamp came from. Recorded in the sidecar so
/// the user can debug weird orderings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TimestampSource {
    /// Read from the container's `creation_time` atom via ffprobe.
    CreationTime,
    /// Parsed from the filename (iPhone / Meta Ray-Ban / Pixel patterns).
    FilenamePattern,
    /// Fallback to the filesystem mtime.
    FileMtime,
}

/// The LLM's verdict on a single clip. Lives in memory and serializes
/// to `decisions.json`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ClipVerdict {
    pub(crate) path: PathBuf,
    pub(crate) duration_s: f64,
    pub(crate) timestamp: DateTime<Utc>,
    pub(crate) timestamp_source: TimestampSource,

    /// Score from 1 to 10, or `None` if analysis failed.
    pub(crate) score: Option<u8>,

    /// LLM's one-sentence reason for the score.
    pub(crate) reason: Option<String>,

    /// Error message if analysis failed for this clip.
    pub(crate) error: Option<String>,

    /// Whether to include this clip in the final vlog. Set during the
    /// filter stage (`build` / `analyze`), honored as-is by `render`.
    #[serde(default)]
    pub(crate) keep: bool,

    /// Transcript that was fed to the LLM during analysis, if any.
    /// Stored in the sidecar for user visibility and debugging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) transcript: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn sample_verdict() -> Result<ClipVerdict, Box<dyn std::error::Error>> {
        let ts = Utc
            .with_ymd_and_hms(2026, 4, 12, 14, 23, 45)
            .single()
            .ok_or("bad timestamp")?;
        Ok(ClipVerdict {
            path: PathBuf::from("IMG_2341.mp4"),
            duration_s: 12.3,
            timestamp: ts,
            timestamp_source: TimestampSource::CreationTime,
            score: Some(8),
            reason: Some("clear subject".to_string()),
            error: None,
            keep: true,
            transcript: None,
        })
    }

    #[test]
    fn timestamp_source_serializes_snake_case() -> TestResult {
        let json = serde_json::to_string(&TimestampSource::CreationTime)?;
        assert_eq!(json, "\"creation_time\"");
        let json = serde_json::to_string(&TimestampSource::FilenamePattern)?;
        assert_eq!(json, "\"filename_pattern\"");
        let json = serde_json::to_string(&TimestampSource::FileMtime)?;
        assert_eq!(json, "\"file_mtime\"");
        Ok(())
    }

    #[test]
    fn verdict_round_trips_through_json() -> TestResult {
        let v = sample_verdict()?;
        let json = serde_json::to_string(&v)?;
        let parsed: ClipVerdict = serde_json::from_str(&json)?;
        assert_eq!(parsed.path, v.path);
        assert_eq!(parsed.score, v.score);
        assert_eq!(parsed.reason, v.reason);
        assert_eq!(parsed.keep, v.keep);
        assert_eq!(parsed.timestamp_source, v.timestamp_source);
        Ok(())
    }

    #[test]
    fn verdict_default_keep_is_false() -> TestResult {
        let json = r#"{
            "path": "clip.mp4",
            "duration_s": 5.0,
            "timestamp": "2026-04-12T14:23:45Z",
            "timestamp_source": "creation_time",
            "score": 7,
            "reason": "ok",
            "error": null
        }"#;
        let v: ClipVerdict = serde_json::from_str(json)?;
        assert!(!v.keep);
        Ok(())
    }

    #[test]
    fn verdict_with_error_has_no_score() -> TestResult {
        let ts = Utc
            .with_ymd_and_hms(2026, 4, 12, 14, 30, 0)
            .single()
            .ok_or("bad timestamp")?;
        let v = ClipVerdict {
            path: PathBuf::from("bad.mp4"),
            duration_s: 3.0,
            timestamp: ts,
            timestamp_source: TimestampSource::FileMtime,
            score: None,
            reason: None,
            error: Some("timed out".to_string()),
            keep: false,
            transcript: None,
        };
        let json = serde_json::to_string(&v)?;
        let parsed: ClipVerdict = serde_json::from_str(&json)?;
        assert!(parsed.score.is_none());
        assert_eq!(parsed.error.as_deref(), Some("timed out"));
        Ok(())
    }
}
