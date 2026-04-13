//! Prompt profiles for the LLM scoring stage.
//!
//! Each profile is a scoring rubric loaded via `include_str!` from the
//! `prompts/` directory at the crate root. Adding a new profile means:
//! 1. create `prompts/<name>.md`
//! 2. add a `const <NAME>: &str = include_str!(...)` line below
//! 3. add an arm to `resolve()`

/// Errors from profile resolution.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PromptError {
    #[error("unknown prompt profile `{0}`. Available: default, adventure, family")]
    Unknown(String),
}

const DEFAULT: &str = include_str!("../prompts/default.md");
const ADVENTURE: &str = include_str!("../prompts/adventure.md");
const FAMILY: &str = include_str!("../prompts/family.md");

/// Return the rubric body for a named profile.
pub(crate) fn resolve(name: &str) -> Result<&'static str, PromptError> {
    match name {
        "default" => Ok(DEFAULT),
        "adventure" => Ok(ADVENTURE),
        "family" => Ok(FAMILY),
        other => Err(PromptError::Unknown(other.to_string())),
    }
}

#[cfg(test)]
const NAMES: &[&str] = &["default", "adventure", "family"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_names_resolve() {
        for name in NAMES {
            assert!(resolve(name).is_ok(), "profile `{name}` did not resolve");
        }
    }

    #[test]
    fn unknown_profile_errors() {
        assert!(matches!(resolve("gaming"), Err(PromptError::Unknown(_))));
    }

    #[test]
    fn default_profile_mentions_scoring() -> Result<(), Box<dyn std::error::Error>> {
        let body = resolve("default")?;
        assert!(body.contains("1 to 10"));
        Ok(())
    }
}
