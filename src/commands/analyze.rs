//! `clipcast analyze <input-dir>` — discover + frames + LLM scoring + sidecar.
//! Stops before the plan stage; next-step is `clipcast plan`.

use crate::analyzer::claude_print::ClaudePrintAnalyzer;
use crate::paths;
use crate::pipeline::{analyze, discover, frames, transcribe};
use crate::preflight;
use crate::prompts;
use crate::sidecar;
use anyhow::{Context, Result};
use chrono::Utc;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Run the analyze-only pipeline.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    input_dir: &Path,
    target_duration: Duration,
    out: Option<PathBuf>,
    concurrency: usize,
    recursive: bool,
    profile: &str,
    whisper_model: Option<&Path>,
) -> Result<()> {
    preflight::check_binaries().context("preflight: missing binary")?;
    preflight::check_input_dir(input_dir, recursive).context("preflight: input dir")?;

    let profile_body = prompts::resolve(profile).context("resolve prompt profile")?;
    let whisper_model = preflight::resolve_whisper_model(whisper_model);

    let output_path = out.unwrap_or_else(|| paths::default_output(input_dir, Utc::now()));
    let output_abs = output_path
        .canonicalize()
        .unwrap_or_else(|_| output_path.clone());

    let mut clips = discover::run(input_dir, recursive)
        .await
        .context("discover stage failed")?;
    clips.retain(|c| c.path.canonicalize().map_or(true, |abs| abs != output_abs));

    transcribe::run(&mut clips, whisper_model.as_deref())
        .await
        .context("transcribe stage failed")?;

    let (_tempdir, clip_frames) = frames::run(clips)
        .await
        .context("frame extraction stage failed")?;

    let analyzer = Arc::new(ClaudePrintAnalyzer::new(profile_body));
    let verdicts = analyze::run(analyzer, clip_frames, concurrency).await;

    let sidecar_path = paths::sidecar_for(&output_path);
    let sidecar_payload = sidecar::build(target_duration.as_secs(), verdicts);
    sidecar::write(&sidecar_path, &sidecar_payload)
        .await
        .context("sidecar write failed")?;
    println!("wrote {}", sidecar_path.display());
    println!(
        "next: clipcast plan {} --brief '...' --duration {}s",
        input_dir.display(),
        target_duration.as_secs()
    );

    Ok(())
}
