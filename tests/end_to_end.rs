//! Real end-to-end pipeline test.
//!
//! Requires real `ffmpeg`, `ffprobe`, `claude`, and Claude Code auth
//! on the host. `#[ignore]`'d by default. Run with:
//!
//! ```text
//! cargo test -- --ignored
//! ```

use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_clipcast"))
}

/// Generate 3 tiny real 9:16 MP4 clips using ffmpeg's testsrc + sine audio.
fn setup_real_clips() -> Result<TempDir, Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    for i in 1..=3 {
        let out = dir.path().join(format!("clip{i}.mp4"));
        let status = Command::new("ffmpeg")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("testsrc=duration=1:size=1080x1920:rate=1")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("anullsrc=r=16000:cl=mono")
            .arg("-t")
            .arg("1")
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("ultrafast")
            .arg("-c:a")
            .arg("aac")
            .arg("-shortest")
            .arg(&out)
            .output()?;
        if !status.status.success() {
            return Err(format!(
                "ffmpeg fixture gen failed: {}",
                String::from_utf8_lossy(&status.stderr)
            )
            .into());
        }
    }
    Ok(dir)
}

#[test]
#[ignore = "requires real ffmpeg, ffprobe, claude, and Claude Code auth"]
fn real_pipeline_produces_vlog() -> TestResult {
    let tmp = setup_real_clips()?;

    let output = Command::new(binary())
        .arg("build")
        .arg(tmp.path())
        .arg("--duration")
        .arg("10s")
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "pipeline failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )
        .into());
    }

    let entries: Vec<_> = std::fs::read_dir(tmp.path())?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let vlog = entries
        .iter()
        .find(|n| {
            n.starts_with("vlog-")
                && std::path::Path::new(n)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
        })
        .ok_or("no vlog file produced")?;

    let vlog_path = tmp.path().join(vlog);
    let metadata = std::fs::metadata(&vlog_path)?;
    if metadata.len() < 1000 {
        return Err(format!("vlog too small: {} bytes", metadata.len()).into());
    }

    Ok(())
}

#[test]
#[ignore = "requires real ffmpeg, ffprobe, claude, and Claude Code auth"]
fn build_with_brief_produces_plan_and_output() -> TestResult {
    let tmp = setup_real_clips()?;

    let output = Command::new(binary())
        .arg("build")
        .arg(tmp.path())
        .arg("--duration")
        .arg("10s")
        .arg("--brief")
        .arg("A quick test reel of the clips in chronological order.")
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "pipeline failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )
        .into());
    }

    let entries: Vec<String> = std::fs::read_dir(tmp.path())?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    let plan_file = entries
        .iter()
        .find(|n| n.ends_with(".plan.json"))
        .ok_or("no plan.json produced")?;
    let plan_text = std::fs::read_to_string(tmp.path().join(plan_file))?;
    let plan: serde_json::Value = serde_json::from_str(&plan_text)?;
    if plan["schema_version"] != serde_json::json!(1) {
        return Err(format!("wrong plan schema_version: {}", plan["schema_version"]).into());
    }

    let decisions_file = entries
        .iter()
        .find(|n| n.ends_with(".decisions.json"))
        .ok_or("no decisions.json produced")?;
    let decisions_text = std::fs::read_to_string(tmp.path().join(decisions_file))?;
    let decisions: serde_json::Value = serde_json::from_str(&decisions_text)?;
    if decisions["schema_version"] != serde_json::json!(2) {
        return Err(format!(
            "wrong decisions schema_version: {}",
            decisions["schema_version"]
        )
        .into());
    }

    let has_vlog = entries.iter().any(|n| {
        n.starts_with("vlog-")
            && std::path::Path::new(n)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
    });
    if !has_vlog {
        return Err(format!("no vlog produced: {entries:?}").into());
    }

    Ok(())
}
