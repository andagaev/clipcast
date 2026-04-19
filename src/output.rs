//! Structured stdout output for agent consumers.
//!
//! Every `clipcast` command emits exactly one JSON object on stdout when
//! `--json` is in effect (or when stdout is not a TTY). Stderr is reserved
//! for human-readable progress.

use serde::Serialize;
use std::io::IsTerminal;

/// Successful command result. `data` is command-specific JSON.
#[derive(Debug, Serialize)]
pub(crate) struct SuccessEnvelope {
    pub(crate) data: serde_json::Value,
    /// What command the agent should run next, or `null` if pipeline complete.
    pub(crate) next_action: Option<String>,
    pub(crate) next_action_reason: Option<String>,
}

impl SuccessEnvelope {
    pub(crate) fn new(data: serde_json::Value) -> Self {
        Self {
            data,
            next_action: None,
            next_action_reason: None,
        }
    }

    pub(crate) fn with_next_action(mut self, cmd: impl Into<String>) -> Self {
        self.next_action = Some(cmd.into());
        self
    }

    pub(crate) fn with_next_action_reason(mut self, reason: impl Into<String>) -> Self {
        self.next_action_reason = Some(reason.into());
        self
    }
}

/// Stable error envelope. `code` strings are documented in the spec and
/// stable across versions.
#[derive(Debug, Serialize)]
pub(crate) struct ErrorEnvelope {
    pub(crate) error: ErrorBody,
}

#[derive(Debug, Serialize)]
pub(crate) struct ErrorBody {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) fix: String,
}

impl ErrorEnvelope {
    pub(crate) fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self {
            error: ErrorBody {
                code: code.into(),
                message: message.into(),
                fix: fix.into(),
            },
        }
    }
}

/// Returns `true` when agent-style structured output should be used:
/// either the user passed `--json`, or stdout is not a TTY.
pub(crate) fn want_json(json_flag: bool) -> bool {
    json_flag || !std::io::stdout().is_terminal()
}

/// Print a success envelope to stdout as a single JSON line.
pub(crate) fn print_success(env: &SuccessEnvelope) -> Result<(), serde_json::Error> {
    let s = serde_json::to_string(env)?;
    println!("{s}");
    Ok(())
}

/// Print an error envelope to stdout as a single JSON line.
pub(crate) fn print_error(env: &ErrorEnvelope) -> Result<(), serde_json::Error> {
    let s = serde_json::to_string(env)?;
    println!("{s}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn success_envelope_serializes_data_and_next_action() -> TestResult {
        let env = SuccessEnvelope::new(serde_json::json!({"foo": "bar"}))
            .with_next_action("clipcast plan ./footage --brief '...'")
            .with_next_action_reason("decisions.json present, plan missing");
        let s = serde_json::to_string(&env)?;
        let v: serde_json::Value = serde_json::from_str(&s)?;
        assert_eq!(v["data"]["foo"], "bar");
        assert_eq!(
            v["next_action"].as_str().unwrap_or(""),
            "clipcast plan ./footage --brief '...'"
        );
        assert_eq!(
            v["next_action_reason"].as_str().unwrap_or(""),
            "decisions.json present, plan missing"
        );
        Ok(())
    }

    #[test]
    fn success_envelope_omits_next_action_when_unset() -> TestResult {
        let env = SuccessEnvelope::new(serde_json::json!({"output_path": "vlog.mp4"}));
        let s = serde_json::to_string(&env)?;
        let v: serde_json::Value = serde_json::from_str(&s)?;
        assert!(v["next_action"].is_null());
        assert!(v["next_action_reason"].is_null());
        Ok(())
    }

    #[test]
    fn error_envelope_has_code_message_fix() -> TestResult {
        let env = ErrorEnvelope::new(
            "missing_decisions",
            "decisions.json not found",
            "run `clipcast analyze <dir>` first",
        );
        let s = serde_json::to_string(&env)?;
        let v: serde_json::Value = serde_json::from_str(&s)?;
        assert_eq!(v["error"]["code"], "missing_decisions");
        assert_eq!(v["error"]["message"], "decisions.json not found");
        assert!(v["error"]["fix"]
            .as_str()
            .unwrap_or("")
            .contains("clipcast analyze"));
        Ok(())
    }

    #[test]
    fn want_json_honors_explicit_flag() {
        // Explicit flag = true should always return true regardless of TTY state.
        assert!(want_json(true));
    }

    #[test]
    fn print_helpers_emit_to_stdout_without_panic() -> TestResult {
        let success = SuccessEnvelope::new(serde_json::json!({"k": 1})).with_next_action("noop");
        print_success(&success)?;

        let error = ErrorEnvelope::new("test_code", "test message", "test fix");
        print_error(&error)?;
        Ok(())
    }
}
