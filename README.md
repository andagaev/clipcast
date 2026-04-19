# clipcast

Turn a directory of short video clips (from Meta Ray-Ban glasses or any camera) into one combined vlog video. A multimodal LLM scores every clip, then an LLM planner assembles a reviewable cut plan, and `ffmpeg` renders the final video.

## Install

Prerequisites:

```bash
brew install ffmpeg
# Claude Code (provides the `claude` binary): already installed
```

Install the binary:

```bash
git clone https://github.com/andagaev/clipcast
cd clipcast
cargo install --path .
```

## Usage

### One-shot

```bash
# Full pipeline: discover → frames → score → plan (LLM) → render
clipcast build ~/Desktop/meta-clips/ --brief "Saturday trail run with friends"

# Shorter vlog (default is 3 minutes)
clipcast build ~/Desktop/meta-clips/ --duration 90s --brief "..."

# Brief from a file (markdown welcome)
clipcast build ~/Desktop/meta-clips/ --brief-file brief.md

# Custom output path
clipcast build ~/Desktop/meta-clips/ --out ~/Movies/sat-run.mp4 --brief "..."
```

### Review-and-render workflow

For tighter control, run the stages by hand and hand-edit `plan.json` before rendering.

```bash
# 1. Score every clip in the directory. Writes decisions.json.
clipcast analyze ~/Desktop/meta-clips/

# 2. Generate a cut plan from the scores + your brief.
clipcast plan ~/Desktop/meta-clips/ \
  --brief "Saturday trail run with friends" \
  --duration 90s

# 3. Review and edit the plan (reorder, drop, or trim segments).
${EDITOR:-vi} ~/Desktop/meta-clips/vlog-*.plan.json

# 4. Render the final vlog from the edited plan.
clipcast render ~/Desktop/meta-clips/
```

Ask the planner to revise its plan instead of editing by hand:

```bash
clipcast plan ~/Desktop/meta-clips/ --revise \
  --instructions "cut the last two rain segments, open with the waterfall clip"
```

### Inspect state

```bash
# Where are we in the pipeline? What's the next command?
clipcast status ~/Desktop/meta-clips/ --json

# Show every scored clip
clipcast list ~/Desktop/meta-clips/

# Print the JSON schema for decisions.json or plan.json
clipcast schema plan
clipcast schema decisions
```

### Commands

- `build` — full pipeline in one shot (`analyze` → `plan` → `render`)
- `analyze` — score clips with the LLM; writes `decisions.json`
- `plan` — assemble a cut plan from scored clips; writes `plan.json`. Use `--revise --instructions "..."` to iterate.
- `render` — read `plan.json`, trim segments, concat into the final mp4. `--dry-run` prints the ffmpeg commands without executing.
- `status` — read-only project state with a `next_action` hint (great for agents and scripts).
- `schema` — print the JSON schema for `plan` or `decisions` sidecars.
- `list` — show every scored clip from `decisions.json`.
- `add` — score one new clip and append it to the sidecar; run `plan --revise` afterwards to pull it into the cut.

### Flags

- `--json` (on `plan`, `status`) — emit a structured stdout envelope with `next_action` hints. Stdout is always JSON when piped, even without the flag.
- `--brief` / `--brief-file` (on `build`, `plan`) — freeform description of the vlog you want. Steers the planner's picks and pacing.
- `--dry-run` (on `plan`, `render`) — print what would happen (prompt preview / ffmpeg invocations) without calling the LLM or writing the mp4.
- `--duration <3m | 90s | 300>` — target length. Fed to the planner as a soft budget.

### Output

All artifacts live in the input directory:

- `vlog-YYYY-MM-DD.decisions.json` — LLM scores + reasons for every clip (schema v2)
- `vlog-YYYY-MM-DD.plan.json` — the cut plan: segments, order, trims, rationale, rejected clips, warnings (schema v1). Hand-editable.
- `vlog-YYYY-MM-DD.mp4` — the concatenated vlog (override with `--out`)

### Scope

v1 supports any portrait (height > width) `.mp4` or `.mov` clip. All segment sources must share the same aspect ratio; mismatches error with an actionable message. Landscape sources error too. No transitions, titles, music, or narration in v1 — hard cuts (with optional `start_s`/`end_s` trims per segment), video + audio output.
