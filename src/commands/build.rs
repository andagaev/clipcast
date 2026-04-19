//! `clipcast build <input-dir>` — full pipeline one-shot:
//! discover → transcribe → frames → analyze → plan (LLM) → render.

use crate::analyzer::claude_print::ClaudePrintAnalyzer;
use crate::paths;
use crate::pipeline::{analyze, concat, discover, frames, plan as pipeline_plan, transcribe};
use crate::plan as plan_types;
use crate::preflight;
use crate::prompts;
use crate::sidecar;
use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const MODEL_LABEL: &str = "claude-opus-4-7";

/// Run the full build pipeline.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    input_dir: &Path,
    target_duration: Duration,
    out: Option<PathBuf>,
    concurrency: usize,
    recursive: bool,
    profile: &str,
    whisper_model: Option<&Path>,
    brief: Option<String>,
    brief_file: Option<PathBuf>,
) -> Result<()> {
    preflight::check_binaries().context("preflight: missing binary")?;
    preflight::check_input_dir(input_dir, recursive).context("preflight: input dir")?;

    let profile_body = prompts::resolve(profile).context("resolve prompt profile")?;
    let whisper_model = preflight::resolve_whisper_model(whisper_model);
    if whisper_model.is_none() {
        println!("note: whisper-cli or model not found — transcription skipped");
    }

    let brief_text = resolve_brief(brief, brief_file).await?;

    let output_path = out.unwrap_or_else(|| paths::default_output(input_dir, Utc::now()));
    let output_abs = output_path
        .canonicalize()
        .unwrap_or_else(|_| output_path.clone());

    let mut clips = discover::run(input_dir, recursive)
        .await
        .context("discover stage failed")?;
    clips.retain(|c| c.path.canonicalize().map_or(true, |abs| abs != output_abs));
    println!("discovered {} clips", clips.len());

    let metas_by_path: HashMap<PathBuf, (u32, u32)> = clips
        .iter()
        .map(|c| (c.path.clone(), (c.meta.width, c.meta.height)))
        .collect();

    transcribe::run(&mut clips, whisper_model.as_deref())
        .await
        .context("transcribe stage failed")?;
    let transcribed = clips.iter().filter(|c| c.transcript.is_some()).count();
    if whisper_model.is_some() {
        println!("transcribed {transcribed}/{} clips", clips.len());
    }

    let (_tempdir, clip_frames) = frames::run(clips)
        .await
        .context("frame extraction stage failed")?;
    println!("extracted frames for {} clips", clip_frames.len());

    let analyzer = Arc::new(ClaudePrintAnalyzer::new(profile_body));
    let verdicts = analyze::run(analyzer, clip_frames, concurrency).await;
    println!("analyzed {} clips (profile: {profile})", verdicts.len());

    let sidecar_path = paths::sidecar_for(&output_path);
    let sidecar_payload = sidecar::build(target_duration.as_secs(), verdicts);
    sidecar::write(&sidecar_path, &sidecar_payload)
        .await
        .context("sidecar write failed")?;
    println!("wrote {}", sidecar_path.display());

    let plan = pipeline_plan::run(
        brief_text,
        target_duration.as_secs(),
        &sidecar_payload,
        &sidecar_path,
        MODEL_LABEL,
    )
    .await
    .context("plan stage failed")?;
    let plan_path = paths::plan_for(&output_path);
    plan_types::save(&plan_path, &plan)
        .await
        .context("plan write failed")?;
    println!(
        "wrote {} ({} segments, {} rejected)",
        plan_path.display(),
        plan.segments.len(),
        plan.rejected.len()
    );

    concat::run_segments(&plan.segments, &metas_by_path, &output_path)
        .await
        .context("concat stage failed")?;
    println!("wrote {}", output_path.display());

    Ok(())
}

async fn resolve_brief(brief: Option<String>, brief_file: Option<PathBuf>) -> Result<String> {
    match (brief, brief_file) {
        (Some(b), _) => Ok(b),
        (None, Some(p)) => tokio::fs::read_to_string(&p)
            .await
            .with_context(|| format!("read brief file {}", p.display())),
        (None, None) => Ok(pipeline_plan::DEFAULT_BRIEF.to_string()),
    }
}
