//! `clipcast add <input-dir> <clip-path>` — analyze one new clip and
//! append it to the existing sidecar with `keep = true`.
//!
//! Reads the sidecar, runs ffprobe + whisper (if available) + frame
//! extraction + LLM scoring on the new clip, then writes the sidecar
//! back with the new verdict appended. Does not re-score existing clips.

use crate::analyzer::claude_print::ClaudePrintAnalyzer;
use crate::analyzer::ClipAnalyzer;
use crate::paths;
use crate::pipeline::{discover, frames, transcribe};
use crate::preflight;
use crate::prompts;
use crate::sidecar;
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use std::path::{Path, PathBuf};

/// Run the add command.
pub(crate) async fn run(
    input_dir: &Path,
    clip_path: &Path,
    out: Option<PathBuf>,
    profile: &str,
    whisper_model: Option<&Path>,
) -> Result<()> {
    preflight::check_binaries().context("preflight: missing binary")?;

    if !clip_path.exists() {
        return Err(anyhow!("clip not found: {}", clip_path.display()));
    }
    if !clip_path.is_file() {
        return Err(anyhow!("not a file: {}", clip_path.display()));
    }

    let clip_path = clip_path
        .canonicalize()
        .with_context(|| format!("resolve {}", clip_path.display()))?;

    let profile_body = prompts::resolve(profile).context("resolve prompt profile")?;
    let whisper_model = preflight::resolve_whisper_model(whisper_model);

    let output_path = out.unwrap_or_else(|| paths::default_output(input_dir, Utc::now()));
    let sidecar_path = paths::sidecar_for(&output_path);
    let mut side = sidecar::read(&sidecar_path)
        .await
        .context("read decisions.json")?;

    if side.clips.iter().any(|c| c.path == clip_path) {
        return Err(anyhow!("clip already in sidecar: {}", clip_path.display()));
    }

    let mut clip = discover::describe(&clip_path)
        .await
        .context("ffprobe failed on new clip")?;
    println!(
        "probed {} ({}x{}, {:.1}s)",
        clip.path.display(),
        clip.meta.width,
        clip.meta.height,
        clip.meta.duration_s
    );

    if let Some(model) = whisper_model.as_deref() {
        match transcribe::transcribe_one(&clip.path, model).await {
            Ok(text) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    println!("transcribed ({} chars)", trimmed.len());
                    clip.transcript = Some(trimmed.to_string());
                }
            }
            Err(e) => println!("transcription failed: {e}; continuing without"),
        }
    }

    let (_tempdir, clip_frames_vec) = frames::run(vec![clip])
        .await
        .context("frame extraction failed")?;
    let cf = clip_frames_vec
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no frames extracted"))?;
    println!("extracted {} frames", cf.frame_paths.len());

    let analyzer = ClaudePrintAnalyzer::new(profile_body);
    let frame_refs: Vec<&Path> = cf.frame_paths.iter().map(PathBuf::as_path).collect();
    let mut verdict = analyzer
        .analyze(&cf.clip, &frame_refs)
        .await
        .context("LLM analysis failed")?;
    verdict.keep = true;
    println!(
        "scored {}: {}",
        verdict
            .score
            .map_or_else(|| "-".to_string(), |s| s.to_string()),
        verdict.reason.as_deref().unwrap_or("")
    );

    side.clips.push(verdict);
    sidecar::write(&sidecar_path, &side)
        .await
        .context("sidecar write failed")?;
    println!("added to {}", sidecar_path.display());

    Ok(())
}
