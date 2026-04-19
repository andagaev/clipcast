//! Read and write `decisions.json` — the sidecar file that carries LLM
//! verdicts between the `analyze` and `render` stages.

use crate::clip::ClipVerdict;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

/// The current `decisions.json` schema version that this build reads and writes.
pub(crate) const DECISIONS_SCHEMA_VERSION: u32 = 2;

fn default_schema_version() -> u32 {
    DECISIONS_SCHEMA_VERSION
}

/// The top-level structure of a `decisions.json` file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Sidecar {
    #[serde(default = "default_schema_version")]
    pub(crate) schema_version: u32,
    pub(crate) clipcast_version: String,
    pub(crate) generated_at: DateTime<Utc>,
    pub(crate) target_duration_s: u64,
    pub(crate) clips: Vec<ClipVerdict>,
}

/// Errors from sidecar I/O.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SidecarError {
    #[error("failed to read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse decisions.json at {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("failed to write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to serialize sidecar: {0}")]
    Serialize(#[source] serde_json::Error),

    #[error(
        "unsupported decisions schema_version {found}; this build expects {expected}. \
         Run `clipcast schema decisions` for the current shape."
    )]
    UnsupportedVersion { found: u32, expected: u32 },
}

/// Read a sidecar file from disk, enforcing the current schema version.
pub(crate) async fn read(path: &Path) -> Result<Sidecar, SidecarError> {
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|source| SidecarError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    let sidecar: Sidecar = serde_json::from_str(&text).map_err(|source| SidecarError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    if sidecar.schema_version != DECISIONS_SCHEMA_VERSION {
        return Err(SidecarError::UnsupportedVersion {
            found: sidecar.schema_version,
            expected: DECISIONS_SCHEMA_VERSION,
        });
    }
    Ok(sidecar)
}

/// Write a sidecar file to disk (pretty-printed).
pub(crate) async fn write(path: &Path, sidecar: &Sidecar) -> Result<(), SidecarError> {
    let json = serde_json::to_string_pretty(sidecar).map_err(SidecarError::Serialize)?;
    tokio::fs::write(path, json)
        .await
        .map_err(|source| SidecarError::Write {
            path: path.to_path_buf(),
            source,
        })
}

/// Build a fresh sidecar for a newly-run analyze pass.
pub(crate) fn build(target_duration_s: u64, clips: Vec<ClipVerdict>) -> Sidecar {
    Sidecar {
        schema_version: DECISIONS_SCHEMA_VERSION,
        clipcast_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: Utc::now(),
        target_duration_s,
        clips,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::TimestampSource;
    use chrono::TimeZone;
    use tempfile::TempDir;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn sample_clips() -> Result<Vec<ClipVerdict>, Box<dyn std::error::Error>> {
        let t1 = Utc
            .with_ymd_and_hms(2026, 4, 12, 14, 0, 0)
            .single()
            .ok_or("bad timestamp")?;
        let t2 = Utc
            .with_ymd_and_hms(2026, 4, 12, 14, 5, 0)
            .single()
            .ok_or("bad timestamp")?;
        Ok(vec![
            ClipVerdict {
                path: PathBuf::from("a.mp4"),
                duration_s: 10.0,
                timestamp: t1,
                timestamp_source: TimestampSource::CreationTime,
                score: Some(8),
                reason: Some("good".to_string()),
                error: None,
                transcript: None,
            },
            ClipVerdict {
                path: PathBuf::from("b.mp4"),
                duration_s: 5.0,
                timestamp: t2,
                timestamp_source: TimestampSource::FilenamePattern,
                score: Some(3),
                reason: Some("boring".to_string()),
                error: None,
                transcript: None,
            },
        ])
    }

    #[tokio::test]
    async fn write_then_read_round_trips() -> TestResult {
        let dir = TempDir::new()?;
        let path = dir.path().join("test.decisions.json");
        let original = build(180, sample_clips()?);
        write(&path, &original).await?;
        let read_back = read(&path).await?;
        assert_eq!(read_back.target_duration_s, original.target_duration_s);
        assert_eq!(read_back.schema_version, DECISIONS_SCHEMA_VERSION);
        assert_eq!(read_back.clips.len(), 2);
        assert_eq!(read_back.clips[0].score, Some(8));
        assert_eq!(read_back.clips[1].score, Some(3));
        Ok(())
    }

    #[tokio::test]
    async fn read_rejects_wrong_schema_version() -> TestResult {
        let dir = TempDir::new()?;
        let path = dir.path().join("old.decisions.json");
        let bogus = serde_json::json!({
            "schema_version": 1,
            "clipcast_version": "0.0.0",
            "generated_at": "2026-04-12T14:00:00Z",
            "target_duration_s": 60,
            "clips": [],
        });
        tokio::fs::write(&path, serde_json::to_string(&bogus)?).await?;
        let err = read(&path).await.err().ok_or("expected error")?;
        if !matches!(err, SidecarError::UnsupportedVersion { .. }) {
            return Err(format!("wrong variant: {err:?}").into());
        }
        Ok(())
    }

    #[tokio::test]
    async fn read_fails_on_missing_file() -> TestResult {
        let err = read(Path::new("/definitely/not/a/real/path.json"))
            .await
            .err()
            .ok_or("expected error")?;
        if !matches!(err, SidecarError::Read { .. }) {
            return Err(format!("wrong variant: {err:?}").into());
        }
        Ok(())
    }

    #[tokio::test]
    async fn read_fails_on_bad_json() -> TestResult {
        let dir = TempDir::new()?;
        let path = dir.path().join("bad.json");
        tokio::fs::write(&path, b"not valid json at all").await?;
        let err = read(&path).await.err().ok_or("expected error")?;
        if !matches!(err, SidecarError::Parse { .. }) {
            return Err(format!("wrong variant: {err:?}").into());
        }
        Ok(())
    }

    #[tokio::test]
    async fn written_file_is_pretty_printed() -> TestResult {
        let dir = TempDir::new()?;
        let path = dir.path().join("test.decisions.json");
        let sidecar = build(180, sample_clips()?);
        write(&path, &sidecar).await?;
        let text = tokio::fs::read_to_string(&path).await?;
        assert!(text.contains('\n'));
        assert!(text.contains("  "));
        Ok(())
    }
}
