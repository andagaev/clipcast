//! `clipcast schema [decisions|plan]` — print the JSON schema.
//!
//! Schemas are compiled into the binary from golden files under
//! `tests/fixtures/golden/`. This lets an agent self-discover the
//! current shape without hitting the network.

use anyhow::{anyhow, Result};

const PLAN_SCHEMA_JSON: &str = include_str!("../../tests/fixtures/golden/plan-schema.json");
const DECISIONS_SCHEMA_JSON: &str =
    include_str!("../../tests/fixtures/golden/decisions-schema.json");

/// Print the JSON schema for the named sidecar.
pub(crate) fn run(kind: &str) -> Result<()> {
    let s = match kind {
        "plan" => PLAN_SCHEMA_JSON,
        "decisions" => DECISIONS_SCHEMA_JSON,
        other => return Err(anyhow!("unknown schema '{other}'; valid: plan | decisions")),
    };
    println!("{s}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn plan_schema_is_valid_json() -> TestResult {
        let v: serde_json::Value = serde_json::from_str(PLAN_SCHEMA_JSON)?;
        assert_eq!(v["title"], "clipcast plan.json");
        Ok(())
    }

    #[test]
    fn decisions_schema_is_valid_json() -> TestResult {
        let v: serde_json::Value = serde_json::from_str(DECISIONS_SCHEMA_JSON)?;
        assert_eq!(v["title"], "clipcast decisions.json");
        Ok(())
    }

    #[test]
    fn run_rejects_unknown_kind() -> TestResult {
        let err = run("unknown").err().ok_or("should error")?;
        assert!(err.to_string().contains("unknown schema"));
        Ok(())
    }
}
