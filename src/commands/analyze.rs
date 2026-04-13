//! `clipcast analyze <input-dir>` — discover + frames + LLM + filter + sidecar.
//! Stops before concat.

use crate::analyzer::claude_print::ClaudePrintAnalyzer;
use crate::paths;
use crate::pipeline::{analyze, discover, filter, frames};
use crate::preflight;
use crate::sidecar;
use anyhow::{Context, Result};
use chrono::Utc;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Run the analyze-only pipeline.
pub(crate) async fn run(
    input_dir: &Path,
    target_duration: Duration,
    out: Option<PathBuf>,
    concurrency: usize,
    recursive: bool,
) -> Result<()> {
    preflight::check_binaries().context("preflight: missing binary")?;
    preflight::check_input_dir(input_dir, recursive).context("preflight: input dir")?;

    let output_path = out.unwrap_or_else(|| paths::default_output(input_dir, Utc::now()));

    let clips = discover::run(input_dir, recursive)
        .await
        .context("discover stage failed")?;
    let (_tempdir, clip_frames) = frames::run(clips)
        .await
        .context("frame extraction stage failed")?;

    let profile_body = crate::prompts::resolve("default").context("resolve prompt profile")?;
    let analyzer = Arc::new(ClaudePrintAnalyzer::new(profile_body));
    let mut verdicts = analyze::run(analyzer, clip_frames, concurrency).await;

    filter::apply(&mut verdicts, target_duration).context("filter stage failed")?;

    let sidecar_path = paths::sidecar_for(&output_path);
    let sidecar_payload = sidecar::build(target_duration.as_secs(), verdicts);
    sidecar::write(&sidecar_path, &sidecar_payload)
        .await
        .context("sidecar write failed")?;
    println!("wrote {}", sidecar_path.display());

    Ok(())
}
