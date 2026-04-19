//! `clipcast list <input-dir>` — print the current sidecar state.
//!
//! Two output modes:
//! - Default: human-readable, 1-indexed positions, scores + reasons
//! - `--json`: machine-readable projection of the sidecar with positions
//!   injected, suitable for an agent to parse

use crate::paths;
use crate::sidecar;
use anyhow::{Context, Result};
use chrono::Utc;
use std::path::{Path, PathBuf};

/// Run the list command.
pub(crate) async fn run(input_dir: &Path, out: Option<PathBuf>, json: bool) -> Result<()> {
    let output_path = out.unwrap_or_else(|| paths::default_output(input_dir, Utc::now()));
    let sidecar_path = paths::sidecar_for(&output_path);
    let side = sidecar::read(&sidecar_path)
        .await
        .context("read decisions.json")?;

    if json {
        print_json(&sidecar_path, &side)?;
    } else {
        print_human(&sidecar_path, &side);
    }
    Ok(())
}

fn print_human(sidecar_path: &Path, side: &sidecar::Sidecar) {
    let total_count = side.clips.len();
    let scored = side
        .clips
        .iter()
        .filter(|c| c.score.is_some() && c.error.is_none())
        .count();

    println!("Sidecar: {}", sidecar_path.display());
    println!(
        "Target: {}s  Scored: {scored} of {total_count} clips",
        side.target_duration_s
    );
    println!();

    for (i, c) in side.clips.iter().enumerate() {
        let name = c.path.file_name().map_or_else(
            || c.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let score = c.score.map_or_else(|| "-".to_string(), |s| s.to_string());
        let reason = c.reason.as_deref().unwrap_or("");
        println!(
            "{pos:>3}. {name:<40} ({dur:5.1}s)  score={score}  {reason}",
            pos = i + 1,
            dur = c.duration_s,
        );
    }
}

fn print_json(sidecar_path: &Path, side: &sidecar::Sidecar) -> Result<()> {
    let scored = side
        .clips
        .iter()
        .filter(|c| c.score.is_some() && c.error.is_none())
        .count();

    let clips: Vec<serde_json::Value> = side
        .clips
        .iter()
        .enumerate()
        .map(|(i, c)| {
            serde_json::json!({
                "position": i + 1,
                "path": c.path,
                "duration_s": c.duration_s,
                "score": c.score,
                "reason": c.reason,
                "error": c.error,
                "timestamp": c.timestamp,
                "timestamp_source": c.timestamp_source,
                "transcript": c.transcript,
            })
        })
        .collect();

    let out = serde_json::json!({
        "sidecar_path": sidecar_path,
        "schema_version": side.schema_version,
        "clipcast_version": side.clipcast_version,
        "generated_at": side.generated_at,
        "target_duration_s": side.target_duration_s,
        "total_clips": side.clips.len(),
        "scored_clips": scored,
        "clips": clips,
    });

    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
