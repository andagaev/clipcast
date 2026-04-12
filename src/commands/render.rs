//! `clipcast render <input-dir>` — read sidecar + ffmpeg concat only.

use crate::paths;
use crate::pipeline::{concat, discover};
use crate::preflight;
use crate::sidecar;
use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Run the render-only pipeline. Trusts the sidecar's `keep` values as authoritative.
pub(crate) async fn run(input_dir: &Path, out: Option<PathBuf>) -> Result<()> {
    preflight::check_binaries().context("preflight: missing binary")?;
    preflight::check_input_dir(input_dir, false).context("preflight: input dir")?;

    let output_path = out.unwrap_or_else(|| paths::default_output(input_dir, Utc::now()));
    let sidecar_path = paths::sidecar_for(&output_path);

    let side = sidecar::read(&sidecar_path)
        .await
        .context("read decisions.json")?;

    let clips = discover::run(input_dir, false)
        .await
        .context("discover stage failed")?;
    let metas_by_path: HashMap<PathBuf, (u32, u32)> = clips
        .iter()
        .map(|c| (c.path.clone(), (c.meta.width, c.meta.height)))
        .collect();

    concat::run(&side.clips, &metas_by_path, &output_path)
        .await
        .context("concat stage failed")?;
    println!("wrote {}", output_path.display());

    Ok(())
}
