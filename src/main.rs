//! clipcast — personal CLI that turns a directory of short video clips
//! into one combined vlog video via multimodal LLM scoring.

mod analyzer;
mod clip;
mod commands;
mod duration;
mod output;
mod paths;
mod pipeline;
mod plan;
mod preflight;
mod process;
mod prompts;
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
    /// Run the full pipeline: discover, analyze, plan (LLM), render.
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
        /// Prompt profile: default | adventure | family.
        #[arg(long, default_value = "default")]
        prompt_profile: String,
        /// Explicit path to a whisper `.bin` model (multilingual recommended).
        #[arg(long)]
        whisper_model: Option<PathBuf>,
        /// Planner brief (tone/focus). Overrides the default brief.
        #[arg(long, conflicts_with = "brief_file")]
        brief: Option<String>,
        /// Planner brief read from a file (markdown welcome).
        #[arg(long, conflicts_with = "brief")]
        brief_file: Option<PathBuf>,
    },
    /// Run discover + frame extraction + LLM scoring, then write
    /// decisions.json. Stops before the plan stage.
    Analyze {
        /// Path to the input directory containing .mp4 / .mov clips.
        input_dir: PathBuf,
        /// Target vlog duration (propagated to the sidecar).
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
        /// Prompt profile: default | adventure | family.
        #[arg(long, default_value = "default")]
        prompt_profile: String,
        /// Explicit path to a whisper `.bin` model (multilingual recommended).
        #[arg(long)]
        whisper_model: Option<PathBuf>,
    },
    /// Generate or revise a vlog plan from existing decisions.json.
    Plan {
        /// Input clips directory (used to locate decisions.json + plan.json).
        input_dir: PathBuf,
        /// Target duration. Required for fresh plan; ignored on --revise.
        #[arg(long)]
        duration: Option<String>,
        /// Vlog brief (freeform string).
        #[arg(long, conflicts_with = "brief_file")]
        brief: Option<String>,
        /// Vlog brief read from a file (markdown welcome).
        #[arg(long, conflicts_with = "brief")]
        brief_file: Option<PathBuf>,
        /// Output `.mp4` path (defaults to alongside `input_dir`).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Revise the existing plan.json instead of creating fresh.
        #[arg(long, requires = "instructions")]
        revise: bool,
        /// Revision instructions (only valid with --revise).
        #[arg(long)]
        instructions: Option<String>,
        /// Print the prompt + candidate clips without calling the LLM.
        #[arg(long)]
        dry_run: bool,
        /// Force structured JSON output on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Read an existing plan.json and concat the planned segments.
    Render {
        /// Path to the input directory containing the video clips + plan.json.
        input_dir: PathBuf,
        /// Override the output .mp4 path.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Print the ffmpeg commands that would run, without executing them.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print the current sidecar state (clips, order, scores).
    List {
        /// Path to the input directory containing the sidecar.
        input_dir: PathBuf,
        /// Override the output .mp4 path (used to locate the sidecar).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Emit machine-readable JSON instead of human text.
        #[arg(long)]
        json: bool,
    },
    /// Print the current state of a clipcast project as JSON or text.
    Status {
        /// Path to the input directory.
        input_dir: PathBuf,
        /// Override the output `.mp4` path (used to locate the sidecars).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Force structured JSON output on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Print the JSON schema for a clipcast sidecar.
    Schema {
        /// Which schema: "decisions" or "plan".
        kind: String,
    },
    /// Analyze one new clip with the LLM and append it to the sidecar.
    /// Run `clipcast plan --revise ...` afterwards to incorporate it.
    Add {
        /// Path to the input directory containing the sidecar.
        input_dir: PathBuf,
        /// Absolute or relative path to the clip to analyze.
        clip: PathBuf,
        /// Override the output .mp4 path (used to locate the sidecar).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Prompt profile: default | adventure | family.
        #[arg(long, default_value = "default")]
        prompt_profile: String,
        /// Explicit path to a whisper `.bin` model (multilingual recommended).
        #[arg(long)]
        whisper_model: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    dispatch(cli.command).await
}

async fn dispatch(cmd: Command) -> Result<()> {
    match cmd {
        Command::Build { .. } => run_build(cmd).await,
        Command::Analyze { .. } => run_analyze(cmd).await,
        Command::Plan {
            input_dir,
            duration,
            brief,
            brief_file,
            out,
            revise,
            instructions,
            dry_run,
            json,
        } => {
            commands::plan::run(commands::plan::PlanArgs {
                input_dir,
                duration_str: duration,
                brief,
                brief_file,
                out,
                revise,
                instructions,
                dry_run,
                json,
            })
            .await
        }
        Command::Render {
            input_dir,
            out,
            dry_run,
        } => commands::render::run(&input_dir, out, dry_run).await,
        Command::List {
            input_dir,
            out,
            json,
        } => commands::list::run(&input_dir, out, json).await,
        Command::Status {
            input_dir,
            out,
            json,
        } => commands::status::run(&input_dir, out, json).await,
        Command::Schema { kind } => commands::schema::run(&kind),
        Command::Add {
            input_dir,
            clip,
            out,
            prompt_profile,
            whisper_model,
        } => {
            commands::add::run(
                &input_dir,
                &clip,
                out,
                &prompt_profile,
                whisper_model.as_deref(),
            )
            .await
        }
    }
}

async fn run_build(cmd: Command) -> Result<()> {
    let Command::Build {
        input_dir,
        duration,
        out,
        concurrency,
        recursive,
        prompt_profile,
        whisper_model,
        brief,
        brief_file,
    } = cmd
    else {
        unreachable!("run_build only handles Command::Build");
    };
    let target = duration::parse(&duration)?;
    commands::build::run(
        &input_dir,
        target,
        out,
        concurrency,
        recursive,
        &prompt_profile,
        whisper_model.as_deref(),
        brief,
        brief_file,
    )
    .await
}

async fn run_analyze(cmd: Command) -> Result<()> {
    let Command::Analyze {
        input_dir,
        duration,
        out,
        concurrency,
        recursive,
        prompt_profile,
        whisper_model,
    } = cmd
    else {
        unreachable!("run_analyze only handles Command::Analyze");
    };
    let target = duration::parse(&duration)?;
    commands::analyze::run(
        &input_dir,
        target,
        out,
        concurrency,
        recursive,
        &prompt_profile,
        whisper_model.as_deref(),
    )
    .await
}
