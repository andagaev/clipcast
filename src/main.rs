//! clipcast — personal CLI that turns a directory of short video clips
//! into one combined vlog video via multimodal LLM scoring.

mod analyzer;
mod clip;
mod commands;
mod duration;
mod paths;
mod pipeline;
mod preflight;
mod process;
mod sidecar;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "clipcast", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the full pipeline: discover, analyze, filter, concat.
    Build {
        /// Path to the input directory containing .mp4 / .mov clips.
        input_dir: PathBuf,
        /// Target vlog duration (e.g., "3m", "2m30s", "90s", "300").
        #[arg(long, default_value = "3m")]
        duration: String,
        /// Override the output .mp4 path. Default: <input-dir>/vlog-YYYY-MM-DD.mp4
        #[arg(long)]
        out: Option<PathBuf>,
        /// Max concurrent LLM calls.
        #[arg(long, default_value_t = 3)]
        concurrency: usize,
        /// Scan subdirectories too.
        #[arg(long)]
        recursive: bool,
    },
    /// Run discover + frame extraction + LLM scoring + filter, then write
    /// decisions.json. Stops before concat.
    Analyze {
        /// Path to the input directory containing .mp4 / .mov clips.
        input_dir: PathBuf,
        /// Target vlog duration (for budget-fill in the filter stage).
        #[arg(long, default_value = "3m")]
        duration: String,
        /// Override the sidecar path base. Default: <input-dir>/vlog-YYYY-MM-DD.mp4
        #[arg(long)]
        out: Option<PathBuf>,
        /// Max concurrent LLM calls.
        #[arg(long, default_value_t = 3)]
        concurrency: usize,
        /// Scan subdirectories too.
        #[arg(long)]
        recursive: bool,
    },
    /// Read an existing decisions.json sidecar and concat the kept clips
    /// (trusting the sidecar's `keep` values as authoritative).
    Render {
        /// Path to the input directory containing the video clips and sidecar.
        input_dir: PathBuf,
        /// Override the output .mp4 path.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Build {
            input_dir,
            duration,
            out,
            concurrency,
            recursive,
        } => {
            let target = duration::parse(&duration)?;
            commands::build::run(&input_dir, target, out, concurrency, recursive).await
        }
        Command::Analyze {
            input_dir,
            duration,
            out,
            concurrency,
            recursive,
        } => {
            let target = duration::parse(&duration)?;
            commands::analyze::run(&input_dir, target, out, concurrency, recursive).await
        }
        Command::Render { input_dir, out } => commands::render::run(&input_dir, out).await,
    }
}
