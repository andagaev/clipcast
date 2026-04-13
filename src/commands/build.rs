//! `clipcast build <input-dir>` — full pipeline one-shot.

use crate::analyzer::claude_print::ClaudePrintAnalyzer;
use crate::paths;
use crate::pipeline::{analyze, concat, discover, filter, frames, transcribe};
use crate::preflight;
use crate::prompts;
use crate::sidecar;
use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Run the full build pipeline.
pub(crate) async fn run(
    input_dir: &Path,
    target_duration: Duration,
    out: Option<PathBuf>,
    concurrency: usize,
    recursive: bool,
    profile: &str,
) -> Result<()> {
    preflight::check_binaries().context("preflight: missing binary")?;
    preflight::check_input_dir(input_dir, recursive).context("preflight: input dir")?;

    let profile_body = prompts::resolve(profile).context("resolve prompt profile")?;
    let whisper_model = preflight::resolve_whisper_model();
    if whisper_model.is_none() {
        println!("note: whisper-cli or model not found — transcription skipped");
    }

    let output_path = out.unwrap_or_else(|| paths::default_output(input_dir, Utc::now()));

    let mut clips = discover::run(input_dir, recursive)
        .await
        .context("discover stage failed")?;
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
    let mut verdicts = analyze::run(analyzer, clip_frames, concurrency).await;
    println!("analyzed {} clips (profile: {profile})", verdicts.len());

    filter::apply(&mut verdicts, target_duration).context("filter stage failed")?;
    let kept_count = verdicts.iter().filter(|v| v.keep).count();
    println!(
        "filter kept {kept_count} clips within {}s budget",
        target_duration.as_secs()
    );

    let sidecar_path = paths::sidecar_for(&output_path);
    let sidecar_payload = sidecar::build(target_duration.as_secs(), verdicts.clone());
    sidecar::write(&sidecar_path, &sidecar_payload)
        .await
        .context("sidecar write failed")?;
    println!("wrote {}", sidecar_path.display());

    concat::run(&verdicts, &metas_by_path, &output_path)
        .await
        .context("concat stage failed")?;
    println!("wrote {}", output_path.display());

    Ok(())
}
