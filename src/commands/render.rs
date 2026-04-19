//! `clipcast render <input-dir>` — read plan.json + ffmpeg trim + concat.

use crate::paths;
use crate::pipeline::{concat, discover};
use crate::plan as plan_types;
use crate::preflight;
use crate::sidecar;
use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Run the render-only pipeline. Trusts `plan.json` as authoritative.
pub(crate) async fn run(input_dir: &Path, out: Option<PathBuf>, dry_run: bool) -> Result<()> {
    preflight::check_binaries().context("preflight: missing binary")?;
    preflight::check_input_dir(input_dir, false).context("preflight: input dir")?;

    let output_path = out.unwrap_or_else(|| paths::default_output(input_dir, Utc::now()));
    let plan_path = paths::plan_for(&output_path);
    let sidecar_path = paths::sidecar_for(&output_path);

    let plan = plan_types::load(&plan_path)
        .await
        .context("read plan.json")?;

    // Stale-plan warning: if decisions.json was updated after the plan was
    // generated, the plan may reference stale scores/transcripts.
    if sidecar_path.exists() {
        if let Ok(side) = sidecar::read(&sidecar_path).await {
            if side.generated_at > plan.decisions_ref.generated_at {
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "warning: {} is newer than the plan's decisions_ref.generated_at; \
                     consider regenerating the plan (`clipcast plan {}`)",
                    sidecar_path.display(),
                    input_dir.display()
                );
            }
        }
    }

    let clips = discover::run(input_dir, false)
        .await
        .context("discover stage failed")?;
    let metas_by_path: HashMap<PathBuf, (u32, u32)> = clips
        .iter()
        .map(|c| (c.path.clone(), (c.meta.width, c.meta.height)))
        .collect();

    if dry_run {
        println!("plan: {}", plan_path.display());
        println!(
            "{} segments, estimated {:.1}s",
            plan.segments.len(),
            plan.estimated_duration_s
        );
        for cmd in concat::dry_run_commands(&plan.segments, &output_path) {
            println!("ffmpeg {}", cmd.join(" "));
        }
        return Ok(());
    }

    concat::run_segments(&plan.segments, &metas_by_path, &output_path)
        .await
        .context("concat stage failed")?;
    println!("wrote {}", output_path.display());

    Ok(())
}
