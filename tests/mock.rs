//! Subprocess-mock integration tests.
//!
//! These tests run the real `clipcast` binary against fake `ffmpeg`,
//! `ffprobe`, and `claude` shell scripts in `tests/fixtures/fake-bin/`.
//! They verify full command dispatch, pipeline wiring, `AGENT_COLLECTOR_IGNORE`
//! propagation, and sidecar read/write round-tripping — without needing
//! real ffmpeg, real ffprobe, or real claude API usage.

use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_clipcast"))
}

fn fake_bin_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests");
    p.push("fixtures");
    p.push("fake-bin");
    p
}

fn fixture_clips_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests");
    p.push("fixtures");
    p.push("clips");
    p
}

fn is_vlog_mp4(name: &String) -> bool {
    if !name.starts_with("vlog-") {
        return false;
    }
    std::path::Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
}

/// Prepend fake-bin to PATH so `ffmpeg`/`ffprobe`/`claude` resolve to our scripts.
fn path_with_fakes() -> String {
    let fakes = fake_bin_dir();
    let existing = std::env::var("PATH").unwrap_or_default();
    format!("{}:{existing}", fakes.display())
}

/// Copy the 3 fixture clips into a fresh tempdir.
fn setup() -> Result<(TempDir, PathBuf), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    for i in 1..=3 {
        let src = fixture_clips_dir().join(format!("clip{i}.mp4"));
        let dst = dir.path().join(format!("clip{i}.mp4"));
        std::fs::copy(&src, &dst)?;
    }
    let path = dir.path().to_path_buf();
    Ok((dir, path))
}

#[test]
fn build_produces_output_and_sidecar() -> TestResult {
    let (_tmp, input_dir) = setup()?;

    let output = Command::new(binary())
        .env("PATH", path_with_fakes())
        .arg("build")
        .arg(&input_dir)
        .arg("--duration")
        .arg("30s")
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "build failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )
        .into());
    }

    let entries: Vec<_> = std::fs::read_dir(&input_dir)?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    let has_sidecar = entries.iter().any(|n| n.contains(".decisions.json"));
    let has_vlog = entries.iter().any(is_vlog_mp4);
    if !has_sidecar {
        return Err(format!("no sidecar in {entries:?}").into());
    }
    if !has_vlog {
        return Err(format!("no vlog output in {entries:?}").into());
    }

    Ok(())
}

#[test]
fn analyze_writes_sidecar_without_concat() -> TestResult {
    let (_tmp, input_dir) = setup()?;

    let output = Command::new(binary())
        .env("PATH", path_with_fakes())
        .arg("analyze")
        .arg(&input_dir)
        .arg("--duration")
        .arg("60s")
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "analyze failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let entries: Vec<_> = std::fs::read_dir(&input_dir)?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    let has_sidecar = entries.iter().any(|n| n.contains(".decisions.json"));
    let has_vlog = entries.iter().any(is_vlog_mp4);
    if !has_sidecar {
        return Err(format!("no sidecar in {entries:?}").into());
    }
    if has_vlog {
        return Err(format!("analyze should NOT produce a vlog, got: {entries:?}").into());
    }

    Ok(())
}

#[test]
fn analyze_then_plan_then_render() -> TestResult {
    let (_tmp, input_dir) = setup()?;

    let a = Command::new(binary())
        .env("PATH", path_with_fakes())
        .arg("analyze")
        .arg(&input_dir)
        .arg("--duration")
        .arg("60s")
        .output()?;
    if !a.status.success() {
        return Err(format!("analyze failed: {}", String::from_utf8_lossy(&a.stderr)).into());
    }

    let p = Command::new(binary())
        .env("PATH", path_with_fakes())
        .arg("plan")
        .arg(&input_dir)
        .arg("--duration")
        .arg("60s")
        .arg("--brief")
        .arg("test brief")
        .output()?;
    if !p.status.success() {
        return Err(format!("plan failed: {}", String::from_utf8_lossy(&p.stderr)).into());
    }

    let entries: Vec<_> = std::fs::read_dir(&input_dir)?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    if !entries.iter().any(|n| n.contains(".plan.json")) {
        return Err(format!("no plan.json in {entries:?}").into());
    }

    let r = Command::new(binary())
        .env("PATH", path_with_fakes())
        .arg("render")
        .arg(&input_dir)
        .output()?;
    if !r.status.success() {
        return Err(format!("render failed: {}", String::from_utf8_lossy(&r.stderr)).into());
    }

    let entries: Vec<_> = std::fs::read_dir(&input_dir)?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let has_vlog = entries.iter().any(is_vlog_mp4);
    if !has_vlog {
        return Err(format!("render produced no vlog: {entries:?}").into());
    }
    Ok(())
}

#[test]
fn missing_binary_fails_preflight() -> TestResult {
    let (_tmp, input_dir) = setup()?;
    let output = Command::new(binary())
        .env("PATH", "/nowhere-xyzzy")
        .arg("build")
        .arg(&input_dir)
        .output()?;

    if output.status.success() {
        return Err("expected preflight failure".into());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.contains("not on PATH") && !stderr.contains("binary") {
        return Err(format!("expected actionable preflight error, got: {stderr}").into());
    }
    Ok(())
}

#[test]
fn agent_collector_ignore_is_propagated_to_claude() -> TestResult {
    let (_tmp, input_dir) = setup()?;
    let output = Command::new(binary())
        .env("PATH", path_with_fakes())
        .arg("build")
        .arg(&input_dir)
        .arg("--duration")
        .arg("30s")
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("AGENT_COLLECTOR_IGNORE not set") {
            return Err("sentinel failed to propagate to claude subprocess".into());
        }
        return Err(format!("unexpected build failure: {stderr}").into());
    }
    Ok(())
}
