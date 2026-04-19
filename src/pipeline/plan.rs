//! Planner stage: take a `Sidecar` (decisions.json) + brief, call the
//! planning LLM via `claude -p`, return a fully-populated `Plan`.

use crate::plan::{DecisionsRef, Plan, PlanError, RejectedClip, Segment, PLAN_SCHEMA_VERSION};
use crate::process;
use crate::sidecar::Sidecar;
use serde::Deserialize;
use std::path::Path;

/// Default brief used by `clipcast build` when the user provides none.
pub(crate) const DEFAULT_BRIEF: &str =
    "Assemble a chronological highlight reel of the best clips fitting the target duration. \
     Drop clips that are blurry, redundant, or scored low. \
     Trim dead time at clip starts/ends only when obvious.";

/// Errors from the planning pipeline stage.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PipelinePlanError {
    #[error("planner subprocess failed: {0}")]
    Subprocess(String),
    #[error("planner returned invalid JSON: {message}")]
    InvalidPlannerOutput {
        message: String,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Plan(#[from] PlanError),
}

/// LLM-side payload (subset of `Plan` produced by the model).
#[derive(Debug, Deserialize)]
pub(crate) struct PlannerOutput {
    pub(crate) estimated_duration_s: f64,
    pub(crate) segments: Vec<Segment>,
    #[serde(default)]
    pub(crate) rejected: Vec<RejectedClip>,
    #[serde(default)]
    pub(crate) warnings: Vec<String>,
}

/// Build the prompt sent to `claude -p`. Shared by `run` and `revise`.
pub(crate) fn render_planner_prompt(brief: &str, target_s: u64, decisions: &Sidecar) -> String {
    let mut s = String::new();
    s.push_str(include_str!("../../prompts/plan.md"));
    s.push_str("\n\n---\n\n## Brief\n\n");
    s.push_str(brief);
    s.push_str(&format!("\n\n## Target duration\n\n{target_s} seconds\n\n"));
    s.push_str("## Clips\n\n");
    for c in &decisions.clips {
        let score = c.score.map_or_else(|| "—".to_string(), |x| x.to_string());
        let reason = c.reason.as_deref().unwrap_or("—");
        let transcript = c.transcript.as_deref().unwrap_or("—");
        let err = c.error.as_deref().unwrap_or("");
        s.push_str(&format!(
            "- path: {}\n  duration_s: {:.2}\n  timestamp: {}\n  score: {}\n  reason: {}\n  transcript: {}\n  error: {}\n\n",
            c.path.display(),
            c.duration_s,
            c.timestamp,
            score,
            reason,
            transcript,
            err,
        ));
    }
    s
}

/// Parse the LLM's JSON output. Tolerates leading/trailing whitespace
/// and stripped ```json``` code fences.
pub(crate) fn parse_planner_output(raw: &str) -> Result<PlannerOutput, PipelinePlanError> {
    let trimmed = raw.trim();
    let cleaned = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .trim_end_matches("```")
        .trim();
    serde_json::from_str::<PlannerOutput>(cleaned).map_err(|source| {
        PipelinePlanError::InvalidPlannerOutput {
            message: source.to_string(),
            source,
        }
    })
}

async fn invoke_claude(prompt: &str) -> Result<String, PipelinePlanError> {
    let output = process::run(
        "claude",
        [
            "-p",
            "--input-format",
            "text",
            "--output-format",
            "text",
            "--permission-mode",
            "plan",
            "--settings",
            "/dev/null",
        ],
        [("AGENT_COLLECTOR_IGNORE", "1")],
        Some(prompt.as_bytes().to_vec()),
    )
    .await
    .map_err(|e| PipelinePlanError::Subprocess(e.to_string()))?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run the planner: render prompt, call `claude -p`, parse output, assemble Plan.
pub(crate) async fn run(
    brief: String,
    target_duration_s: u64,
    decisions: &Sidecar,
    decisions_path: &Path,
    model_label: &str,
) -> Result<Plan, PipelinePlanError> {
    let prompt = render_planner_prompt(&brief, target_duration_s, decisions);
    let raw = invoke_claude(&prompt).await?;
    let parsed = parse_planner_output(&raw)?;

    Ok(Plan {
        schema_version: PLAN_SCHEMA_VERSION,
        clipcast_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: chrono::Utc::now(),
        model: model_label.to_string(),
        decisions_ref: DecisionsRef {
            path: decisions_path.to_path_buf(),
            generated_at: decisions.generated_at,
        },
        brief,
        target_duration_s,
        estimated_duration_s: parsed.estimated_duration_s,
        segments: parsed.segments,
        rejected: parsed.rejected,
        warnings: parsed.warnings,
    })
}

/// Revise an existing plan with new instructions.
pub(crate) async fn revise(
    existing: &Plan,
    instructions: &str,
    decisions: &Sidecar,
    model_label: &str,
) -> Result<Plan, PipelinePlanError> {
    let mut prompt = String::new();
    prompt.push_str(include_str!("../../prompts/plan.md"));
    prompt.push_str("\n\n---\n\n## Existing plan to revise\n\n");
    let existing_json = serde_json::to_string_pretty(existing)
        .unwrap_or_else(|_| "(failed to serialize existing plan)".to_string());
    prompt.push_str(&existing_json);
    prompt.push_str("\n\n## Revision instructions\n\n");
    prompt.push_str(instructions);
    prompt.push_str("\n\n## Brief\n\n");
    prompt.push_str(&existing.brief);
    prompt.push_str(&format!(
        "\n\n## Target duration\n\n{} seconds\n\n",
        existing.target_duration_s
    ));
    prompt.push_str("## Clips\n\n");
    for c in &decisions.clips {
        let score = c.score.map_or_else(|| "—".to_string(), |x| x.to_string());
        prompt.push_str(&format!(
            "- path: {}\n  duration_s: {:.2}\n  score: {}\n  reason: {}\n\n",
            c.path.display(),
            c.duration_s,
            score,
            c.reason.as_deref().unwrap_or("—"),
        ));
    }
    let raw = invoke_claude(&prompt).await?;
    let parsed = parse_planner_output(&raw)?;

    Ok(Plan {
        schema_version: PLAN_SCHEMA_VERSION,
        clipcast_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: chrono::Utc::now(),
        model: model_label.to_string(),
        decisions_ref: existing.decisions_ref.clone(),
        brief: existing.brief.clone(),
        target_duration_s: existing.target_duration_s,
        estimated_duration_s: parsed.estimated_duration_s,
        segments: parsed.segments,
        rejected: parsed.rejected,
        warnings: parsed.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{ClipVerdict, TimestampSource};
    use crate::sidecar::DECISIONS_SCHEMA_VERSION;
    use chrono::TimeZone;
    use std::path::PathBuf;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn sample_decisions() -> Result<Sidecar, Box<dyn std::error::Error>> {
        let ts = chrono::Utc
            .with_ymd_and_hms(2026, 4, 18, 12, 0, 0)
            .single()
            .ok_or("bad ts")?;
        Ok(Sidecar {
            schema_version: DECISIONS_SCHEMA_VERSION,
            clipcast_version: "test".to_string(),
            generated_at: ts,
            target_duration_s: 60,
            clips: vec![ClipVerdict {
                path: PathBuf::from("a.mp4"),
                duration_s: 10.0,
                timestamp: ts,
                timestamp_source: TimestampSource::CreationTime,
                score: Some(8),
                reason: Some("nice".to_string()),
                error: None,
                transcript: Some("hello".to_string()),
            }],
        })
    }

    #[test]
    fn render_prompt_includes_brief_and_clip_table() -> TestResult {
        let decisions = sample_decisions()?;
        let rendered = render_planner_prompt("BRIEF TEXT", 60, &decisions);
        assert!(rendered.contains("BRIEF TEXT"));
        assert!(rendered.contains("60 seconds"));
        assert!(rendered.contains("a.mp4"));
        assert!(rendered.contains("hello"));
        Ok(())
    }

    #[test]
    fn parse_planner_output_extracts_segments() -> TestResult {
        let raw = r#"{
            "estimated_duration_s": 10.0,
            "segments": [
                {"order": 1, "source": "a.mp4", "start_s": null, "end_s": null,
                 "duration_s": 10.0, "title": "T", "rationale": "R"}
            ],
            "rejected": [],
            "warnings": []
        }"#;
        let parsed = parse_planner_output(raw)?;
        assert_eq!(parsed.segments.len(), 1);
        assert_eq!(parsed.segments[0].source, PathBuf::from("a.mp4"));
        Ok(())
    }

    #[test]
    fn parse_planner_output_strips_code_fences() -> TestResult {
        let raw = "```json\n{\"estimated_duration_s\":0.0,\"segments\":[]}\n```";
        let parsed = parse_planner_output(raw)?;
        assert_eq!(parsed.segments.len(), 0);
        Ok(())
    }

    #[test]
    fn parse_planner_output_rejects_garbage() {
        let result = parse_planner_output("not json at all");
        assert!(matches!(
            result,
            Err(PipelinePlanError::InvalidPlannerOutput { .. })
        ));
    }
}
