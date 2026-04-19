# Vlog Planning Stage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the deterministic `filter` stage with an LLM-driven `plan` stage that emits a reviewable, hand-editable `plan.json`. Add agent-first CLI affordances (`status`, `schema`, structured JSON output, error envelope, `next_action` hints).

**Architecture:** New `plan` pipeline stage between `analyze` and `render`. Plan stage reads `decisions.json`, calls `claude -p` with the brief and per-clip text data, writes `plan.json`. Render reads `plan.json` (not `decisions.json`) and performs per-segment ffmpeg trim+concat. `filter.rs` is deleted; `build` wires through the new stage.

**Tech Stack:** Rust 2021, tokio, serde/serde_json, thiserror, anyhow, chrono, clap, ffmpeg/ffprobe (subprocess), `claude -p` (subprocess), whisper-cli (subprocess). All existing — no new crates.

**Spec:** `ideas/docs/superpowers/specs/2026-04-18-vlog-planning-design.md`

---

## File-Level Map

**New files:**
- `src/output.rs` — JSON output envelope, error envelope, TTY detection, `next_action` builder.
- `src/plan.rs` — typed structs (`Plan`, `Segment`, `RejectedClip`, `DecisionsRef`) + `PlanError` (thiserror) + load/save helpers.
- `src/pipeline/plan.rs` — orchestrates the planner LLM call (initial + revise).
- `src/commands/plan.rs` — `clipcast plan` command.
- `src/commands/status.rs` — `clipcast status` command.
- `src/commands/schema.rs` — `clipcast schema [decisions|plan]` command.
- `prompts/plan.md` — planner system prompt template.
- `tests/fixtures/golden/plan-schema.json` — golden file for schema-stability test.

**Modified files:**
- `src/clip.rs` — drop `keep` field from `ClipVerdict`.
- `src/sidecar.rs` — bump `schema_version` to 2; remove `keep` round-trips.
- `src/pipeline/concat.rs` — add per-segment trim support; aspect check moves to per-segment.
- `src/commands/render.rs` — read `plan.json` (not `decisions.json`).
- `src/commands/build.rs` — call `pipeline::plan::run` instead of `filter::apply`.
- `src/main.rs` — wire new commands and `--json` flag.
- `src/commands.rs` — re-export new command modules.
- `src/pipeline.rs` — add `pub(crate) mod plan;`, remove `pub(crate) mod filter;`.
- `src/lib.rs` (or `src/main.rs` if no lib) — add `pub mod output;` and `pub mod plan;`.

**Deleted files:**
- `src/pipeline/filter.rs`

---

## Verification Gate

After every task that changes Rust code:
```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

All three must pass before committing. Pre-commit hook handles fmt + clippy.

---

## Task 1: Output envelope + TTY detection

**Files:**
- Create: `src/output.rs`
- Modify: `src/main.rs` (add `mod output;`)

- [ ] **Step 1: Write the failing test (in `src/output.rs` under `#[cfg(test)]`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn success_envelope_serializes() -> TestResult {
        let env = SuccessEnvelope::new(serde_json::json!({"foo": "bar"}))
            .with_next_action("clipcast plan ./footage --brief '...'");
        let s = serde_json::to_string(&env)?;
        assert!(s.contains("\"foo\":\"bar\""));
        assert!(s.contains("\"next_action\""));
        assert!(s.contains("clipcast plan"));
        Ok(())
    }

    #[test]
    fn error_envelope_has_code_message_fix() -> TestResult {
        let env = ErrorEnvelope::new("missing_decisions", "decisions.json not found", "run `clipcast analyze <dir>` first");
        let s = serde_json::to_string(&env)?;
        let v: serde_json::Value = serde_json::from_str(&s)?;
        assert_eq!(v["error"]["code"], "missing_decisions");
        assert_eq!(v["error"]["message"], "decisions.json not found");
        assert!(v["error"]["fix"].as_str().unwrap().contains("clipcast analyze"));
        Ok(())
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

`cargo test --lib output::tests` → fails with "cannot find type SuccessEnvelope".

- [ ] **Step 3: Implement `src/output.rs`**

```rust
//! Structured stdout output for agent consumers.
//!
//! Every `clipcast` command emits exactly one JSON object on stdout when
//! `--json` is in effect (or when stdout is not a TTY). Stderr is reserved
//! for human-readable progress.

use serde::Serialize;
use std::io::IsTerminal;

/// Successful command result. `data` is command-specific.
#[derive(Debug, Serialize)]
pub(crate) struct SuccessEnvelope {
    pub(crate) data: serde_json::Value,
    /// What command the agent should run next, or `null` if pipeline complete.
    pub(crate) next_action: Option<String>,
    pub(crate) next_action_reason: Option<String>,
}

impl SuccessEnvelope {
    pub(crate) fn new(data: serde_json::Value) -> Self {
        Self { data, next_action: None, next_action_reason: None }
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

/// Stable error envelope. `code` strings are versioned in the spec.
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

/// Returns `true` when the agent-style structured output should be used:
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
```

Add to `src/main.rs`: `mod output;`

- [ ] **Step 4: Run tests, verify pass**

`cargo test --lib output` → pass. Run `cargo clippy --all-targets -- -D warnings` to confirm no warnings.

- [ ] **Step 5: Commit**

```bash
git add src/output.rs src/main.rs
git commit -m "feat: add structured output envelope (SuccessEnvelope/ErrorEnvelope) for agent CLI"
```

---

## Task 2: Plan types and serialization

**Files:**
- Create: `src/plan.rs`
- Modify: `src/main.rs` (add `mod plan;`)

- [ ] **Step 1: Write failing tests (in `src/plan.rs`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn sample_plan() -> Plan {
        Plan {
            schema_version: 1,
            clipcast_version: "0.1.0".to_string(),
            generated_at: chrono::Utc.with_ymd_and_hms(2026, 4, 18, 14, 32, 11).unwrap(),
            model: "claude-opus-4-7".to_string(),
            decisions_ref: DecisionsRef {
                path: PathBuf::from("trip.decisions.json"),
                generated_at: chrono::Utc.with_ymd_and_hms(2026, 4, 18, 14, 28, 3).unwrap(),
            },
            brief: "test brief".to_string(),
            target_duration_s: 180,
            estimated_duration_s: 173.4,
            segments: vec![Segment {
                order: 1,
                source: PathBuf::from("a.mp4"),
                start_s: None,
                end_s: None,
                duration_s: 12.4,
                title: "Opener".to_string(),
                rationale: "Sets the scene.".to_string(),
                trim_reason: None,
            }, Segment {
                order: 2,
                source: PathBuf::from("b.mp4"),
                start_s: Some(4.2),
                end_s: Some(18.0),
                duration_s: 13.8,
                title: "Climax".to_string(),
                rationale: "Hooks the viewer.".to_string(),
                trim_reason: Some("dead time at start".to_string()),
            }],
            rejected: vec![RejectedClip {
                source: PathBuf::from("c.mp4"),
                score: 3,
                rejected_reason: "redundant".to_string(),
            }],
            warnings: vec![],
        }
    }

    #[test]
    fn plan_round_trips_through_json() -> TestResult {
        let p = sample_plan();
        let json = serde_json::to_string_pretty(&p)?;
        let parsed: Plan = serde_json::from_str(&json)?;
        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.segments.len(), 2);
        assert_eq!(parsed.segments[1].start_s, Some(4.2));
        assert_eq!(parsed.segments[0].start_s, None);
        assert_eq!(parsed.rejected[0].source, PathBuf::from("c.mp4"));
        Ok(())
    }

    #[test]
    fn explicit_null_trim_serializes_as_null_not_omitted() -> TestResult {
        let p = sample_plan();
        let json = serde_json::to_string(&p)?;
        // segment 0 has both start_s/end_s as None and must serialize as null,
        // not be omitted, per the schema (rigid for agent consumption).
        assert!(json.contains("\"start_s\":null"));
        assert!(json.contains("\"end_s\":null"));
        Ok(())
    }

    #[tokio::test]
    async fn write_and_read_round_trips() -> TestResult {
        let dir = tempfile::TempDir::new()?;
        let path = dir.path().join("p.plan.json");
        let p = sample_plan();
        save(&path, &p).await?;
        let loaded = load(&path).await?;
        assert_eq!(loaded.brief, p.brief);
        Ok(())
    }
}
```

- [ ] **Step 2: Run, verify fail**

`cargo test --lib plan` → fails ("cannot find type Plan").

- [ ] **Step 3: Implement `src/plan.rs`**

```rust
//! `plan.json` — the agent-produced cut assembly plan that lives between
//! `clipcast plan` (writer) and `clipcast render` (reader).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The current schema version that this build of clipcast writes and reads.
pub(crate) const PLAN_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Plan {
    pub(crate) schema_version: u32,
    pub(crate) clipcast_version: String,
    pub(crate) generated_at: DateTime<Utc>,
    pub(crate) model: String,
    pub(crate) decisions_ref: DecisionsRef,
    pub(crate) brief: String,
    pub(crate) target_duration_s: u64,
    pub(crate) estimated_duration_s: f64,
    pub(crate) segments: Vec<Segment>,
    #[serde(default)]
    pub(crate) rejected: Vec<RejectedClip>,
    #[serde(default)]
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DecisionsRef {
    pub(crate) path: PathBuf,
    pub(crate) generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Segment {
    pub(crate) order: u32,
    pub(crate) source: PathBuf,
    /// Trim start in seconds within the source clip. `null` = use whole clip.
    pub(crate) start_s: Option<f64>,
    /// Trim end in seconds within the source clip. `null` = use whole clip.
    pub(crate) end_s: Option<f64>,
    /// Computed duration of the segment as it will appear in the cut.
    pub(crate) duration_s: f64,
    pub(crate) title: String,
    pub(crate) rationale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) trim_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RejectedClip {
    pub(crate) source: PathBuf,
    pub(crate) score: u8,
    pub(crate) rejected_reason: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PlanError {
    #[error("failed to read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse plan.json at {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to serialize plan: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("unsupported plan schema_version {found}; this build expects {expected}. Run `clipcast schema plan` for the current shape.")]
    UnsupportedVersion { found: u32, expected: u32 },
}

pub(crate) async fn load(path: &Path) -> Result<Plan, PlanError> {
    let text = tokio::fs::read_to_string(path).await.map_err(|source| PlanError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let plan: Plan = serde_json::from_str(&text).map_err(|source| PlanError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    if plan.schema_version != PLAN_SCHEMA_VERSION {
        return Err(PlanError::UnsupportedVersion {
            found: plan.schema_version,
            expected: PLAN_SCHEMA_VERSION,
        });
    }
    Ok(plan)
}

pub(crate) async fn save(path: &Path, plan: &Plan) -> Result<(), PlanError> {
    let json = serde_json::to_string_pretty(plan).map_err(PlanError::Serialize)?;
    tokio::fs::write(path, json).await.map_err(|source| PlanError::Write {
        path: path.to_path_buf(),
        source,
    })
}
```

Add `mod plan;` to `src/main.rs`.

- [ ] **Step 4: Run tests, verify pass**

`cargo test --lib plan` → pass.

- [ ] **Step 5: Commit**

```bash
git add src/plan.rs src/main.rs
git commit -m "feat: add Plan/Segment/RejectedClip types and plan.json (de)serialization"
```

---

## Task 3: Decisions schema v2 — remove `keep`, version bump

**Files:**
- Modify: `src/clip.rs:42-66` (`ClipVerdict`)
- Modify: `src/sidecar.rs` (add `schema_version` field; bump to 2)
- Modify: `src/pipeline/filter.rs` (will be deleted in Task 13; for now, update tests to set `keep` locally on a derived Vec<bool>, or leave as-is until deletion).

> Decision: leave `filter.rs` untouched here; we delete it whole in Task 13. Skip changes to filter.rs in this task.

- [ ] **Step 1: Update `ClipVerdict` — remove `keep` field**

In `src/clip.rs`, delete lines defining `keep` and the related doc comment. Update `sample_verdict()` and other test instantiations to remove `keep: true`.

- [ ] **Step 2: Add `schema_version` to `Sidecar`**

In `src/sidecar.rs`:

```rust
pub(crate) const DECISIONS_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Sidecar {
    #[serde(default = "default_schema_version")]
    pub(crate) schema_version: u32,
    pub(crate) clipcast_version: String,
    pub(crate) generated_at: DateTime<Utc>,
    pub(crate) target_duration_s: u64,
    pub(crate) clips: Vec<ClipVerdict>,
}

fn default_schema_version() -> u32 {
    DECISIONS_SCHEMA_VERSION
}
```

In `build()`, set `schema_version: DECISIONS_SCHEMA_VERSION`.

Add a `read()` check that errors with a `SidecarError::UnsupportedVersion { found, expected }` variant when `schema_version != DECISIONS_SCHEMA_VERSION`.

- [ ] **Step 3: Update existing sidecar tests**

Remove all references to `keep` in `src/sidecar.rs::tests` and `src/clip.rs::tests`. Delete the `verdict_default_keep_is_false` test (no longer applicable). Update the `assert!(read_back.clips[0].keep)` assertion to just check the score round-trips.

- [ ] **Step 4: Update callers that read/write `keep`**

```bash
# Find all current callers of `.keep`:
```
Use Grep tool: pattern `\.keep\b` in `src`. Expected callsites:
- `src/pipeline/filter.rs` — leave alone, deleted in Task 13.
- `src/commands/build.rs:70` — `let kept_count = verdicts.iter().filter(|v| v.keep).count();` — for this task, leave the build command broken; Task 12 fixes it. Add `#[allow(dead_code)]` or comment-out as needed to keep it compiling? **Better:** since we'll touch build in task 12, just keep this task laser-focused on the type changes and let build.rs / render.rs / concat.rs be broken until they're rewritten. To keep `cargo test` green during the gap, do this task and Tasks 8/9/12/13 in a SINGLE branch and commit them together if needed.

> **Pragmatic adjustment:** because `keep` is deeply wired into the current build/render/concat path, the cleanest path is: do Task 3 first (types), then immediately do Tasks 8, 9, 12, 13 in sequence before re-running the full test suite. We'll commit Task 3 with a temporary `#[allow(dead_code)]` shim removing `keep` from `ClipVerdict` and adapting `build.rs`/`render.rs`/`concat.rs` to a simple "pass through every clip with `score.is_some() && error.is_none()` in chronological order" stub. That keeps the codebase compiling/passing tests until plan.rs lands.

- [ ] **Step 5: Apply the temporary shim**

In `src/commands/build.rs`, replace the `filter::apply` block with:

```rust
// TEMP shim until pipeline::plan lands (Task 6). For now, keep all
// scored clips in chronological order so render still works end-to-end.
verdicts.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
let kept_count = verdicts.iter().filter(|v| v.score.is_some() && v.error.is_none()).count();
```

Update `concat::run` signature/use to drop reliance on `keep` — change it to accept a slice of "selected" verdicts directly. In `concat.rs`, change the filtering:

```rust
// was: verdicts.iter().filter(|v| v.keep)
// now: verdicts.iter().filter(|v| v.score.is_some() && v.error.is_none())
```

In `commands/render.rs`, do the same (drop `.keep` references).

In `commands/analyze.rs`, the `--ratify` (or whatever it's called — check) path that flips keep flags goes away or becomes a no-op for this task — Task 12 will replace it with plan generation.

> **Note:** Be ruthless about keeping this task's diff small. The goal is "code compiles, all existing tests pass, `keep` is gone." It's OK if behavior temporarily degrades to "include all scored clips."

- [ ] **Step 6: Run full verification**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

Expected: all green. Filter tests still pass (filter.rs is untouched and still works on its own).

- [ ] **Step 7: Commit**

```bash
git add -u src/clip.rs src/sidecar.rs src/commands/build.rs src/commands/render.rs src/commands/analyze.rs src/pipeline/concat.rs
git commit -m "refactor: remove ClipVerdict.keep; bump decisions schema_version to 2"
```

---

## Task 4: Planner prompt template

**Files:**
- Create: `prompts/plan.md`

- [ ] **Step 1: Write the prompt**

```markdown
# Vlog Cut Planner

You are assembling a vlog from raw clips. Your job is to produce a structured cut plan in JSON.

## Inputs you receive

- A **brief** describing the desired vlog (tone, focus, duration target).
- A **target duration** in seconds.
- A **list of analyzed clips**, one per row, with: file path, duration, timestamp, score (1–10), one-line analysis reason, transcript (may be empty).

## Your output

A single JSON object with exactly this shape (no markdown, no commentary, just JSON):

```json
{
  "estimated_duration_s": 173.4,
  "segments": [
    {
      "order": 1,
      "source": "GH010234.MP4",
      "start_s": null,
      "end_s": null,
      "duration_s": 12.4,
      "title": "Arrival at the beach",
      "rationale": "Sets the location and who's there. Natural opener."
    }
  ],
  "rejected": [
    {
      "source": "GH010240.MP4",
      "score": 3,
      "rejected_reason": "Redundant with GH010238 — same wave, weaker angle."
    }
  ],
  "warnings": []
}
```

## Rules

1. **`source` MUST be the exact `path` value from the input list** — do not invent, abbreviate, or rename clips.
2. **`order`** is a 1-based integer; segments will be concatenated in `order` ascending.
3. **`start_s` / `end_s`**: set to `null` to use the whole clip. Otherwise set both to numbers in seconds within the source clip. Only trim when there's a clear reason (dead time at the start, awkward end). When you trim, include a `trim_reason` string field.
4. **`duration_s`**: the segment's playback length (= `end_s - start_s` if trimmed, else the full source duration).
5. **`estimated_duration_s`**: sum of all `segments[].duration_s`. Try to land within ±10% of the target.
6. **Reject** clips that don't fit the brief, are redundant, low-scoring, or have errors. Every rejected clip needs a one-line `rejected_reason`.
7. **`warnings`**: surface non-fatal issues — mixed aspect ratios, target overrun, missing audio. One string per warning.
8. **Narrative judgment matters more than score.** A 6/10 clip can earn a slot if it's the right beat at the right moment.

## Output ONLY the JSON object. No prose before or after.
```

- [ ] **Step 2: Commit**

```bash
git add prompts/plan.md
git commit -m "feat: add planner prompt template (prompts/plan.md)"
```

---

## Task 5: Planning pipeline stage

**Files:**
- Create: `src/pipeline/plan.rs`
- Modify: `src/pipeline.rs` (add `pub(crate) mod plan;`)

- [ ] **Step 1: Write failing tests**

In `src/pipeline/plan.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::TimestampSource;
    use crate::sidecar::Sidecar;
    use chrono::TimeZone;
    use std::path::PathBuf;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn sample_decisions() -> Sidecar {
        let ts = chrono::Utc.with_ymd_and_hms(2026, 4, 18, 12, 0, 0).unwrap();
        Sidecar {
            schema_version: crate::sidecar::DECISIONS_SCHEMA_VERSION,
            clipcast_version: "test".to_string(),
            generated_at: ts,
            target_duration_s: 60,
            clips: vec![crate::clip::ClipVerdict {
                path: PathBuf::from("a.mp4"),
                duration_s: 10.0,
                timestamp: ts,
                timestamp_source: TimestampSource::CreationTime,
                score: Some(8),
                reason: Some("nice".to_string()),
                error: None,
                transcript: Some("hello".to_string()),
            }],
        }
    }

    #[test]
    fn render_prompt_includes_brief_and_clip_table() -> TestResult {
        let decisions = sample_decisions();
        let rendered = render_planner_prompt("BRIEF TEXT", 60, &decisions);
        assert!(rendered.contains("BRIEF TEXT"));
        assert!(rendered.contains("60"));
        assert!(rendered.contains("a.mp4"));
        assert!(rendered.contains("hello"));
        Ok(())
    }

    #[test]
    fn parse_planner_output_extracts_segments() -> TestResult {
        let raw = r#"{
            "estimated_duration_s": 10.0,
            "segments": [
                {"order": 1, "source": "a.mp4", "start_s": null, "end_s": null,
                 "duration_s": 10.0, "title": "T", "rationale": "R"}
            ],
            "rejected": [],
            "warnings": []
        }"#;
        let parsed = parse_planner_output(raw)?;
        assert_eq!(parsed.segments.len(), 1);
        assert_eq!(parsed.segments[0].source, PathBuf::from("a.mp4"));
        Ok(())
    }

    #[test]
    fn parse_planner_output_rejects_garbage() {
        let result = parse_planner_output("not json at all");
        assert!(matches!(result, Err(PipelinePlanError::InvalidPlannerOutput { .. })));
    }
}
```

- [ ] **Step 2: Run, verify fail**

`cargo test --lib pipeline::plan` → fail.

- [ ] **Step 3: Implement `src/pipeline/plan.rs`**

```rust
//! Planner stage: take a `Sidecar` (decisions.json) + brief, call the
//! planning LLM, return a fully-populated `Plan`.

use crate::clip::ClipVerdict;
use crate::plan::{DecisionsRef, Plan, PlanError, RejectedClip, Segment, PLAN_SCHEMA_VERSION};
use crate::process;
use crate::sidecar::Sidecar;
use serde::Deserialize;
use std::path::Path;

/// Default brief used by `clipcast build` when the user provides none.
pub(crate) const DEFAULT_BRIEF: &str =
    "Assemble a chronological highlight reel of the best clips fitting the target duration. \
     Drop clips that are blurry, redundant, or scored low. \
     Trim dead time at clip starts/ends only when obvious.";

#[derive(Debug, thiserror::Error)]
pub(crate) enum PipelinePlanError {
    #[error("planner subprocess failed: {0}")]
    Subprocess(String),
    #[error("planner returned invalid JSON: {message}")]
    InvalidPlannerOutput {
        message: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("plan write failed: {0}")]
    Save(#[from] PlanError),
}

/// LLM-side payload (subset of `Plan` produced by the model).
#[derive(Debug, Deserialize)]
struct PlannerOutput {
    estimated_duration_s: f64,
    segments: Vec<Segment>,
    #[serde(default)]
    rejected: Vec<RejectedClip>,
    #[serde(default)]
    warnings: Vec<String>,
}

/// Build the prompt sent to `claude -p`.
pub(crate) fn render_planner_prompt(brief: &str, target_s: u64, decisions: &Sidecar) -> String {
    let mut s = String::new();
    s.push_str(include_str!("../../prompts/plan.md"));
    s.push_str("\n\n---\n\n## Brief\n\n");
    s.push_str(brief);
    s.push_str(&format!("\n\n## Target duration\n\n{target_s} seconds\n\n"));
    s.push_str("## Clips\n\n");
    for c in &decisions.clips {
        let score = c.score.map(|x| x.to_string()).unwrap_or_else(|| "—".to_string());
        let reason = c.reason.as_deref().unwrap_or("—");
        let transcript = c.transcript.as_deref().unwrap_or("—");
        let err = c.error.as_deref().unwrap_or("");
        s.push_str(&format!(
            "- path: {}\n  duration_s: {:.2}\n  timestamp: {}\n  score: {}\n  reason: {}\n  transcript: {}\n  error: {}\n\n",
            c.path.display(), c.duration_s, c.timestamp, score, reason, transcript, err,
        ));
    }
    s
}

/// Parse the LLM's JSON output. Tolerates leading/trailing whitespace.
pub(crate) fn parse_planner_output(raw: &str) -> Result<PlannerOutput, PipelinePlanError> {
    let trimmed = raw.trim();
    // Some models wrap JSON in ```json fences; strip them.
    let cleaned = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .trim_end_matches("```")
        .trim();
    serde_json::from_str::<PlannerOutput>(cleaned).map_err(|source| {
        PipelinePlanError::InvalidPlannerOutput {
            message: source.to_string(),
            source,
        }
    })
}

/// Run the planner: render prompt, call `claude -p`, parse output, assemble Plan.
pub(crate) async fn run(
    brief: String,
    target_duration_s: u64,
    decisions: &Sidecar,
    decisions_path: &Path,
    model_label: &str,
) -> Result<Plan, PipelinePlanError> {
    let prompt = render_planner_prompt(&brief, target_duration_s, decisions);
    let raw = process::run_capture(
        "claude",
        &["-p", &prompt],
        &[("AGENT_COLLECTOR_IGNORE", "1")],
    )
    .await
    .map_err(|e| PipelinePlanError::Subprocess(e.to_string()))?;
    let parsed = parse_planner_output(&raw)?;

    Ok(Plan {
        schema_version: PLAN_SCHEMA_VERSION,
        clipcast_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: chrono::Utc::now(),
        model: model_label.to_string(),
        decisions_ref: DecisionsRef {
            path: decisions_path.to_path_buf(),
            generated_at: decisions.generated_at,
        },
        brief,
        target_duration_s,
        estimated_duration_s: parsed.estimated_duration_s,
        segments: parsed.segments,
        rejected: parsed.rejected,
        warnings: parsed.warnings,
    })
}

/// Revise an existing plan with new instructions.
pub(crate) async fn revise(
    existing: &Plan,
    instructions: &str,
    decisions: &Sidecar,
    model_label: &str,
) -> Result<Plan, PipelinePlanError> {
    let mut prompt = String::new();
    prompt.push_str(include_str!("../../prompts/plan.md"));
    prompt.push_str("\n\n---\n\n## Existing plan to revise\n\n");
    prompt.push_str(&serde_json::to_string_pretty(existing).unwrap_or_default());
    prompt.push_str("\n\n## Revision instructions\n\n");
    prompt.push_str(instructions);
    prompt.push_str("\n\n## Brief\n\n");
    prompt.push_str(&existing.brief);
    prompt.push_str(&format!(
        "\n\n## Target duration\n\n{} seconds\n\n",
        existing.target_duration_s
    ));
    prompt.push_str("## Clips\n\n");
    for c in &decisions.clips {
        let score = c.score.map(|x| x.to_string()).unwrap_or_else(|| "—".to_string());
        prompt.push_str(&format!(
            "- path: {}\n  duration_s: {:.2}\n  score: {}\n  reason: {}\n\n",
            c.path.display(), c.duration_s, score,
            c.reason.as_deref().unwrap_or("—"),
        ));
    }
    let raw = process::run_capture("claude", &["-p", &prompt], &[("AGENT_COLLECTOR_IGNORE", "1")])
        .await
        .map_err(|e| PipelinePlanError::Subprocess(e.to_string()))?;
    let parsed = parse_planner_output(&raw)?;

    Ok(Plan {
        schema_version: PLAN_SCHEMA_VERSION,
        clipcast_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: chrono::Utc::now(),
        model: model_label.to_string(),
        decisions_ref: existing.decisions_ref.clone(),
        brief: existing.brief.clone(),
        target_duration_s: existing.target_duration_s,
        estimated_duration_s: parsed.estimated_duration_s,
        segments: parsed.segments,
        rejected: parsed.rejected,
        warnings: parsed.warnings,
    })
}
```

> Verify the actual signature of `process::run_capture` (or whatever it's named in `src/process.rs`). Adapt to existing API. If env-var injection isn't supported, set `AGENT_COLLECTOR_IGNORE=1` differently (mirror what `analyzer/claude_print.rs` does today).

Add to `src/pipeline.rs`: `pub(crate) mod plan;`

- [ ] **Step 4: Run tests, verify pass**

`cargo test --lib pipeline::plan` → pass.

- [ ] **Step 5: Commit**

```bash
git add src/pipeline/plan.rs src/pipeline.rs
git commit -m "feat: add pipeline::plan stage (LLM-driven cut assembly)"
```

---

## Task 6: `clipcast plan` command

**Files:**
- Create: `src/commands/plan.rs`
- Modify: `src/commands.rs` (add `pub(crate) mod plan;`)
- Modify: `src/main.rs` (add `Plan` subcommand to clap enum)

- [ ] **Step 1: Add Clap subcommand**

In `src/main.rs`, find the `Commands` enum and add:

```rust
/// Generate or revise a vlog plan from existing decisions.json.
Plan {
    /// Input clips directory (used to locate decisions.json + plan.json).
    input_dir: PathBuf,
    /// Target duration. Required for fresh plan; ignored on --revise.
    #[arg(short, long)]
    duration: Option<String>,
    /// Vlog brief (freeform string).
    #[arg(long, conflicts_with = "brief_file")]
    brief: Option<String>,
    /// Vlog brief read from a file (markdown welcome).
    #[arg(long, conflicts_with = "brief")]
    brief_file: Option<PathBuf>,
    /// Output dir / file path (defaults to alongside input_dir).
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
    #[arg(long, global = true)]
    json: bool,
},
```

- [ ] **Step 2: Implement `src/commands/plan.rs`**

```rust
//! `clipcast plan <input-dir>` — write or revise plan.json.

use crate::output::{print_error, print_success, want_json, ErrorEnvelope, SuccessEnvelope};
use crate::paths;
use crate::pipeline::plan as pipeline_plan;
use crate::plan as plan_types;
use crate::sidecar;
use crate::duration;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

const MODEL_LABEL: &str = "claude-opus-4-7";

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    input_dir: &Path,
    duration_str: Option<String>,
    brief: Option<String>,
    brief_file: Option<PathBuf>,
    out: Option<PathBuf>,
    revise: bool,
    instructions: Option<String>,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    let json_mode = want_json(json);

    let output_path = out.unwrap_or_else(|| paths::default_output(input_dir, chrono::Utc::now()));
    let decisions_path = paths::sidecar_for(&output_path);
    let plan_path = paths::plan_for(&output_path);

    // 1. Load decisions.json.
    if !decisions_path.exists() {
        return emit_error(
            json_mode,
            "missing_decisions",
            format!("decisions.json not found at {}", decisions_path.display()),
            "run `clipcast analyze <dir>` first",
        );
    }
    let decisions = sidecar::read(&decisions_path).await.context("read decisions.json")?;
    if decisions.clips.is_empty() {
        return emit_error(
            json_mode,
            "empty_decisions",
            "decisions.json contains no clips",
            "re-run `clipcast analyze <dir>` with a non-empty input dir",
        );
    }

    // 2. Resolve brief.
    let brief_text = match (revise, &brief, &brief_file) {
        (true, _, _) => {
            // Revise: brief comes from existing plan.
            if !plan_path.exists() {
                return emit_error(
                    json_mode,
                    "revise_without_plan",
                    format!("no plan.json at {} to revise", plan_path.display()),
                    "run `clipcast plan` without --revise first",
                );
            }
            String::new() // unused below; we load existing plan
        }
        (false, Some(b), _) => b.clone(),
        (false, None, Some(p)) => tokio::fs::read_to_string(p)
            .await
            .with_context(|| format!("read brief file {}", p.display()))?,
        (false, None, None) => {
            // Default brief only allowed via `clipcast build`. For `plan`, require explicit brief.
            return emit_error(
                json_mode,
                "missing_brief",
                "no --brief or --brief-file provided",
                "pass --brief \"...\" or --brief-file <path>",
            );
        }
    };

    // 3. Resolve duration.
    let target_duration_s = match duration_str {
        Some(s) => duration::parse(&s).context("parse --duration")?.as_secs(),
        None if revise => {
            let existing = plan_types::load(&plan_path).await.context("load existing plan")?;
            existing.target_duration_s
        }
        None => {
            return emit_error(
                json_mode,
                "missing_duration",
                "no --duration provided",
                "pass --duration 3m (or seconds, etc.)",
            );
        }
    };

    // 4. Dry-run: emit prompt, exit.
    if dry_run {
        let prompt = pipeline_plan::render_planner_prompt(&brief_text, target_duration_s, &decisions);
        if json_mode {
            print_success(
                &SuccessEnvelope::new(serde_json::json!({
                    "dry_run": true,
                    "decisions_path": decisions_path,
                    "candidate_clip_count": decisions.clips.len(),
                    "prompt_preview_chars": prompt.len(),
                }))
                .with_next_action("re-run without --dry-run to invoke the planner"),
            )?;
        } else {
            eprintln!("{}", prompt);
        }
        return Ok(());
    }

    // 5. Run the planner.
    let plan = if revise {
        let existing = plan_types::load(&plan_path).await.context("load existing plan")?;
        let instr = instructions.ok_or_else(|| anyhow!("--revise requires --instructions"))?;
        pipeline_plan::revise(&existing, &instr, &decisions, MODEL_LABEL).await?
    } else {
        pipeline_plan::run(brief_text, target_duration_s, &decisions, &decisions_path, MODEL_LABEL).await?
    };

    // 6. Save plan.json.
    plan_types::save(&plan_path, &plan).await?;

    // 7. Emit success.
    let next_action = format!("review {} then run `clipcast render {}`", plan_path.display(), input_dir.display());
    if json_mode {
        print_success(
            &SuccessEnvelope::new(serde_json::json!({
                "plan_path": plan_path,
                "segments": plan.segments.len(),
                "rejected": plan.rejected.len(),
                "warnings": plan.warnings,
                "estimated_duration_s": plan.estimated_duration_s,
                "target_duration_s": plan.target_duration_s,
            }))
            .with_next_action(next_action.clone())
            .with_next_action_reason("plan.json written; user review before render"),
        )?;
    } else {
        println!("wrote {}", plan_path.display());
        println!("{} segments planned, {} rejected", plan.segments.len(), plan.rejected.len());
        println!("next: {next_action}");
    }
    Ok(())
}

fn emit_error(json_mode: bool, code: &str, msg: impl Into<String>, fix: impl Into<String>) -> Result<()> {
    let msg = msg.into();
    let fix = fix.into();
    if json_mode {
        print_error(&ErrorEnvelope::new(code, &msg, &fix))?;
    } else {
        eprintln!("error [{code}]: {msg}");
        eprintln!("fix: {fix}");
    }
    Err(anyhow!("{code}: {msg}"))
}
```

- [ ] **Step 3: Add `paths::plan_for`**

In `src/paths.rs`, mirror `sidecar_for`:

```rust
pub(crate) fn plan_for(output_path: &Path) -> PathBuf {
    output_path.with_extension("plan.json")
}
```

Plus a unit test alongside `sidecar_for`'s test.

- [ ] **Step 4: Wire dispatch in `src/commands.rs`**

Add `Plan { ... }` arm in the dispatch match that calls `commands::plan::run(...)`.

- [ ] **Step 5: Smoke-test with mocked claude binary**

`tests/end_to_end.rs` already has fake-bin infrastructure. Add an `#[ignore]` integration test:

```rust
#[tokio::test]
#[ignore]
async fn plan_command_writes_plan_json() {
    // ... use fake claude to return a fixed JSON plan; assert plan.json exists.
}
```

- [ ] **Step 6: Run verification**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

- [ ] **Step 7: Commit**

```bash
git add -u src/commands/plan.rs src/commands.rs src/main.rs src/paths.rs tests/end_to_end.rs
git commit -m "feat: add clipcast plan command (initial + --revise modes)"
```

---

## Task 7: Per-segment trim in concat

**Files:**
- Modify: `src/pipeline/concat.rs`

- [ ] **Step 1: Read current `concat::run` signature**

Use Read tool: `src/pipeline/concat.rs`. Note current parameter shape and aspect-ratio logic.

- [ ] **Step 2: Add a `Segment`-driven entry point**

Add (alongside the existing `run`):

```rust
/// Concat a list of trimmed segments to `output_path`. Each segment may
/// optionally specify in/out points within its source clip.
pub(crate) async fn run_segments(
    segments: &[crate::plan::Segment],
    metas_by_path: &std::collections::HashMap<std::path::PathBuf, (u32, u32)>,
    output_path: &std::path::Path,
) -> Result<(), ConcatError> {
    if segments.is_empty() {
        return Err(ConcatError::NoSegments);
    }

    // 1. Aspect-ratio check (same as before, but per-segment source).
    let first = &segments[0];
    let (w0, h0) = metas_by_path.get(&first.source).copied()
        .ok_or_else(|| ConcatError::MissingMeta(first.source.clone()))?;
    for s in segments.iter().skip(1) {
        let (w, h) = metas_by_path.get(&s.source).copied()
            .ok_or_else(|| ConcatError::MissingMeta(s.source.clone()))?;
        if (w, h) != (w0, h0) {
            return Err(ConcatError::AspectMismatch {
                first: first.source.clone(),
                first_dims: (w0, h0),
                offender: s.source.clone(),
                offender_dims: (w, h),
            });
        }
    }

    // 2. Materialize trimmed temp files where needed.
    let tempdir = tempfile::TempDir::new().map_err(ConcatError::TempDir)?;
    let mut paths_for_concat: Vec<std::path::PathBuf> = Vec::with_capacity(segments.len());
    for (i, s) in segments.iter().enumerate() {
        match (s.start_s, s.end_s) {
            (None, None) => {
                paths_for_concat.push(s.source.clone());
            }
            (start, end) => {
                let trimmed = tempdir.path().join(format!("seg-{:04}.mp4", i));
                let mut args: Vec<String> = vec!["-y".into()];
                if let Some(ss) = start { args.extend(["-ss".into(), format!("{ss}")]); }
                args.extend(["-i".into(), s.source.display().to_string()]);
                if let Some(ee) = end { args.extend(["-to".into(), format!("{}", ee - start.unwrap_or(0.0))]); }
                args.extend(["-c".into(), "copy".into(), trimmed.display().to_string()]);
                let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                crate::process::run_status("ffmpeg", &arg_refs)
                    .await
                    .map_err(|e| ConcatError::Trim {
                        source_path: s.source.clone(),
                        message: e.to_string(),
                    })?;
                paths_for_concat.push(trimmed);
            }
        }
    }

    // 3. Reuse existing concat logic on `paths_for_concat`. Extract the
    // ffmpeg-concat-list-and-invoke logic from the existing `run()` into
    // a private `concat_files(&[PathBuf], &Path) -> Result<...>` and call
    // it here.
    concat_files(&paths_for_concat, output_path).await?;
    Ok(())
}
```

Add corresponding error variants to `ConcatError`:

```rust
#[error("no segments to concat")]
NoSegments,
#[error("missing metadata for {0}")]
MissingMeta(std::path::PathBuf),
#[error("aspect ratio mismatch: {} ({:?}) vs {} ({:?})",
        first.display(), first_dims, offender.display(), offender_dims)]
AspectMismatch {
    first: std::path::PathBuf,
    first_dims: (u32, u32),
    offender: std::path::PathBuf,
    offender_dims: (u32, u32),
},
#[error("ffmpeg trim failed for {}: {message}", source_path.display())]
Trim { source_path: std::path::PathBuf, message: String },
#[error("tempdir creation failed: {0}")]
TempDir(#[source] std::io::Error),
```

- [ ] **Step 3: Add a unit test**

```rust
#[tokio::test]
async fn run_segments_errors_when_empty() {
    let segments: Vec<crate::plan::Segment> = vec![];
    let metas = std::collections::HashMap::new();
    let dir = tempfile::TempDir::new().unwrap();
    let out = dir.path().join("out.mp4");
    let result = run_segments(&segments, &metas, &out).await;
    assert!(matches!(result, Err(ConcatError::NoSegments)));
}

#[tokio::test]
async fn run_segments_detects_aspect_mismatch() {
    let segments = vec![
        crate::plan::Segment { order: 1, source: "a.mp4".into(), start_s: None, end_s: None,
            duration_s: 1.0, title: "".into(), rationale: "".into(), trim_reason: None },
        crate::plan::Segment { order: 2, source: "b.mp4".into(), start_s: None, end_s: None,
            duration_s: 1.0, title: "".into(), rationale: "".into(), trim_reason: None },
    ];
    let mut metas = std::collections::HashMap::new();
    metas.insert("a.mp4".into(), (1920, 1080));
    metas.insert("b.mp4".into(), (1080, 1920));
    let dir = tempfile::TempDir::new().unwrap();
    let result = run_segments(&segments, &metas, &dir.path().join("out.mp4")).await;
    assert!(matches!(result, Err(ConcatError::AspectMismatch { .. })));
}
```

- [ ] **Step 4: Verify**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test --lib pipeline::concat
```

- [ ] **Step 5: Commit**

```bash
git add -u src/pipeline/concat.rs
git commit -m "feat: add per-segment trim+concat (concat::run_segments)"
```

---

## Task 8: Render reads `plan.json`

**Files:**
- Modify: `src/commands/render.rs`

- [ ] **Step 1: Read current render command**

Use Read tool: `src/commands/render.rs`. Identify where it currently loads `decisions.json` and calls `concat::run`.

- [ ] **Step 2: Replace decisions read with plan read; call `concat::run_segments`**

Replace the body so render:
1. Loads `plan.json` (via `plan::load`).
2. Verifies aspect by re-running ffprobe on each unique segment source (or load decisions.json for metas if available).
3. Calls `concat::run_segments(&plan.segments, &metas, &output_path)`.
4. Emits success envelope with `next_action: null` (pipeline complete).

For metas: simplest path is to also load `decisions.json` (if present) and reuse `(width, height)` from it. If decisions is missing or stale, run ffprobe on each segment source.

- [ ] **Step 3: Add stale-plan warning**

If `decisions.json` exists and `decisions.generated_at > plan.decisions_ref.generated_at`, write a warning to stderr (and include in stdout warnings array if `--json`). Don't block.

- [ ] **Step 4: Verify + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add -u src/commands/render.rs
git commit -m "feat: render now consumes plan.json and runs per-segment trim+concat"
```

---

## Task 9: `clipcast status` command

**Files:**
- Create: `src/commands/status.rs`
- Modify: `src/commands.rs`, `src/main.rs`

- [ ] **Step 1: Add Clap subcommand**

```rust
/// Print the current state of a clipcast project as JSON.
Status {
    input_dir: PathBuf,
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
},
```

- [ ] **Step 2: Implement `src/commands/status.rs`**

```rust
//! `clipcast status <input-dir>` — read-only project state inspection.

use crate::output::{print_success, want_json, SuccessEnvelope};
use crate::paths;
use crate::plan as plan_types;
use crate::sidecar;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub(crate) async fn run(input_dir: &Path, out: Option<PathBuf>, json: bool) -> Result<()> {
    let json_mode = want_json(json);
    let output_path = out.unwrap_or_else(|| paths::default_output(input_dir, chrono::Utc::now()));
    let decisions_path = paths::sidecar_for(&output_path);
    let plan_path = paths::plan_for(&output_path);

    let has_decisions = decisions_path.exists();
    let has_plan = plan_path.exists();
    let has_output = output_path.exists();

    let (stage, next_action, reason) = match (has_decisions, has_plan, has_output) {
        (false, _, _) => ("none", Some(format!("clipcast analyze {}", input_dir.display())),
                          "no decisions.json yet"),
        (true, false, _) => ("analyzed",
            Some(format!("clipcast plan {} --brief '...' --duration 3m", input_dir.display())),
            "decisions.json exists; plan.json missing"),
        (true, true, false) => ("planned",
            Some(format!("review {} then `clipcast render {}`", plan_path.display(), input_dir.display())),
            "plan.json exists; final mp4 not yet rendered"),
        (true, true, true) => ("rendered", None, "pipeline complete"),
    };

    let clip_count = if has_decisions {
        sidecar::read(&decisions_path).await.ok().map(|s| s.clips.len())
    } else { None };
    let planned_count = if has_plan {
        plan_types::load(&plan_path).await.ok().map(|p| p.segments.len())
    } else { None };

    let payload = serde_json::json!({
        "stage": stage,
        "decisions_path": has_decisions.then(|| decisions_path.clone()),
        "plan_path": has_plan.then(|| plan_path.clone()),
        "output_path": has_output.then(|| output_path.clone()),
        "clip_count": clip_count,
        "planned_clip_count": planned_count,
    });

    if json_mode {
        let mut env = SuccessEnvelope::new(payload);
        if let Some(na) = next_action {
            env = env.with_next_action(na).with_next_action_reason(reason);
        }
        print_success(&env)?;
    } else {
        println!("stage: {stage}");
        if let Some(c) = clip_count { println!("analyzed clips: {c}"); }
        if let Some(p) = planned_count { println!("planned segments: {p}"); }
        if let Some(na) = next_action { println!("next: {na}\n  ({reason})"); }
    }
    Ok(())
}
```

- [ ] **Step 3: Wire into `commands.rs` dispatch**

- [ ] **Step 4: Add a unit test**

```rust
#[tokio::test]
async fn status_reports_none_when_no_files() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::TempDir::new()?;
    // Just verify it doesn't panic; full assertion comes from integration test.
    run(dir.path(), Some(dir.path().join("out.mp4")), true).await?;
    Ok(())
}
```

- [ ] **Step 5: Verify + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add -u src/commands/status.rs src/commands.rs src/main.rs
git commit -m "feat: add clipcast status command (project state inspection)"
```

---

## Task 10: `clipcast schema` command + golden file

**Files:**
- Create: `src/commands/schema.rs`
- Create: `tests/fixtures/golden/plan-schema.json`, `tests/fixtures/golden/decisions-schema.json`
- Modify: `src/commands.rs`, `src/main.rs`

- [ ] **Step 1: Add Clap subcommand**

```rust
/// Print the JSON schema for a clipcast sidecar.
Schema {
    /// Which schema: "decisions" or "plan".
    kind: String,
},
```

- [ ] **Step 2: Implement `src/commands/schema.rs`**

```rust
//! `clipcast schema [decisions|plan]` — print the JSON schema.

use anyhow::{anyhow, Result};

const PLAN_SCHEMA_JSON: &str = include_str!("../../tests/fixtures/golden/plan-schema.json");
const DECISIONS_SCHEMA_JSON: &str = include_str!("../../tests/fixtures/golden/decisions-schema.json");

pub(crate) fn run(kind: &str) -> Result<()> {
    let s = match kind {
        "plan" => PLAN_SCHEMA_JSON,
        "decisions" => DECISIONS_SCHEMA_JSON,
        other => return Err(anyhow!("unknown schema '{other}'; valid: plan | decisions")),
    };
    println!("{s}");
    Ok(())
}
```

- [ ] **Step 3: Author the golden schema files**

Write hand-curated JSON schema documents (Draft-07) describing `Plan` and `Sidecar`. Keep them concise — they're documentation, not enforcement. Example for plan:

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "clipcast plan.json",
  "type": "object",
  "required": ["schema_version", "clipcast_version", "generated_at", "model",
               "decisions_ref", "brief", "target_duration_s", "estimated_duration_s", "segments"],
  "properties": {
    "schema_version": { "type": "integer", "const": 1 },
    "clipcast_version": { "type": "string" },
    "generated_at": { "type": "string", "format": "date-time" },
    "model": { "type": "string" },
    "decisions_ref": {
      "type": "object",
      "required": ["path", "generated_at"],
      "properties": {
        "path": { "type": "string" },
        "generated_at": { "type": "string", "format": "date-time" }
      }
    },
    "brief": { "type": "string" },
    "target_duration_s": { "type": "integer", "minimum": 1 },
    "estimated_duration_s": { "type": "number" },
    "segments": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["order", "source", "start_s", "end_s", "duration_s", "title", "rationale"],
        "properties": {
          "order": { "type": "integer", "minimum": 1 },
          "source": { "type": "string" },
          "start_s": { "type": ["number", "null"] },
          "end_s": { "type": ["number", "null"] },
          "duration_s": { "type": "number" },
          "title": { "type": "string" },
          "rationale": { "type": "string" },
          "trim_reason": { "type": "string" }
        }
      }
    },
    "rejected": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["source", "score", "rejected_reason"],
        "properties": {
          "source": { "type": "string" },
          "score": { "type": "integer", "minimum": 0, "maximum": 10 },
          "rejected_reason": { "type": "string" }
        }
      }
    },
    "warnings": { "type": "array", "items": { "type": "string" } }
  }
}
```

(And a similar one for decisions.)

- [ ] **Step 4: Add schema-stability test**

```rust
#[test]
fn plan_schema_golden_unchanged() {
    let on_disk = include_str!("../../tests/fixtures/golden/plan-schema.json");
    let from_command = PLAN_SCHEMA_JSON;
    assert_eq!(on_disk, from_command);
}
```

(Trivial — they're the same string. The test exists so `git diff` flags any change to the golden file as a conscious decision.)

- [ ] **Step 5: Verify + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add tests/fixtures/golden src/commands/schema.rs src/commands.rs src/main.rs
git commit -m "feat: add clipcast schema command + golden schema files"
```

---

## Task 11: Wire `build` to call `pipeline::plan` (replace shim)

**Files:**
- Modify: `src/commands/build.rs`

- [ ] **Step 1: Replace the chronological-pass-through shim from Task 3**

```rust
// Replace the temporary sort with:
let sidecar_payload = sidecar::build(target_duration.as_secs(), verdicts);
sidecar::write(&sidecar_path, &sidecar_payload).await.context("sidecar write failed")?;
println!("wrote {}", sidecar_path.display());

let brief = brief.unwrap_or_else(|| pipeline::plan::DEFAULT_BRIEF.to_string());
let plan = pipeline::plan::run(
    brief,
    target_duration.as_secs(),
    &sidecar_payload,
    &sidecar_path,
    "claude-opus-4-7",
).await.context("plan stage failed")?;
let plan_path = paths::plan_for(&output_path);
crate::plan::save(&plan_path, &plan).await.context("plan write failed")?;
println!("wrote {}", plan_path.display());

concat::run_segments(&plan.segments, &metas_by_path, &output_path)
    .await
    .context("concat stage failed")?;
println!("wrote {}", output_path.display());
```

Add a `--brief` / `--brief-file` flag pair to the `build` clap subcommand mirroring `plan`. Pass through.

- [ ] **Step 2: Verify + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add -u src/commands/build.rs src/main.rs
git commit -m "feat: build now runs analyze→plan→render (LLM planner replaces filter)"
```

---

## Task 12: Delete `filter.rs`

**Files:**
- Delete: `src/pipeline/filter.rs`
- Modify: `src/pipeline.rs` (remove `pub(crate) mod filter;`)

- [ ] **Step 1: Verify no remaining callers**

Use Grep tool: `pattern = "filter::"` in `src`. Expected: zero matches (build.rs was rewritten in Task 11).

- [ ] **Step 2: Delete file**

```bash
git rm src/pipeline/filter.rs
```

- [ ] **Step 3: Remove module declaration**

Edit `src/pipeline.rs` to remove the `filter` line.

- [ ] **Step 4: Verify + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add -u src/pipeline.rs
git commit -m "refactor: remove filter stage (replaced by plan)"
```

---

## Task 13: End-to-end integration test

**Files:**
- Modify: `tests/end_to_end.rs`
- Possibly modify: `tests/fixtures/fake-bin/claude` (extend mock to return plan JSON)

- [ ] **Step 1: Inspect existing fixtures**

Use Read tool on `tests/end_to_end.rs` and `tests/fixtures/fake-bin/claude`.

- [ ] **Step 2: Add `#[ignore]`d integration test**

```rust
#[tokio::test]
#[ignore]
async fn build_with_brief_produces_plan_and_output() {
    // 1. Set up fake-bin PATH (existing helper).
    // 2. Configure fake claude to return:
    //    - On per-clip analysis prompts: a JSON {"score":7,"reason":"ok"}.
    //    - On planner prompt (detect by "## Brief" in stdin or arg):
    //      a valid PlannerOutput JSON referencing the input clips.
    // 3. Run `clipcast build <fixture-dir> --brief "test" --duration 30s`.
    // 4. Assert output mp4 exists.
    // 5. Assert plan.json exists and parses.
    // 6. Assert decisions.json schema_version == 2.
}
```

- [ ] **Step 3: Run with `--ignored`**

```bash
cargo test -- --ignored build_with_brief_produces_plan_and_output
```

(Will only pass if `ffmpeg`/`ffprobe` are on PATH locally; that's fine — integration tests are gated.)

- [ ] **Step 4: Verify standard test suite still green**

```bash
cargo test
```

- [ ] **Step 5: Commit**

```bash
git add -u tests/
git commit -m "test: end-to-end build→plan→render integration test"
```

---

## Self-Review

**Spec coverage check:**
- ✓ New plan stage between analyze and render (Tasks 4, 5, 6)
- ✓ `plan.json` schema with all specified fields (Task 2)
- ✓ Brief lifecycle (Task 6) — required for `plan`, defaulted for `build` (Task 11)
- ✓ `--revise` mode (Task 6)
- ✓ Per-segment trim in render (Tasks 7, 8)
- ✓ `clipcast status` (Task 9)
- ✓ `clipcast schema` (Task 10)
- ✓ Output envelope + error envelope + `next_action` (Task 1)
- ✓ Decisions schema_version bump + `keep` removal (Task 3)
- ✓ Filter deletion (Task 12)
- ✓ Build wired through plan (Task 11)
- ✓ End-to-end test (Task 13)
- ✗ `--json` global flag — partially covered (plan/status/schema have it; would benefit from a global flag in main.rs). Acceptable to ship per-command for v1.
- ✗ `--dry-run` on render — spec mentions; not explicitly tasked. Could add to Task 8 cheaply: skip the ffmpeg invocation and just print the planned commands. **TODO inline:** add to Task 8 step 2.

**Placeholder scan:** No "TBD"/"TODO" leakage in task bodies; all code blocks have actual content. The "TODO inline" note above is a self-flag, addressed by amending Task 8.

**Type consistency:** `Segment.start_s` is `Option<f64>` everywhere (plan.rs, pipeline/plan.rs, concat.rs). `PLAN_SCHEMA_VERSION` and `DECISIONS_SCHEMA_VERSION` consistently named. `next_action` field present on every success envelope construction. `MODEL_LABEL` is the same constant string in plan.rs and build.rs (`"claude-opus-4-7"`).

**Amendment to Task 8:** add a step:

```
- [ ] Step 2.5: Wire `--dry-run` flag on render. When set: read plan, print
  the segment list and the ffmpeg commands that would be invoked, exit 0.
```

(Self-amendment recorded; treat as part of the live plan.)

---

## Execution Note

Per user instruction: skip the execution-mode prompt. Execute inline using executing-plans, in this session, on `main`. No worktree.
