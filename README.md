# clipcast

Turn a directory of short video clips (from Meta Ray-Ban glasses or any camera) into one combined vlog video. A multimodal LLM scores every clip, then an LLM planner assembles a reviewable cut plan, and `ffmpeg` renders the final video.

Designed to be driven by a coding agent — every command exposes `--json` structured output with `next_action` hints, so an agent can run the whole pipeline unattended.

## 1. Install

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

Verify:

```bash
clipcast --help
clipcast schema plan   # sanity-check that the binary runs
```

## 2. Run an agent against your clips

Open Claude Code (or any shell-capable agent) in the directory containing your clips and give it a prompt like:

> Drive the clipcast pipeline for the clips in `~/Desktop/meta-clips/`.
> Goal: a 90-second highlight reel. Tone: relaxed, chronological, drop blurry or redundant clips.
>
> Loop:
> 1. Run `clipcast status <dir> --json` and parse it.
> 2. If `stage == "rendered"`, report the `output_path` and stop.
> 3. Otherwise, run the command in `next_action`. For the `plan` step, pass `--brief "<your inferred brief>"` and `--duration 90s`.
> 4. Go back to step 1.
>
> If a command returns `{"error": {"code": ..., "fix": ...}}`, follow the `fix` field.
> To tweak the cut after an initial render, run `clipcast plan <dir> --revise --instructions "..." --json` and then `clipcast render <dir>` again.

Substitute your own clips directory and brief. The agent will:

1. `clipcast analyze <dir>` — score every clip with the LLM. Produces `decisions.json`.
2. `clipcast plan <dir> --brief "..." --duration 90s` — LLM assembles a cut plan. Produces `plan.json`.
3. `clipcast render <dir>` — ffmpeg trims + concats. Produces `vlog-YYYY-MM-DD.mp4`.

All artifacts land in the input directory. You review/edit `plan.json` or ask the agent to `--revise` it.

## 3. Manual usage (optional)

If you'd rather run it yourself without an agent:

```bash
# One-shot
clipcast build ~/Desktop/meta-clips/ \
  --duration 90s \
  --brief "Saturday trail run with friends"

# Step-by-step, editing plan.json between plan and render
clipcast analyze ~/Desktop/meta-clips/
clipcast plan    ~/Desktop/meta-clips/ --brief "..." --duration 90s
${EDITOR:-vi}    ~/Desktop/meta-clips/vlog-*.plan.json
clipcast render  ~/Desktop/meta-clips/

# Ask the planner to revise instead of hand-editing
clipcast plan ~/Desktop/meta-clips/ --revise \
  --instructions "cut the last two rain segments; open with the waterfall"
clipcast render ~/Desktop/meta-clips/
```

### Commands

- `build` — full pipeline one-shot (`analyze` → `plan` → `render`)
- `analyze` — LLM scores every clip; writes `decisions.json`
- `plan` — LLM assembles a cut plan; writes `plan.json`. Use `--revise --instructions "..."` to iterate.
- `render` — trim + concat planned segments. `--dry-run` prints ffmpeg commands without executing.
- `status` — read-only project state with a `next_action` hint (the agent loop's anchor).
- `schema` — print the JSON schema for `plan` or `decisions` sidecars.
- `list` — show every scored clip from `decisions.json`.
- `add` — score one new clip and append to the sidecar; follow up with `plan --revise`.

### Flags

- `--json` (on `plan`, `status`) — force structured stdout envelope. Enabled automatically when stdout is piped.
- `--brief` / `--brief-file` (on `build`, `plan`) — freeform description that steers the planner.
- `--dry-run` (on `plan`, `render`) — preview without calling the LLM or writing the mp4.
- `--duration <3m | 90s | 300>` — target length; soft budget for the planner.

### Output

All artifacts live in the input directory:

- `vlog-YYYY-MM-DD.decisions.json` — LLM scores + reasons for every clip (schema v2)
- `vlog-YYYY-MM-DD.plan.json` — cut plan: segments, order, trims, rationale, rejected clips, warnings (schema v1). Hand-editable.
- `vlog-YYYY-MM-DD.mp4` — the concatenated vlog (override with `--out`)

### Scope

v1 supports any portrait (height > width) `.mp4` or `.mov` clip. All segment sources must share the same aspect ratio; mismatches error with an actionable message. Landscape sources error too. No transitions, titles, music, or narration in v1 — hard cuts (with optional `start_s`/`end_s` trims per segment), video + audio output.
