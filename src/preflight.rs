//! Preflight checks that fail fast with actionable errors before any
//! pipeline stage runs.

use std::path::{Path, PathBuf};

/// Binaries that must be on PATH.
pub(crate) const REQUIRED_BINARIES: &[(&str, &str)] = &[
    ("ffmpeg", "brew install ffmpeg"),
    ("ffprobe", "brew install ffmpeg"),
    ("claude", "already installed if you use Claude Code"),
];

/// Optional binary: whisper.cpp's `whisper-cli` for audio transcription.
/// If missing, clipcast skips transcription and relies on frames alone.
pub(crate) const OPTIONAL_WHISPER: &str = "whisper-cli";

/// Errors returned from preflight checks.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PreflightError {
    #[error(
        "required binary not on PATH: {name}\n\
         Install with: {hint}"
    )]
    MissingBinary {
        name: &'static str,
        hint: &'static str,
    },

    #[error("input directory does not exist: {}", path.display())]
    InputDirMissing { path: PathBuf },

    #[error("input directory is not a directory: {}", path.display())]
    InputDirNotADir { path: PathBuf },

    #[error("no video clips (.mp4 or .mov) found in {}", path.display())]
    NoClipsFound { path: PathBuf },

    #[error("failed to read directory {}: {source}", path.display())]
    ReadDirFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Check that every binary in `REQUIRED_BINARIES` is resolvable via `which`.
pub(crate) fn check_binaries() -> Result<(), PreflightError> {
    for (name, hint) in REQUIRED_BINARIES {
        if which::which(name).is_err() {
            return Err(PreflightError::MissingBinary { name, hint });
        }
    }
    Ok(())
}

/// Check that the input directory exists, is a directory, and contains
/// at least one video clip (.mp4 or .mov) at the top level (or recursively
/// if `recursive`).
pub(crate) fn check_input_dir(dir: &Path, recursive: bool) -> Result<(), PreflightError> {
    if !dir.exists() {
        return Err(PreflightError::InputDirMissing {
            path: dir.to_path_buf(),
        });
    }
    if !dir.is_dir() {
        return Err(PreflightError::InputDirNotADir {
            path: dir.to_path_buf(),
        });
    }

    if has_clip(dir, recursive)? {
        Ok(())
    } else {
        Err(PreflightError::NoClipsFound {
            path: dir.to_path_buf(),
        })
    }
}

fn has_clip(dir: &Path, recursive: bool) -> Result<bool, PreflightError> {
    let entries = std::fs::read_dir(dir).map_err(|source| PreflightError::ReadDirFailed {
        path: dir.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| PreflightError::ReadDirFailed {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_file() && is_video_clip(&path) {
            return Ok(true);
        }
        if recursive && path.is_dir() && has_clip(&path, true)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Accepted input extensions: `.mp4` and `.mov` (case-insensitive).
fn is_video_clip(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mp4") || e.eq_ignore_ascii_case("mov"))
}

/// Resolve the path to a whisper `.bin` model file, if one is available.
///
/// Resolution order (first hit wins):
/// 1. `explicit` argument (from a `--whisper-model` CLI flag)
/// 2. `$CLIPCAST_MODEL` (absolute path to a `.bin`)
/// 3. `$CLIPCAST_MODELS_DIR/ggml-small.bin` (multilingual preferred)
/// 4. `$CLIPCAST_MODELS_DIR/ggml-small.en.bin`
/// 5. `~/.whisper-cpp-models/ggml-small.bin` (multilingual)
/// 6. `~/.whisper-cpp-models/ggml-small.en.bin`
///
/// Multilingual (`ggml-small.bin`) is preferred because it supports 99
/// languages; the `.en` variant is English-only but slightly faster.
/// Returns `None` if `whisper-cli` is missing or no model is found.
pub(crate) fn resolve_whisper_model(explicit: Option<&Path>) -> Option<PathBuf> {
    if which::which(OPTIONAL_WHISPER).is_err() {
        return None;
    }

    if let Some(path) = explicit {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
    }

    if let Some(path) = std::env::var_os("CLIPCAST_MODEL").map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }

    if let Some(dir) = std::env::var_os("CLIPCAST_MODELS_DIR").map(PathBuf::from) {
        for name in ["ggml-small.bin", "ggml-small.en.bin"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for name in ["ggml-small.bin", "ggml-small.en.bin"] {
            let candidate = home.join(".whisper-cpp-models").join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn check_input_dir_ok_with_mp4() -> TestResult {
        let dir = TempDir::new()?;
        std::fs::write(dir.path().join("clip.mp4"), b"fake")?;
        check_input_dir(dir.path(), false)?;
        Ok(())
    }

    #[test]
    fn check_input_dir_fails_on_missing() -> TestResult {
        let err = check_input_dir(Path::new("/definitely/not/real"), false)
            .err()
            .ok_or("expected error")?;
        if !matches!(err, PreflightError::InputDirMissing { .. }) {
            return Err(format!("wrong variant: {err:?}").into());
        }
        Ok(())
    }

    #[test]
    fn check_input_dir_fails_on_file_not_dir() -> TestResult {
        let dir = TempDir::new()?;
        let file = dir.path().join("actually-a-file.mp4");
        std::fs::write(&file, b"fake")?;
        let err = check_input_dir(&file, false)
            .err()
            .ok_or("expected error")?;
        if !matches!(err, PreflightError::InputDirNotADir { .. }) {
            return Err(format!("wrong variant: {err:?}").into());
        }
        Ok(())
    }

    #[test]
    fn check_input_dir_fails_on_empty_dir() -> TestResult {
        let dir = TempDir::new()?;
        let err = check_input_dir(dir.path(), false)
            .err()
            .ok_or("expected error")?;
        if !matches!(err, PreflightError::NoClipsFound { .. }) {
            return Err(format!("wrong variant: {err:?}").into());
        }
        Ok(())
    }

    #[test]
    fn check_input_dir_fails_on_dir_with_only_non_mp4() -> TestResult {
        let dir = TempDir::new()?;
        std::fs::write(dir.path().join("readme.txt"), b"not a clip")?;
        std::fs::write(dir.path().join("photo.jpg"), b"not a clip")?;
        let err = check_input_dir(dir.path(), false)
            .err()
            .ok_or("expected error")?;
        if !matches!(err, PreflightError::NoClipsFound { .. }) {
            return Err(format!("wrong variant: {err:?}").into());
        }
        Ok(())
    }

    #[test]
    fn check_input_dir_recursive_finds_nested_mp4() -> TestResult {
        let dir = TempDir::new()?;
        let nested = dir.path().join("subdir");
        std::fs::create_dir(&nested)?;
        std::fs::write(nested.join("clip.mp4"), b"fake")?;
        check_input_dir(dir.path(), true)?;
        Ok(())
    }

    #[test]
    fn check_input_dir_non_recursive_ignores_nested_mp4() -> TestResult {
        let dir = TempDir::new()?;
        let nested = dir.path().join("subdir");
        std::fs::create_dir(&nested)?;
        std::fs::write(nested.join("clip.mp4"), b"fake")?;
        let err = check_input_dir(dir.path(), false)
            .err()
            .ok_or("expected error")?;
        if !matches!(err, PreflightError::NoClipsFound { .. }) {
            return Err(format!("wrong variant: {err:?}").into());
        }
        Ok(())
    }
}
