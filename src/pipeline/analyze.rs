//! Orchestrate `ClipAnalyzer::analyze` across multiple clips with
//! concurrency limiting and per-clip retry logic.
//!
//! - Concurrency: caps via a `tokio::sync::Semaphore` with `concurrency` permits.
//! - Retries: 2 retries (total 3 attempts) per clip with 1s, 4s backoffs.
//! - Failure policy: after the final retry, record the error in the
//!   `ClipVerdict::error` field and continue. The run completes.

use crate::analyzer::{AnalyzerError, ClipAnalyzer};
use crate::clip::ClipVerdict;
use crate::pipeline::frames::ClipFrames;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// Retry backoff schedule in seconds (attempt 2 = 1s, attempt 3 = 4s).
const RETRY_BACKOFFS_S: &[u64] = &[1, 4];

/// Maximum total attempts per clip (1 initial + 2 retries).
const MAX_ATTEMPTS: usize = 3;

/// Run the analyzer over every clip with concurrency cap + retries.
pub(crate) async fn run<A: ClipAnalyzer + Send + Sync + 'static>(
    analyzer: Arc<A>,
    clip_frames: Vec<ClipFrames>,
    concurrency: usize,
) -> Vec<ClipVerdict> {
    let concurrency = concurrency.max(1);
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let mut handles = Vec::with_capacity(clip_frames.len());

    for cf in clip_frames {
        let analyzer = Arc::clone(&analyzer);
        let semaphore = Arc::clone(&semaphore);
        handles.push(tokio::spawn(async move {
            let _permit = semaphore.acquire_owned().await.ok();
            analyze_with_retries(analyzer.as_ref(), &cf).await
        }));
    }

    let mut verdicts = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok(v) = handle.await {
            verdicts.push(v);
        }
    }
    verdicts
}

async fn analyze_with_retries<A: ClipAnalyzer>(analyzer: &A, cf: &ClipFrames) -> ClipVerdict {
    let frame_refs: Vec<&Path> = cf.frame_paths.iter().map(PathBuf::as_path).collect();
    let mut last_err: Option<AnalyzerError> = None;

    for attempt in 0..MAX_ATTEMPTS {
        match analyzer.analyze(&cf.clip, &frame_refs).await {
            Ok(verdict) => return verdict,
            Err(e) => {
                last_err = Some(e);
                if attempt + 1 < MAX_ATTEMPTS {
                    let backoff_s = RETRY_BACKOFFS_S.get(attempt).copied().unwrap_or(4);
                    tokio::time::sleep(Duration::from_secs(backoff_s)).await;
                }
            }
        }
    }

    let error_msg =
        last_err.map_or_else(|| "unknown analyzer error".to_string(), |e| format!("{e}"));
    ClipVerdict {
        path: cf.clip.path.clone(),
        duration_s: cf.clip.meta.duration_s,
        timestamp: cf.clip.meta.timestamp,
        timestamp_source: cf.clip.meta.timestamp_source,
        score: None,
        reason: None,
        error: Some(error_msg),
        keep: false,
    }
}

use std::path::PathBuf;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{Clip, ClipMeta, TimestampSource};
    use chrono::{TimeZone, Utc};
    use std::sync::atomic::{AtomicUsize, Ordering};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn sample_cf() -> Result<ClipFrames, Box<dyn std::error::Error>> {
        let ts = Utc
            .with_ymd_and_hms(2026, 4, 12, 14, 0, 0)
            .single()
            .ok_or("bad timestamp")?;
        Ok(ClipFrames {
            clip: Clip {
                path: PathBuf::from("a.mp4"),
                meta: ClipMeta {
                    duration_s: 10.0,
                    width: 1080,
                    height: 1920,
                    timestamp: ts,
                    timestamp_source: TimestampSource::CreationTime,
                },
            },
            frame_paths: vec![PathBuf::from("/tmp/f0.jpg")],
        })
    }

    struct AlwaysSuccess;
    impl ClipAnalyzer for AlwaysSuccess {
        async fn analyze(
            &self,
            clip: &Clip,
            _frames: &[&Path],
        ) -> Result<ClipVerdict, AnalyzerError> {
            Ok(ClipVerdict {
                path: clip.path.clone(),
                duration_s: clip.meta.duration_s,
                timestamp: clip.meta.timestamp,
                timestamp_source: clip.meta.timestamp_source,
                score: Some(8),
                reason: Some("always 8".to_string()),
                error: None,
                keep: false,
            })
        }
    }

    struct FailsThenSucceeds {
        fail_count: AtomicUsize,
        max_failures: usize,
    }
    impl ClipAnalyzer for FailsThenSucceeds {
        async fn analyze(
            &self,
            clip: &Clip,
            _frames: &[&Path],
        ) -> Result<ClipVerdict, AnalyzerError> {
            let prev = self.fail_count.fetch_add(1, Ordering::SeqCst);
            if prev < self.max_failures {
                Err(AnalyzerError::Empty)
            } else {
                Ok(ClipVerdict {
                    path: clip.path.clone(),
                    duration_s: clip.meta.duration_s,
                    timestamp: clip.meta.timestamp,
                    timestamp_source: clip.meta.timestamp_source,
                    score: Some(5),
                    reason: Some("eventually".to_string()),
                    error: None,
                    keep: false,
                })
            }
        }
    }

    struct AlwaysFails;
    impl ClipAnalyzer for AlwaysFails {
        async fn analyze(
            &self,
            _clip: &Clip,
            _frames: &[&Path],
        ) -> Result<ClipVerdict, AnalyzerError> {
            Err(AnalyzerError::Empty)
        }
    }

    #[tokio::test]
    async fn success_on_first_attempt() -> TestResult {
        let verdict = analyze_with_retries(&AlwaysSuccess, &sample_cf()?).await;
        assert_eq!(verdict.score, Some(8));
        assert!(verdict.error.is_none());
        Ok(())
    }

    #[tokio::test]
    #[ignore = "takes ~1s due to retry backoff"]
    async fn retries_and_eventually_succeeds() -> TestResult {
        let analyzer = FailsThenSucceeds {
            fail_count: AtomicUsize::new(0),
            max_failures: 1,
        };
        let verdict = analyze_with_retries(&analyzer, &sample_cf()?).await;
        assert_eq!(verdict.score, Some(5));
        assert!(verdict.error.is_none());
        Ok(())
    }

    #[tokio::test]
    #[ignore = "takes ~5s due to retry backoffs"]
    async fn all_retries_exhausted_marks_error() -> TestResult {
        let verdict = analyze_with_retries(&AlwaysFails, &sample_cf()?).await;
        assert!(verdict.score.is_none());
        assert!(verdict.error.is_some());
        assert!(verdict
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("empty"));
        Ok(())
    }

    #[tokio::test]
    async fn run_parallelizes_over_clips() -> TestResult {
        let clips = vec![sample_cf()?, sample_cf()?, sample_cf()?];
        let analyzer = Arc::new(AlwaysSuccess);
        let verdicts = run(analyzer, clips, 2).await;
        assert_eq!(verdicts.len(), 3);
        for v in &verdicts {
            assert_eq!(v.score, Some(8));
        }
        Ok(())
    }
}
