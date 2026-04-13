//! Transcribe each clip's audio using `whisper-cli` from whisper.cpp.
//!
//! For each clip:
//! 1. `ffmpeg` extracts 16 kHz mono WAV bytes from the video
//! 2. Those bytes are piped on stdin to `whisper-cli -m <model> -f - -otxt -of -`
//! 3. stdout contains the plain-text transcript
//!
//! If `whisper-cli` or a model file is not available, transcription is
//! skipped silently (every clip's transcript stays `None`) — the rest
//! of the pipeline still runs, just without text context for the LLM.

use crate::clip::Clip;
use crate::process::{self, ProcessError};
use std::path::Path;

/// Errors from the transcribe stage.
#[derive(Debug, thiserror::Error)]
pub(crate) enum TranscribeError {
    #[error(transparent)]
    Process(#[from] ProcessError),
}

/// Transcribe every clip in `clips` in place. Returns the same vec with
/// `clip.transcript` populated for every clip where whisper produced output.
///
/// `model_path`: absolute path to a whisper `.bin` model file. If `None`,
/// the function is a no-op (every clip keeps its existing `transcript`).
pub(crate) async fn run(
    clips: &mut [Clip],
    model_path: Option<&Path>,
) -> Result<(), TranscribeError> {
    let Some(model_path) = model_path else {
        return Ok(());
    };

    for clip in clips.iter_mut() {
        match transcribe_one(&clip.path, model_path).await {
            Ok(text) => {
                let trimmed = text.trim();
                clip.transcript = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
            }
            Err(_) => {
                clip.transcript = None;
            }
        }
    }

    Ok(())
}

/// Extract audio + run whisper for a single clip.
pub(crate) async fn transcribe_one(
    video_path: &Path,
    model_path: &Path,
) -> Result<String, TranscribeError> {
    let wav_bytes = extract_wav(video_path).await?;

    let model_str = model_path.to_string_lossy().into_owned();
    let output = process::run(
        "whisper-cli",
        [
            "-m",
            model_str.as_str(),
            "-f",
            "-",
            "--no-prints",
            "-otxt",
            "-of",
            "-",
        ],
        std::iter::empty::<(&str, &str)>(),
        Some(wav_bytes),
    )
    .await?;

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Extract 16 kHz mono WAV bytes from a video via ffmpeg.
async fn extract_wav(video: &Path) -> Result<Vec<u8>, ProcessError> {
    let video_str = video.to_string_lossy().into_owned();
    let output = process::run(
        "ffmpeg",
        [
            "-nostdin",
            "-loglevel",
            "error",
            "-i",
            video_str.as_str(),
            "-ac",
            "1",
            "-ar",
            "16000",
            "-f",
            "wav",
            "-",
        ],
        std::iter::empty::<(&str, &str)>(),
        None,
    )
    .await?;
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{ClipMeta, TimestampSource};
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[tokio::test]
    async fn run_is_noop_when_model_is_none() -> TestResult {
        let ts = Utc
            .with_ymd_and_hms(2026, 4, 12, 14, 0, 0)
            .single()
            .ok_or("bad timestamp")?;
        let mut clips = vec![Clip {
            path: PathBuf::from("nonexistent.mov"),
            meta: ClipMeta {
                duration_s: 10.0,
                width: 1080,
                height: 1920,
                timestamp: ts,
                timestamp_source: TimestampSource::CreationTime,
            },
            transcript: Some("existing".to_string()),
        }];
        run(&mut clips, None).await?;
        assert_eq!(clips[0].transcript.as_deref(), Some("existing"));
        Ok(())
    }
}
