//! Pipeline stages: discover, frames, analyze, filter, concat.
//!
//! This module is a router: no runtime logic, just declarations.

pub(crate) mod analyze;
pub(crate) mod concat;
pub(crate) mod discover;
pub(crate) mod frames;
pub(crate) mod plan;
pub(crate) mod transcribe;
