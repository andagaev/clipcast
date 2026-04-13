//! v1 `ClipAnalyzer` backend: `claude -p` with `@/path/to/frame.jpg` attachments.
//!
//! Spawns `claude -p --input-format text --output-format text
//! --permission-mode plan --settings /dev/null` with `AGENT_COLLECTOR_IGNORE=1`
//! on the environment. The prompt is piped to stdin and references the
//! extracted frames via `@/absolute/path/to/frame.jpg` — the same
//! mechanism Claude Code uses for interactive-mode attachments.

use super::{AnalyzerError, ClipAnalyzer};
use crate::clip::{Clip, ClipVerdict};
use crate::process;
use std::path::Path;

/// v1 analyzer backend: shells out to `claude -p` with frame attachments.
pub(crate) struct ClaudePrintAnalyzer;

impl ClipAnalyzer for ClaudePrintAnalyzer {
    async fn analyze(&self, clip: &Clip, frames: &[&Path]) -> Result<ClipVerdict, AnalyzerError> {
        let prompt = compose_prompt(clip, frames);

        let output = process::run(
            "claude",
            [
                "-p",
                "--input-format",
                "text",
                "--output-format",
                "text",
                "--permission-mode",
                "plan",
                "--settings",
                "/dev/null",
            ],
            [("AGENT_COLLECTOR_IGNORE", "1")],
            Some(prompt.into_bytes()),
        )
        .await?;

        parse_verdict(clip, &output.stdout, &output.stderr)
    }
}

fn compose_prompt(clip: &Clip, frames: &[&Path]) -> String {
    let frame_refs: String = frames
        .iter()
        .map(|p| format!("@{}", p.display()))
        .collect::<Vec<_>>()
        .join(" ");

    format!(
        "You are scoring a short video clip for inclusion in a personal vlog.\n\
         \n\
         Clip file: {path}\n\
         Duration: {duration:.1}s\n\
         Recorded: {timestamp}\n\
         \n\
         Here are 5 frames from the clip at 0%, 25%, 50%, 75%, and 100% \
         of its duration:\n\
         {frames}\n\
         \n\
         Score this clip from 1 to 10 for \"how interesting is it to watch\".\n\
         Higher scores mean better composition, clearer subject, more action, \
         or more emotional impact. Lower scores mean shaky camera, boring \
         subject, mostly ground, or visual noise.\n\
         \n\
         Respond with ONLY a JSON object like:\n\
         {{\"score\": 7, \"reason\": \"one-sentence explanation\"}}\n\
         Do not include any text outside the JSON. Do not wrap in a code fence.",
        path = clip.path.display(),
        duration = clip.meta.duration_s,
        timestamp = clip.meta.timestamp.to_rfc3339(),
        frames = frame_refs,
    )
}

#[derive(serde::Deserialize)]
struct Wire {
    score: u8,
    reason: String,
}

fn parse_verdict(clip: &Clip, stdout: &[u8], stderr: &[u8]) -> Result<ClipVerdict, AnalyzerError> {
    let raw = String::from_utf8_lossy(stdout).trim().to_string();
    if raw.is_empty() {
        return Err(AnalyzerError::Empty);
    }

    let wire: Wire = serde_json::from_str(&raw).map_err(|e| {
        let stderr_text = String::from_utf8_lossy(stderr);
        let details = if stderr_text.trim().is_empty() {
            e.to_string()
        } else {
            format!("{e}\nstderr: {stderr_text}")
        };
        AnalyzerError::ParseFailed {
            details,
            raw: raw.clone(),
        }
    })?;

    Ok(ClipVerdict {
        path: clip.path.clone(),
        duration_s: clip.meta.duration_s,
        timestamp: clip.meta.timestamp,
        timestamp_source: clip.meta.timestamp_source,
        score: Some(wire.score.clamp(1, 10)),
        reason: Some(wire.reason),
        error: None,
        keep: false,
        transcript: clip.transcript.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{ClipMeta, TimestampSource};
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn sample_clip() -> Result<Clip, Box<dyn std::error::Error>> {
        let ts = Utc
            .with_ymd_and_hms(2026, 4, 12, 14, 23, 45)
            .single()
            .ok_or("bad timestamp")?;
        Ok(Clip {
            path: PathBuf::from("IMG_2341.mp4"),
            meta: ClipMeta {
                duration_s: 12.3,
                width: 1080,
                height: 1920,
                timestamp: ts,
                timestamp_source: TimestampSource::CreationTime,
            },
            transcript: None,
        })
    }

    #[test]
    fn compose_prompt_includes_frame_refs() -> TestResult {
        let clip = sample_clip()?;
        let f1 = PathBuf::from("/tmp/frame_0.jpg");
        let f2 = PathBuf::from("/tmp/frame_1.jpg");
        let frames: Vec<&Path> = vec![f1.as_path(), f2.as_path()];
        let prompt = compose_prompt(&clip, &frames);
        assert!(prompt.contains("@/tmp/frame_0.jpg"));
        assert!(prompt.contains("@/tmp/frame_1.jpg"));
        assert!(prompt.contains("IMG_2341.mp4"));
        assert!(prompt.contains("12.3"));
        Ok(())
    }

    #[test]
    fn parse_verdict_happy_path() -> TestResult {
        let clip = sample_clip()?;
        let stdout = br#"{"score": 8, "reason": "clear subject"}"#;
        let v = parse_verdict(&clip, stdout, b"")?;
        assert_eq!(v.score, Some(8));
        assert_eq!(v.reason.as_deref(), Some("clear subject"));
        assert_eq!(v.error, None);
        assert!(!v.keep);
        Ok(())
    }

    #[test]
    fn parse_verdict_clamps_out_of_range() -> TestResult {
        let clip = sample_clip()?;
        let stdout = br#"{"score": 255, "reason": "huh"}"#;
        let v = parse_verdict(&clip, stdout, b"")?;
        assert_eq!(v.score, Some(10));
        Ok(())
    }

    #[test]
    fn parse_verdict_rejects_empty() -> TestResult {
        let clip = sample_clip()?;
        let err = parse_verdict(&clip, b"", b"")
            .err()
            .ok_or("expected error")?;
        if !matches!(err, AnalyzerError::Empty) {
            return Err(format!("wrong variant: {err:?}").into());
        }
        Ok(())
    }

    #[test]
    fn parse_verdict_rejects_garbage() -> TestResult {
        let clip = sample_clip()?;
        let err = parse_verdict(&clip, b"not json at all", b"")
            .err()
            .ok_or("expected error")?;
        if !matches!(err, AnalyzerError::ParseFailed { .. }) {
            return Err(format!("wrong variant: {err:?}").into());
        }
        Ok(())
    }
}
