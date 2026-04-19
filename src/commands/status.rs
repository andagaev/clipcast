//! `clipcast status <input-dir>` — read-only project state inspection.

use crate::output::{print_success, want_json, SuccessEnvelope};
use crate::paths;
use crate::plan as plan_types;
use crate::sidecar;
use anyhow::Result;
use chrono::Utc;
use std::path::{Path, PathBuf};

/// Print the current state of a clipcast project.
pub(crate) async fn run(input_dir: &Path, out: Option<PathBuf>, json: bool) -> Result<()> {
    let json_mode = want_json(json);
    let output_path = out.unwrap_or_else(|| paths::default_output(input_dir, Utc::now()));
    let decisions_path = paths::sidecar_for(&output_path);
    let plan_path = paths::plan_for(&output_path);

    let has_decisions = decisions_path.exists();
    let has_plan = plan_path.exists();
    let has_output = output_path.exists();

    let (stage, next_action, reason) =
        derive_stage(has_decisions, has_plan, has_output, input_dir, &plan_path);

    let clip_count = if has_decisions {
        sidecar::read(&decisions_path)
            .await
            .ok()
            .map(|s| s.clips.len())
    } else {
        None
    };
    let planned_count = if has_plan {
        plan_types::load(&plan_path)
            .await
            .ok()
            .map(|p| p.segments.len())
    } else {
        None
    };

    if json_mode {
        let payload = serde_json::json!({
            "stage": stage,
            "decisions_path": has_decisions.then(|| decisions_path.clone()),
            "plan_path": has_plan.then(|| plan_path.clone()),
            "output_path": has_output.then(|| output_path.clone()),
            "clip_count": clip_count,
            "planned_clip_count": planned_count,
        });
        let mut env = SuccessEnvelope::new(payload);
        if let Some(na) = next_action {
            env = env.with_next_action(na).with_next_action_reason(reason);
        }
        print_success(&env)?;
    } else {
        println!("stage: {stage}");
        if let Some(c) = clip_count {
            println!("analyzed clips: {c}");
        }
        if let Some(p) = planned_count {
            println!("planned segments: {p}");
        }
        if let Some(na) = next_action {
            println!("next: {na}");
            println!("  ({reason})");
        }
    }
    Ok(())
}

fn derive_stage(
    has_decisions: bool,
    has_plan: bool,
    has_output: bool,
    input_dir: &Path,
    plan_path: &Path,
) -> (&'static str, Option<String>, &'static str) {
    match (has_decisions, has_plan, has_output) {
        (false, _, _) => (
            "none",
            Some(format!("clipcast analyze {}", input_dir.display())),
            "no decisions.json yet",
        ),
        (true, false, _) => (
            "analyzed",
            Some(format!(
                "clipcast plan {} --brief '...' --duration 3m",
                input_dir.display()
            )),
            "decisions.json exists; plan.json missing",
        ),
        (true, true, false) => (
            "planned",
            Some(format!(
                "review {} then `clipcast render {}`",
                plan_path.display(),
                input_dir.display()
            )),
            "plan.json exists; final mp4 not yet rendered",
        ),
        (true, true, true) => ("rendered", None, "pipeline complete"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[tokio::test]
    async fn status_reports_none_when_no_files() -> TestResult {
        let dir = tempfile::TempDir::new()?;
        run(dir.path(), Some(dir.path().join("out.mp4")), true).await?;
        Ok(())
    }

    #[test]
    fn derive_stage_none_when_no_decisions() {
        let (stage, next, _) = derive_stage(
            false,
            false,
            false,
            Path::new("/tmp/x"),
            Path::new("/tmp/x.plan.json"),
        );
        assert_eq!(stage, "none");
        assert!(next.is_some());
    }

    #[test]
    fn derive_stage_rendered_has_no_next_action() {
        let (stage, next, _) = derive_stage(
            true,
            true,
            true,
            Path::new("/tmp/x"),
            Path::new("/tmp/x.plan.json"),
        );
        assert_eq!(stage, "rendered");
        assert!(next.is_none());
    }
}
