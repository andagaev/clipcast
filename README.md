# clipcast

Turn a directory of short video clips (from Meta Ray-Ban glasses or any camera) into one combined vlog video, using a multimodal LLM to auto-select which clips are interesting enough to include.

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

```bash
# One-shot: discover → frames → LLM → filter → concat
clipcast build ~/Desktop/meta-clips/

# Shorter vlog (default is 3 minutes)
clipcast build ~/Desktop/meta-clips/ --duration 90s

# Custom output path
clipcast build ~/Desktop/meta-clips/ --out ~/Movies/sat-run.mp4

# Review-then-render loop: run analysis, hand-edit the sidecar, then concat
clipcast analyze ~/Desktop/meta-clips/
${EDITOR:-vi} ~/Desktop/meta-clips/vlog-*.decisions.json
clipcast render ~/Desktop/meta-clips/
```

### Commands

- `build` — full pipeline in one shot
- `analyze` — discover + frame extraction + LLM scoring + write `decisions.json`, stops before concat
- `render` — read `decisions.json`, skip LLM entirely, do ffmpeg concat only

### Output

All artifacts live in the input directory:

- `vlog-YYYY-MM-DD.mp4` — the concatenated vlog (override with `--out`)
- `vlog-YYYY-MM-DD.decisions.json` — the LLM's verdict on every clip, hand-editable

### Scope

v1 supports only 9:16 (portrait) clips. Non-9:16 sources error with an actionable message. No transitions, titles, music, or narration in v1 — hard cuts only.
