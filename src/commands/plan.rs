//! `clipcast plan <input-dir>` — write or revise plan.json.

use crate::duration;
use crate::output::{print_error, print_success, want_json, ErrorEnvelope, SuccessEnvelope};
use crate::paths;
use crate::pipeline::plan as pipeline_plan;
use crate::plan as plan_types;
use crate::sidecar::{self, Sidecar};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use std::io::Write;
use std::path::{Path, PathBuf};

const MODEL_LABEL: &str = "claude-opus-4-7";

/// Arguments to `commands::plan::run`, bundled to keep clippy happy and
/// to mirror the `Plan` clap subcommand 1:1.
pub(crate) struct PlanArgs {
    pub(crate) input_dir: PathBuf,
    pub(crate) duration_str: Option<String>,
    pub(crate) brief: Option<String>,
    pub(crate) brief_file: Option<PathBuf>,
    pub(crate) out: Option<PathBuf>,
    pub(crate) revise: bool,
    pub(crate) instructions: Option<String>,
    pub(crate) dry_run: bool,
    pub(crate) json: bool,
}

pub(crate) async fn run(args: PlanArgs) -> Result<()> {
    let json_mode = want_json(args.json);

    let output_path = args
        .out
        .clone()
        .unwrap_or_else(|| paths::default_output(&args.input_dir, Utc::now()));
    let decisions_path = paths::sidecar_for(&output_path);
    let plan_path = paths::plan_for(&output_path);

    let Some(decisions) = load_decisions(&decisions_path, json_mode).await? else {
        return Ok(());
    };

    let Some(brief_text) = resolve_brief(&args, &plan_path, json_mode).await? else {
        return Ok(());
    };

    let Some(target_duration_s) = resolve_duration(&args, &plan_path, json_mode).await? else {
        return Ok(());
    };

    if args.dry_run {
        emit_dry_run(
            json_mode,
            &decisions_path,
            &decisions,
            &brief_text,
            target_duration_s,
        )?;
        return Ok(());
    }

    let plan = run_planner(
        &args,
        &decisions,
        &decisions_path,
        &plan_path,
        brief_text,
        target_duration_s,
    )
    .await?;

    plan_types::save(&plan_path, &plan)
        .await
        .context("write plan.json")?;
    emit_success(json_mode, &args.input_dir, &plan_path, &plan);
    Ok(())
}

async fn load_decisions(decisions_path: &Path, json_mode: bool) -> Result<Option<Sidecar>> {
    if !decisions_path.exists() {
        emit_error(
            json_mode,
            "missing_decisions",
            format!("decisions.json not found at {}", decisions_path.display()),
            "run `clipcast analyze <dir>` first",
        )?;
        return Ok(None);
    }
    let decisions = sidecar::read(decisions_path)
        .await
        .context("read decisions.json")?;
    if decisions.clips.is_empty() {
        emit_error(
            json_mode,
            "empty_decisions",
            "decisions.json contains no clips",
            "re-run `clipcast analyze <dir>` with a non-empty input dir",
        )?;
        return Ok(None);
    }
    Ok(Some(decisions))
}

async fn resolve_brief(
    args: &PlanArgs,
    plan_path: &Path,
    json_mode: bool,
) -> Result<Option<String>> {
    match (args.revise, &args.brief, &args.brief_file) {
        (true, _, _) => {
            if !plan_path.exists() {
                emit_error(
                    json_mode,
                    "revise_without_plan",
                    format!("no plan.json at {} to revise", plan_path.display()),
                    "run `clipcast plan` without --revise first",
                )?;
                return Ok(None);
            }
            Ok(Some(String::new()))
        }
        (false, Some(b), _) => Ok(Some(b.clone())),
        (false, None, Some(p)) => {
            let text = tokio::fs::read_to_string(p)
                .await
                .with_context(|| format!("read brief file {}", p.display()))?;
            Ok(Some(text))
        }
        (false, None, None) => {
            emit_error(
                json_mode,
                "missing_brief",
                "no --brief or --brief-file provided",
                "pass --brief \"...\" or --brief-file <path>",
            )?;
            Ok(None)
        }
    }
}

async fn resolve_duration(
    args: &PlanArgs,
    plan_path: &Path,
    json_mode: bool,
) -> Result<Option<u64>> {
    match &args.duration_str {
        Some(s) => Ok(Some(
            duration::parse(s).context("parse --duration")?.as_secs(),
        )),
        None if args.revise => {
            let existing = plan_types::load(plan_path)
                .await
                .context("load existing plan")?;
            Ok(Some(existing.target_duration_s))
        }
        None => {
            emit_error(
                json_mode,
                "missing_duration",
                "no --duration provided",
                "pass --duration 3m (or seconds, etc.)",
            )?;
            Ok(None)
        }
    }
}

fn emit_dry_run(
    json_mode: bool,
    decisions_path: &Path,
    decisions: &Sidecar,
    brief_text: &str,
    target_duration_s: u64,
) -> Result<()> {
    let prompt = pipeline_plan::render_planner_prompt(brief_text, target_duration_s, decisions);
    if json_mode {
        print_success(
            &SuccessEnvelope::new(serde_json::json!({
                "dry_run": true,
                "decisions_path": decisions_path,
                "candidate_clip_count": decisions.clips.len(),
                "prompt_preview_chars": prompt.len(),
            }))
            .with_next_action("re-run without --dry-run to invoke the planner"),
        )?;
    } else {
        let _ = writeln!(std::io::stderr().lock(), "{prompt}");
    }
    Ok(())
}

async fn run_planner(
    args: &PlanArgs,
    decisions: &Sidecar,
    decisions_path: &Path,
    plan_path: &Path,
    brief_text: String,
    target_duration_s: u64,
) -> Result<plan_types::Plan> {
    if args.revise {
        let existing = plan_types::load(plan_path)
            .await
            .context("load existing plan")?;
        let instr = args
            .instructions
            .clone()
            .ok_or_else(|| anyhow!("--revise requires --instructions"))?;
        pipeline_plan::revise(&existing, &instr, decisions, MODEL_LABEL)
            .await
            .context("planner revise failed")
    } else {
        pipeline_plan::run(
            brief_text,
            target_duration_s,
            decisions,
            decisions_path,
            MODEL_LABEL,
        )
        .await
        .context("planner run failed")
    }
}

fn emit_success(json_mode: bool, input_dir: &Path, plan_path: &Path, plan: &plan_types::Plan) {
    let next_action = format!(
        "review {} then run `clipcast render {}`",
        plan_path.display(),
        input_dir.display()
    );
    if json_mode {
        let env = SuccessEnvelope::new(serde_json::json!({
            "plan_path": plan_path,
            "segments": plan.segments.len(),
            "rejected": plan.rejected.len(),
            "warnings": plan.warnings,
            "estimated_duration_s": plan.estimated_duration_s,
            "target_duration_s": plan.target_duration_s,
        }))
        .with_next_action(next_action.clone())
        .with_next_action_reason("plan.json written; user review before render");
        let _ = print_success(&env);
    } else {
        println!("wrote {}", plan_path.display());
        println!(
            "{} segments planned, {} rejected",
            plan.segments.len(),
            plan.rejected.len()
        );
        println!("next: {next_action}");
    }
}

fn emit_error(
    json_mode: bool,
    code: &str,
    msg: impl Into<String>,
    fix: impl Into<String>,
) -> Result<()> {
    let msg = msg.into();
    let fix = fix.into();
    if json_mode {
        print_error(&ErrorEnvelope::new(code, &msg, &fix))?;
    } else {
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "error [{code}]: {msg}");
        let _ = writeln!(stderr, "fix: {fix}");
    }
    Err(anyhow!("{code}: {msg}"))
}
