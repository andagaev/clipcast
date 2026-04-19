# Vlog Planning Stage — Agent-Driven Cut Assembly

**Date:** 2026-04-18
**Status:** Approved (design); pending implementation plan
**Supersedes:** `filter` stage in `2026-04-12-clipcast-design.md`

## Goal

Replace the current deterministic greedy-by-score `filter` stage with an LLM-driven `plan` stage that assembles a narrative cut. The user (via Claude Code as orchestrator) provides a freeform brief; the planner emits a reviewable, hand-editable `plan.json` describing the running order, optional per-segment trims, and rationale for each slot. The user reviews and edits the plan; `render` consumes it.

## Workflow

```
user → Claude Code → clipcast analyze → clipcast plan --brief "..." → (review/edit plan.json) → clipcast render
```

Claude Code is the only intended caller of `clipcast`. The CLI is designed for agent consumption first, human use second.

## Design Principles

The CLI's primary user is an LLM agent. Every interface decision flows from this:

1. **The filesystem is the only state.** Every command reads inputs from disk, writes outputs to disk. No hidden sessions, no env-dependent magic. The agent can stop and resume from any state.
2. **Every command answers "what do I run next?"** in machine-readable form.
3. **Stdout is structured; stderr is prose.** Agents parse stdout. Humans (and progress logs) read stderr.
4. **No interactive prompts, ever.** Missing required info = exit with structured error.
5. **Errors are actionable.** Every error includes a `fix` field telling the agent what to do.
6. **Schemas are versioned and self-discoverable** via `clipcast schema [decisions|plan]`.

## Pipeline Shape

```
discover → ffprobe → frames → transcribe → analyze (per-clip LLM) → plan (whole-cut LLM) → render (ffmpeg)
```

The `filter` stage is removed. Its responsibilities (drop low scores, fit a duration budget) move into the planner's prompt.

## Command Surface

### `clipcast analyze <dir> [flags]`
Per-clip scoring + transcription. Unchanged from today **except**: the `keep` field is removed from `decisions.json`. That decision now belongs to `plan`.

### `clipcast plan <dir> [flags]`
Reads `decisions.json`, calls the planning LLM, writes `plan.json`.

Flags:
- `--brief "..."` — freeform vlog brief (string).
- `--brief-file <path>` — read brief from a file (markdown welcome).
- `--duration 3m` — target duration. Required.
- `--revise` — read existing `plan.json` and ask the LLM to revise it according to `--instructions`. Mutually exclusive with the brief flags (revise reuses the brief stored in the existing plan).
- `--instructions "..."` — freeform revision instructions, only valid with `--revise`.
- `--dry-run` — print the brief + the candidate clips that would be considered, without calling the LLM.

Brief lifecycle (from design decision D):
- First call to `clipcast plan`: `--brief` or `--brief-file` is required; brief is persisted into the resulting `plan.json` as a top-level field. No brief = error `missing_brief`.
- Subsequent calls (existing `plan.json` present): brief defaults to the one stored in the existing plan. `--brief` overrides if provided.
- `clipcast build` (the lazy convenience wrapper): when no brief is given, build supplies the **default brief** automatically. The default brief is still passed through the LLM (per design decision B; the LLM is always called for planning).

Default brief used by `build` when none is provided:

> "Assemble a chronological highlight reel of the best clips fitting the target duration. Drop clips that are blurry, redundant, or scored low. Trim dead time at clip starts/ends only when obvious."

The default brief is persisted into `plan.json` like any other brief — the file always self-describes.

### `clipcast render <dir>`
Reads `plan.json`, performs per-segment ffmpeg trim + concat, writes the final `.mp4`. Aspect ratio check is per-segment.

### `clipcast build <dir> [flags]`
Convenience wrapper: runs analyze → plan → render with no review gate. Same flags as `plan`. The LLM is always called for the planning step (no fallback to non-LLM filtering).

### `clipcast status <dir>` (NEW)
Read-only inspection. The first command an agent runs to orient itself. Returns:

```json
{
  "stage": "analyzed",
  "decisions_path": "trip.decisions.json",
  "plan_path": null,
  "output_path": null,
  "clip_count": 27,
  "planned_clip_count": null,
  "next_action": "clipcast plan <dir> --brief '...'",
  "next_action_reason": "decisions.json exists; plan.json missing"
}
```

`stage` is one of: `none | analyzed | planned | rendered`.

### `clipcast schema [decisions|plan]` (NEW)
Prints the JSON schema for either sidecar to stdout. Lets the agent ground itself on the data model without reading Rust source.

## Output Conventions (Agent-First)

- **`--json` flag** on every non-render command. When stdout is not a TTY, JSON output is the default — auto-switches in agent contexts.
- Stdout = a single JSON object (success result or error envelope).
- Stderr = human-readable progress lines. Agents may ignore.
- Every success result includes a `next_action` field (string command to run next, or `null` if the pipeline is complete).
- `render` is mostly side-effect (writes mp4); prints a single JSON result line at end with `{"output_path": "...", "duration_s": ..., "next_action": null}`.

### Error Envelope

```json
{
  "error": {
    "code": "missing_decisions",
    "message": "decisions.json not found at trip.decisions.json",
    "fix": "run `clipcast analyze <dir>` first"
  }
}
```

Error codes are stable across versions. Documented in `clipcast --help`.

Plan-stage error codes:
- `missing_decisions` — no `decisions.json` at expected path
- `empty_decisions` — decisions exist but contain no scored clips
- `missing_brief` — first plan call without `--brief`/`--brief-file`
- `revise_without_plan` — `--revise` passed but no existing `plan.json`
- `llm_call_failed` — `claude -p` returned non-zero or invalid JSON
- `invalid_plan_json` — LLM output failed schema validation
- `stale_plan` — (warning, not error) `decisions.json` is newer than `plan.decisions_ref.generated_at`

## File Path Conventions

Predictable and documented in `--help`:

- `<output_dir>/<input_basename>.decisions.json`
- `<output_dir>/<input_basename>.plan.json`
- `<output_dir>/<input_basename>.mp4`

Agent never has to guess.

## `plan.json` Schema

```json
{
  "schema_version": 1,
  "clipcast_version": "0.x.x",
  "generated_at": "2026-04-18T14:32:11Z",
  "model": "claude-opus-4-7",
  "decisions_ref": {
    "path": "trip.decisions.json",
    "generated_at": "2026-04-18T14:28:03Z"
  },
  "brief": "Family beach day, ~3 min, focus on the kids in the water.",
  "target_duration_s": 180,
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
    },
    {
      "order": 2,
      "source": "GH010238.MP4",
      "start_s": 4.2,
      "end_s": 18.0,
      "duration_s": 13.8,
      "title": "Maya's first wave",
      "rationale": "Emotional payoff early — hooks the viewer.",
      "trim_reason": "First 4s is camera pointing at sand."
    }
  ],
  "rejected": [
    {
      "source": "GH010240.MP4",
      "score": 3,
      "rejected_reason": "Redundant with GH010238 — same wave, weaker angle."
    }
  ],
  "warnings": [
    "Mixed aspect ratios: GH010237 is 16:9, others are 9:16. Render will fail unless you drop GH010237."
  ]
}
```

### Field-Level Rules

- **`schema_version`** — first field; render refuses unknown versions and points the agent at `clipcast schema plan` via `fix`.
- **`decisions_ref`** — links plan to the specific `decisions.json` it was built from. If `decisions.generated_at > plan.decisions_ref.generated_at`, render emits a `stale_plan` warning to stderr but proceeds.
- **`brief`** — persisted brief (from `--brief` or `--brief-file`).
- **`segments[].order`** — explicit integer (not implicit from array index). Render sorts by `order` and warns on duplicates/gaps.
- **`start_s` / `end_s`** — always present; `null` means "use whole clip." Explicit nulls keep the schema rigid for agent consumption.
- **`duration_s`** — computed convenience field; render recomputes from source and warns on drift > 0.1s.
- **`title`** — one-line description of what's in the segment (kept separate from `rationale` so each can be edited independently).
- **`rationale`** — one-line narrative justification for this slot.
- **`trim_reason`** — only present when the segment is trimmed.
- **`rejected[]`** — clips deliberately left out of the cut, with the planner's reason. Distinct from per-clip score reason in `decisions.json`.
- **`warnings[]`** — non-fatal issues; render echoes to stderr but doesn't block.

### Out of Scope for v1

- Transitions, fades, music, captions (render stays straight concat with source audio).
- Per-clip aspect ratio normalization (render keeps strict-equality check; warn at plan time).
- Stable segment IDs / UUIDs (`order` is enough for revise operations).
- Preview rough-cut command (`clipcast preview`) — recognized as future work; schema and command surface designed to leave room for it.

## `decisions.json` Schema Change

Field removed:
- `clips[].keep` — moved to `plan.json` as inclusion in `segments[]`.

`schema_version` bump to 2 on the existing decisions sidecar; analyze writes the new shape; render no longer reads decisions directly (it reads plan).

## Planning LLM Prompt Strategy

Single-shot `claude -p` call with structured-output mode. The planner sees text only — no frames. Visual/audio quality reasoning already happened in `analyze` and is summarized in each clip's `score`, `reason`, and `transcript` fields.

Planner input:
- The brief
- Target duration
- Per-clip rows from `decisions.json`: path, duration, timestamp, score, reason, transcript

Planner output: a JSON object matching the `plan.json` schema (segments + rejected + warnings + estimated_duration_s).

For `--revise`: prompt = brief (from existing plan) + existing plan + revision instructions. Output = a new plan.

Planner prompt template lives at `prompts/plan.md` (alongside the existing analyzer prompts).

## Render Stage Changes

Today: ffmpeg concat of whole clips with matching aspect ratio.

New behavior:
1. Sort `segments` by `order`.
2. For each segment:
   - If `start_s` and `end_s` are both `null`: use whole source clip (current behavior).
   - Otherwise: ffmpeg trim using `-ss <start_s> -to <end_s>`. Prefer `-c copy` (stream copy) when keyframe-aligned; fall back to re-encode when not. Re-encode emits a stderr warning.
3. Concat the (possibly trimmed) segments using existing concat code.
4. Per-segment aspect-ratio check before concat; mismatched aspect = error with `fix` listing the offending segment paths.

## Module Layout Changes

Following the project's `module.rs` + `module/` convention:

- **New:** `src/pipeline/plan.rs` — orchestrates the planning LLM call and `plan.json` write.
- **New:** `src/plan.rs` — typed structs for `Plan`, `Segment`, `RejectedClip`, with serde + a separate `PlanError` (`thiserror`).
- **New:** `src/commands/plan.rs` — CLI command for `clipcast plan`.
- **New:** `src/commands/status.rs` — CLI command for `clipcast status`.
- **New:** `src/commands/schema.rs` — CLI command for `clipcast schema`.
- **New:** `src/output.rs` — JSON output envelope, error envelope, stdout/stderr discipline. Used by every command.
- **Removed:** `src/pipeline/filter.rs` (and tests). Its budget-fitting logic moves into the planner prompt.
- **Modified:** `src/pipeline/concat.rs` — gain per-segment trim support; aspect-ratio check moves to per-segment.
- **Modified:** `src/clip.rs` — drop `keep` field from `ClipVerdict`.
- **Modified:** `src/sidecar.rs` — bump `schema_version` to 2; remove `keep` reads/writes.
- **Modified:** `src/commands/build.rs` — replace `filter::apply` call with `plan::run`.
- **Modified:** `src/commands/render.rs` — read `plan.json` instead of `decisions.json`.
- **New prompt:** `prompts/plan.md`.

## Testing Strategy

- **Unit tests** (in-file `#[cfg(test)] mod tests`):
  - `plan.rs` — schema round-trip; revise round-trip; explicit-null trim handling.
  - `output.rs` — JSON envelope serialization, error envelope shape stability.
  - `pipeline/plan.rs` — prompt construction (input subsetting from decisions), LLM-output parse failure surfaces as `invalid_plan_json`.
  - `pipeline/concat.rs` — per-segment trim, aspect mismatch detection.
- **Subprocess mocks** (`tests/mock.rs`, `tests/fixtures/fake-bin/claude`) — mock the planning LLM call; cover happy path, malformed JSON, non-zero exit.
- **Integration tests** (`tests/end_to_end.rs`, `#[ignore]` by default) — full analyze → plan → render against real binaries with a tiny fixture set; one test for `--revise` flow.
- **Schema-stability test** — `clipcast schema plan` output is byte-equal to a checked-in golden file. Forces conscious schema bumps.

## Migration / Compatibility

- Single-user local tool: no migration tooling for old `decisions.json` files. Re-run `analyze` if you hit `schema_version: 1`.
- `clipcast` will refuse to read sidecars with unknown `schema_version`, surfacing a `fix` telling the agent which command to re-run.

## Open Questions Deferred

- **Preview rough-cut (`clipcast preview`)** — left out of v1. Schema is forward-compatible; will be a new command consuming `plan.json`.
- **Re-encode-on-non-keyframe-aligned trim** — v1 falls back to re-encode with a warning; future work could add a "snap trim points to nearest keyframe" option.
- **Multi-take selection** — when the planner sees N takes of the same moment, it picks one; currently the rejected reason is the only signal of why. Future work could surface "alternates" per slot.
