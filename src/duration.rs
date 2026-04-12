//! Parse human-friendly duration strings for the `--duration` CLI flag.
//!
//! Accepted formats:
//! - `3m` — 3 minutes
//! - `2m30s` — 2 minutes 30 seconds
//! - `90s` — 90 seconds
//! - `300` — bare integer means seconds
//!
//! Mixed `h` / `m` / `s` suffixes in any combination, no whitespace.

use std::time::Duration;

/// Errors from duration string parsing.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ParseDurationError {
    #[error("empty duration string")]
    Empty,

    #[error("invalid duration `{0}`: expected format like `3m`, `2m30s`, `90s`, or `300`")]
    InvalidFormat(String),

    #[error("invalid number in duration `{0}`")]
    InvalidNumber(String),
}

/// Parse a duration string into a `std::time::Duration`.
pub(crate) fn parse(s: &str) -> Result<Duration, ParseDurationError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ParseDurationError::Empty);
    }

    if let Ok(n) = s.parse::<u64>() {
        return Ok(Duration::from_secs(n));
    }

    let mut total_seconds: u64 = 0;
    let mut current_number = String::new();
    let mut saw_any = false;

    for ch in s.chars() {
        if ch.is_ascii_digit() {
            current_number.push(ch);
            continue;
        }
        if current_number.is_empty() {
            return Err(ParseDurationError::InvalidFormat(s.to_string()));
        }
        let n: u64 = current_number
            .parse()
            .map_err(|_| ParseDurationError::InvalidNumber(s.to_string()))?;
        current_number.clear();
        saw_any = true;

        let seconds = match ch {
            'h' | 'H' => n.checked_mul(3600),
            'm' | 'M' => n.checked_mul(60),
            's' | 'S' => Some(n),
            _ => return Err(ParseDurationError::InvalidFormat(s.to_string())),
        };
        let seconds = seconds.ok_or_else(|| ParseDurationError::InvalidFormat(s.to_string()))?;
        total_seconds = total_seconds
            .checked_add(seconds)
            .ok_or_else(|| ParseDurationError::InvalidFormat(s.to_string()))?;
    }

    if !current_number.is_empty() || !saw_any {
        return Err(ParseDurationError::InvalidFormat(s.to_string()));
    }

    Ok(Duration::from_secs(total_seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn parses_bare_seconds() -> TestResult {
        assert_eq!(parse("300")?, Duration::from_secs(300));
        assert_eq!(parse("0")?, Duration::from_secs(0));
        Ok(())
    }

    #[test]
    fn parses_minutes_only() -> TestResult {
        assert_eq!(parse("3m")?, Duration::from_secs(180));
        assert_eq!(parse("10m")?, Duration::from_secs(600));
        Ok(())
    }

    #[test]
    fn parses_seconds_suffix() -> TestResult {
        assert_eq!(parse("90s")?, Duration::from_secs(90));
        Ok(())
    }

    #[test]
    fn parses_mixed_minutes_seconds() -> TestResult {
        assert_eq!(parse("2m30s")?, Duration::from_secs(150));
        assert_eq!(parse("1m1s")?, Duration::from_secs(61));
        Ok(())
    }

    #[test]
    fn parses_hours_minutes_seconds() -> TestResult {
        assert_eq!(parse("1h")?, Duration::from_secs(3600));
        assert_eq!(parse("1h30m")?, Duration::from_secs(5400));
        assert_eq!(parse("1h2m3s")?, Duration::from_secs(3723));
        Ok(())
    }

    #[test]
    fn rejects_empty() {
        assert!(matches!(parse(""), Err(ParseDurationError::Empty)));
        assert!(matches!(parse("   "), Err(ParseDurationError::Empty)));
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            parse("abc"),
            Err(ParseDurationError::InvalidFormat(_))
        ));
        assert!(matches!(
            parse("3x"),
            Err(ParseDurationError::InvalidFormat(_))
        ));
        assert!(matches!(
            parse("m30"),
            Err(ParseDurationError::InvalidFormat(_))
        ));
    }
}
