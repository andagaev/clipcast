# clipcast

## Project Facts

- Single-binary Rust CLI (`clipcast`), no workspace
- Personal tool: local-first, single-user, no accounts
- Pipeline: `ffprobe` (metadata) → `ffmpeg` (frame extraction) → `claude -p` (multimodal scoring) → filter + budget → `ffmpeg` (concat)
- Three subcommands: `build`, `analyze`, `render`
- Decisions cached in `decisions.json` sidecar next to the output, user-editable
- LLM hook pollution suppressed via `AGENT_COLLECTOR_IGNORE=1` env on claude subprocess
- Design spec: `ideas/docs/superpowers/specs/2026-04-12-clipcast-design.md`

## Module Layout

```
src/
├── main.rs               # clap dispatch
├── commands.rs           # router
├── commands/
│   ├── build.rs
│   ├── analyze.rs
│   └── render.rs
├── pipeline.rs           # router
├── pipeline/
│   ├── discover.rs       # scan + ffprobe
│   ├── frames.rs         # ffmpeg frame extraction
│   ├── analyze.rs        # orchestrator with parallelism + retries
│   ├── filter.rs         # score + duration budget
│   └── concat.rs         # aspect check + ffmpeg concat
├── analyzer.rs           # ClipAnalyzer trait
├── analyzer/
│   └── claude_print.rs   # v1 backend
├── process.rs            # shared async subprocess runner
├── preflight.rs          # binary + input dir checks
├── clip.rs               # Clip / ClipMeta / ClipVerdict / TimestampSource
├── paths.rs              # output + sidecar path derivation
├── sidecar.rs            # decisions.json read/write
└── duration.rs           # duration string parser
```

`module.rs` + `module/` pairing: `module.rs` declares children and re-exports, nothing else.

## Rules

- Never `#[allow(...)]` to silence clippy. Fix the code or add a project-level `"allow"` in `Cargo.toml` `[lints.clippy]`. ONE documented exception: `#[allow(async_fn_in_trait)]` on the `ClipAnalyzer` trait (see src/analyzer.rs docstring).
- No `unsafe`.
- No `panic!` in production code. `unwrap`/`expect` allowed only in tests — and in tests, prefer `TestResult = Result<(), Box<dyn std::error::Error>>` with `?` to avoid tripping `unwrap_used` warn.
- Typed errors with `thiserror` at module boundaries. `anyhow::Error` only at `main.rs` and inside `commands/*.rs`.
- Tests live in-file under `#[cfg(test)] mod tests`. Integration tests under `tests/`.
- Singular filenames. Router/dir pairs match directory names (`commands.rs` + `commands/`).
- Imports at top of file. Prefer `use crate::pipeline;` then `pipeline::discover::run(...)` over flat imports unless materially clearer.
- Docstrings on every `pub(crate)` item.

## Verification Gate

Run before committing:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

All three must pass. Pre-commit hook enforces fmt + clippy automatically (not test — CI handles that).

## Tests

- Unit tests: in-file `#[cfg(test)] mod tests`, `TestResult = Result<(), Box<dyn std::error::Error>>` idiom for tests that would otherwise `.unwrap()`
- Subprocess mocks: `tests/mock.rs` using `tests/fixtures/fake-bin/*` shell scripts
- Real integration: `tests/end_to_end.rs`, every test `#[ignore]`'d by default, opt-in with `cargo test -- --ignored`

## External Dependencies

On PATH at runtime:
- `ffmpeg` — frame extraction, concat
- `ffprobe` — container metadata (ships with ffmpeg)
- `claude` — multimodal LLM via `claude -p`

Preflight check in `src/preflight.rs` fails fast if any are missing.

## Commit Conventions

- Conventional commits (`feat:`, `fix:`, `test:`, `chore:`, `docs:`, `refactor:`)
- NO `Co-Authored-By` line in commit messages (user preference)
