//! `ClipAnalyzer` trait and its implementations.
//!
//! The trait abstracts "score a single clip given its frames" so the
//! rest of the pipeline doesn't care which backend is doing the work.
//! v1 has one impl (`claude_print::ClaudePrintAnalyzer`). Future work
//! adds `claude_api::ClaudeApiAnalyzer` (direct Anthropic SDK) and
//! `gemini::GeminiVideoAnalyzer` (native video input).
//!
//! ## `#[allow(async_fn_in_trait)]`
//!
//! Rust requires either the `async-trait` crate or manual `BoxFuture`
//! juggling to make async trait methods dyn-compatible. v1 has one
//! static impl and doesn't need dynamic dispatch, so we accept the
//! language-level lint here as a single documented exception. When
//! we add a second backend that needs dynamic dispatch, switch to
//! `async-trait`.

#![allow(async_fn_in_trait)]

use crate::clip::{Clip, ClipVerdict};
use crate::pipeline::frames::FramesError;
use crate::process::ProcessError;
use std::path::Path;

pub(crate) mod claude_print;

/// Errors returned by any `ClipAnalyzer` implementation.
#[derive(Debug, thiserror::Error)]
pub(crate) enum AnalyzerError {
    #[error(transparent)]
    Process(#[from] ProcessError),

    #[error("LLM returned unparseable output: {details}\nraw: {raw}")]
    ParseFailed { details: String, raw: String },

    #[error("LLM returned an empty response")]
    Empty,

    #[error(transparent)]
    Frames(#[from] FramesError),
}

/// Score a single clip and return a verdict.
///
/// Implementations receive the clip metadata and a slice of
/// pre-extracted frame paths. They return either a full `ClipVerdict`
/// or an error. The orchestrator in `pipeline::analyze` converts
/// errors into `ClipVerdict { error: Some(...), ... }` entries.
pub(crate) trait ClipAnalyzer {
    async fn analyze(&self, clip: &Clip, frames: &[&Path]) -> Result<ClipVerdict, AnalyzerError>;
}
