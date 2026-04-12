//! Filter clips by score and fill to a target duration budget.
//!
//! Algorithm:
//! 1. Drop clips with `score.is_none()` or `error.is_some()`.
//! 2. Sort by score descending (stable; ties broken by timestamp ascending).
//! 3. Walk top-to-bottom, accumulating duration; mark `keep = true`
//!    while total <= budget. Stop at the first clip that would overflow.
//!
//! The function mutates the passed-in `Vec<ClipVerdict>` in place so the
//! caller (build / analyze command) can write it to the sidecar directly.

use crate::clip::ClipVerdict;
use std::time::Duration;

/// Errors from the filter stage.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FilterError {
    #[error(
        "no clip fits the target duration budget.\n\
         Budget: {budget_s:.1}s. Shortest candidate: {shortest_s:.1}s.\n\
         Either increase --duration or drop the long clips from decisions.json."
    )]
    NoClipFitsBudget { budget_s: f64, shortest_s: f64 },

    #[error("all clips failed analysis or were excluded")]
    NothingToInclude,
}

/// Apply the filter + budget algorithm in place.
///
/// After this function returns, `verdicts[i].keep == true` iff clip i
/// should be in the final vlog. The order of `verdicts` is unchanged
/// from the caller's perspective except for the `keep` flag.
pub(crate) fn apply(verdicts: &mut [ClipVerdict], target: Duration) -> Result<(), FilterError> {
    for v in verdicts.iter_mut() {
        v.keep = false;
    }

    let candidates: Vec<usize> = verdicts
        .iter()
        .enumerate()
        .filter(|(_, v)| v.score.is_some() && v.error.is_none())
        .map(|(i, _)| i)
        .collect();

    if candidates.is_empty() {
        return Err(FilterError::NothingToInclude);
    }

    let mut sorted = candidates.clone();
    sorted.sort_by(|&a, &b| {
        let va = &verdicts[a];
        let vb = &verdicts[b];
        let sa = va.score.unwrap_or(0);
        let sb = vb.score.unwrap_or(0);
        sb.cmp(&sa).then(va.timestamp.cmp(&vb.timestamp))
    });

    let budget_s = target.as_secs_f64();
    let mut total_s: f64 = 0.0;
    let mut any_included = false;

    for idx in sorted {
        let clip_s = verdicts[idx].duration_s;
        if total_s + clip_s <= budget_s {
            verdicts[idx].keep = true;
            total_s += clip_s;
            any_included = true;
        }
    }

    if !any_included {
        let shortest_s = verdicts
            .iter()
            .filter(|v| v.score.is_some() && v.error.is_none())
            .map(|v| v.duration_s)
            .fold(f64::INFINITY, f64::min);
        return Err(FilterError::NoClipFitsBudget {
            budget_s,
            shortest_s,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::TimestampSource;
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn verdict(
        name: &str,
        duration: f64,
        score: Option<u8>,
        error: Option<&str>,
    ) -> Result<ClipVerdict, Box<dyn std::error::Error>> {
        let ts = Utc
            .with_ymd_and_hms(2026, 4, 12, 14, 0, 0)
            .single()
            .ok_or("bad timestamp")?;
        Ok(ClipVerdict {
            path: PathBuf::from(name),
            duration_s: duration,
            timestamp: ts,
            timestamp_source: TimestampSource::CreationTime,
            score,
            reason: None,
            error: error.map(str::to_string),
            keep: false,
        })
    }

    #[test]
    fn keeps_highest_scoring_clips_within_budget() -> TestResult {
        let mut v = vec![
            verdict("a.mp4", 30.0, Some(5), None)?,
            verdict("b.mp4", 30.0, Some(9), None)?,
            verdict("c.mp4", 30.0, Some(7), None)?,
            verdict("d.mp4", 30.0, Some(3), None)?,
        ];
        apply(&mut v, Duration::from_secs(60))?;
        let kept: Vec<&str> = v
            .iter()
            .filter(|x| x.keep)
            .map(|x| x.path.to_str().unwrap_or(""))
            .collect();
        assert!(kept.contains(&"b.mp4"));
        assert!(kept.contains(&"c.mp4"));
        assert_eq!(kept.len(), 2);
        Ok(())
    }

    #[test]
    fn drops_errored_clips() -> TestResult {
        let mut v = vec![
            verdict("good.mp4", 10.0, Some(8), None)?,
            verdict("bad.mp4", 10.0, None, Some("failed"))?,
        ];
        apply(&mut v, Duration::from_secs(180))?;
        assert!(v[0].keep);
        assert!(!v[1].keep);
        Ok(())
    }

    #[test]
    fn drops_scoreless_clips() -> TestResult {
        let mut v = vec![
            verdict("good.mp4", 10.0, Some(8), None)?,
            verdict("noscore.mp4", 10.0, None, None)?,
        ];
        apply(&mut v, Duration::from_secs(180))?;
        assert!(v[0].keep);
        assert!(!v[1].keep);
        Ok(())
    }

    #[test]
    fn errors_when_everything_is_errored() -> TestResult {
        let mut v = vec![
            verdict("a.mp4", 10.0, None, Some("fail"))?,
            verdict("b.mp4", 10.0, None, Some("fail"))?,
        ];
        let result = apply(&mut v, Duration::from_secs(180));
        if !matches!(result, Err(FilterError::NothingToInclude)) {
            return Err(format!("wrong result: {result:?}").into());
        }
        Ok(())
    }

    #[test]
    fn errors_when_budget_too_small_for_any_clip() -> TestResult {
        let mut v = vec![
            verdict("long.mp4", 60.0, Some(9), None)?,
            verdict("also-long.mp4", 45.0, Some(7), None)?,
        ];
        let result = apply(&mut v, Duration::from_secs(30));
        if !matches!(
            result,
            Err(FilterError::NoClipFitsBudget {
                budget_s: _,
                shortest_s: _
            })
        ) {
            return Err(format!("wrong result: {result:?}").into());
        }
        Ok(())
    }

    #[test]
    fn build_resets_existing_keep_flags() -> TestResult {
        let mut v = vec![
            verdict("a.mp4", 10.0, Some(5), None)?,
            verdict("b.mp4", 10.0, Some(9), None)?,
        ];
        v[0].keep = true;
        v[1].keep = true;
        apply(&mut v, Duration::from_secs(10))?;
        assert!(!v[0].keep);
        assert!(v[1].keep);
        Ok(())
    }
}
