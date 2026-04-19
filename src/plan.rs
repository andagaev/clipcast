//! `plan.json` — the agent-produced cut assembly plan that lives between
//! `clipcast plan` (writer) and `clipcast render` (reader).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The current schema version that this build of clipcast writes and reads.
pub(crate) const PLAN_SCHEMA_VERSION: u32 = 1;

/// A complete cut plan: what segments to include, in what order, at what trims.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Plan {
    pub(crate) schema_version: u32,
    pub(crate) clipcast_version: String,
    pub(crate) generated_at: DateTime<Utc>,
    pub(crate) model: String,
    pub(crate) decisions_ref: DecisionsRef,
    pub(crate) brief: String,
    pub(crate) target_duration_s: u64,
    pub(crate) estimated_duration_s: f64,
    pub(crate) segments: Vec<Segment>,
    #[serde(default)]
    pub(crate) rejected: Vec<RejectedClip>,
    #[serde(default)]
    pub(crate) warnings: Vec<String>,
}

/// Reference back to the `decisions.json` this plan was derived from,
/// so `render` can detect staleness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DecisionsRef {
    pub(crate) path: PathBuf,
    pub(crate) generated_at: DateTime<Utc>,
}

/// One segment of the final cut. `start_s` and `end_s` are `null` to
/// mean "use the whole clip"; otherwise both are in seconds within the
/// source clip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Segment {
    pub(crate) order: u32,
    pub(crate) source: PathBuf,
    /// Trim start in seconds within the source clip. `null` = use whole clip.
    pub(crate) start_s: Option<f64>,
    /// Trim end in seconds within the source clip. `null` = use whole clip.
    pub(crate) end_s: Option<f64>,
    /// Computed duration of the segment as it will appear in the cut.
    pub(crate) duration_s: f64,
    pub(crate) title: String,
    pub(crate) rationale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) trim_reason: Option<String>,
}

/// A clip the planner chose NOT to include, with a human-readable reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RejectedClip {
    pub(crate) source: PathBuf,
    pub(crate) score: u8,
    pub(crate) rejected_reason: String,
}

/// Errors from `plan.json` I/O.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PlanError {
    #[error("failed to read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse plan.json at {}: {source}", path.display())]
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
    #[error("failed to serialize plan: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error(
        "unsupported plan schema_version {found}; this build expects {expected}. \
         Run `clipcast schema plan` for the current shape."
    )]
    UnsupportedVersion { found: u32, expected: u32 },
}

/// Read a plan file, enforcing the current schema version.
pub(crate) async fn load(path: &Path) -> Result<Plan, PlanError> {
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|source| PlanError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    let plan: Plan = serde_json::from_str(&text).map_err(|source| PlanError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    if plan.schema_version != PLAN_SCHEMA_VERSION {
        return Err(PlanError::UnsupportedVersion {
            found: plan.schema_version,
            expected: PLAN_SCHEMA_VERSION,
        });
    }
    Ok(plan)
}

/// Write a plan file to disk (pretty-printed).
pub(crate) async fn save(path: &Path, plan: &Plan) -> Result<(), PlanError> {
    let json = serde_json::to_string_pretty(plan).map_err(PlanError::Serialize)?;
    tokio::fs::write(path, json)
        .await
        .map_err(|source| PlanError::Write {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn sample_plan() -> Result<Plan, Box<dyn std::error::Error>> {
        let ts = Utc
            .with_ymd_and_hms(2026, 4, 18, 14, 32, 11)
            .single()
            .ok_or("bad timestamp")?;
        let ts2 = Utc
            .with_ymd_and_hms(2026, 4, 18, 14, 28, 3)
            .single()
            .ok_or("bad timestamp")?;
        Ok(Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            clipcast_version: "0.1.0".to_string(),
            generated_at: ts,
            model: "claude-opus-4-7".to_string(),
            decisions_ref: DecisionsRef {
                path: PathBuf::from("trip.decisions.json"),
                generated_at: ts2,
            },
            brief: "test brief".to_string(),
            target_duration_s: 180,
            estimated_duration_s: 26.2,
            segments: vec![
                Segment {
                    order: 1,
                    source: PathBuf::from("a.mp4"),
                    start_s: None,
                    end_s: None,
                    duration_s: 12.4,
                    title: "Opener".to_string(),
                    rationale: "Sets the scene.".to_string(),
                    trim_reason: None,
                },
                Segment {
                    order: 2,
                    source: PathBuf::from("b.mp4"),
                    start_s: Some(4.2),
                    end_s: Some(18.0),
                    duration_s: 13.8,
                    title: "Climax".to_string(),
                    rationale: "Hooks the viewer.".to_string(),
                    trim_reason: Some("dead time at start".to_string()),
                },
            ],
            rejected: vec![RejectedClip {
                source: PathBuf::from("c.mp4"),
                score: 3,
                rejected_reason: "redundant".to_string(),
            }],
            warnings: vec![],
        })
    }

    #[test]
    fn plan_round_trips_through_json() -> TestResult {
        let p = sample_plan()?;
        let json = serde_json::to_string_pretty(&p)?;
        let parsed: Plan = serde_json::from_str(&json)?;
        assert_eq!(parsed.schema_version, PLAN_SCHEMA_VERSION);
        assert_eq!(parsed.segments.len(), 2);
        assert_eq!(parsed.segments[1].start_s, Some(4.2));
        assert_eq!(parsed.segments[0].start_s, None);
        assert_eq!(parsed.rejected[0].source, PathBuf::from("c.mp4"));
        Ok(())
    }

    #[test]
    fn explicit_null_trim_serializes_as_null_not_omitted() -> TestResult {
        let p = sample_plan()?;
        let json = serde_json::to_string(&p)?;
        assert!(json.contains("\"start_s\":null"));
        assert!(json.contains("\"end_s\":null"));
        Ok(())
    }

    #[tokio::test]
    async fn write_and_read_round_trips() -> TestResult {
        let dir = tempfile::TempDir::new()?;
        let path = dir.path().join("p.plan.json");
        let p = sample_plan()?;
        save(&path, &p).await?;
        let loaded = load(&path).await?;
        assert_eq!(loaded.brief, p.brief);
        Ok(())
    }

    #[tokio::test]
    async fn load_rejects_wrong_schema_version() -> TestResult {
        let dir = tempfile::TempDir::new()?;
        let path = dir.path().join("bad.plan.json");
        let bogus = serde_json::json!({
            "schema_version": 9999,
            "clipcast_version": "0.0.0",
            "generated_at": "2026-04-18T14:32:11Z",
            "model": "m",
            "decisions_ref": {
                "path": "x.decisions.json",
                "generated_at": "2026-04-18T14:28:03Z",
            },
            "brief": "",
            "target_duration_s": 60,
            "estimated_duration_s": 0.0,
            "segments": [],
        });
        tokio::fs::write(&path, serde_json::to_string(&bogus)?).await?;
        let err = load(&path).await.err().ok_or("expected error")?;
        if !matches!(err, PlanError::UnsupportedVersion { .. }) {
            return Err(format!("wrong variant: {err:?}").into());
        }
        Ok(())
    }
}
