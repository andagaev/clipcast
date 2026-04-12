//! Path derivation for output vlog + sidecar files.
//!
//! Rules:
//! - Default output: `<input-dir>/vlog-YYYY-MM-DD.mp4`
//! - Sidecar: same directory and stem as output, with `.decisions.json` extension
//! - `--out <path>` overrides the full output path; sidecar stem still follows it

use chrono::{DateTime, Local, Utc};
use std::path::{Path, PathBuf};

/// Return the default output path for a vlog produced from the given
/// input directory, dated to the local day.
pub(crate) fn default_output(input_dir: &Path, now: DateTime<Utc>) -> PathBuf {
    let local = now.with_timezone(&Local);
    let date = local.format("%Y-%m-%d");
    input_dir.join(format!("vlog-{date}.mp4"))
}

/// Return the sidecar path for a given output path.
///
/// `/foo/bar/vlog-2026-04-12.mp4` → `/foo/bar/vlog-2026-04-12.decisions.json`
pub(crate) fn sidecar_for(output: &Path) -> PathBuf {
    let stem = output
        .file_stem()
        .unwrap_or_else(|| std::ffi::OsStr::new("vlog"))
        .to_os_string();
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let mut name = stem;
    name.push(".decisions.json");
    parent.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn default_output_uses_local_date() -> TestResult {
        let fixed = Utc
            .with_ymd_and_hms(2026, 4, 12, 20, 0, 0)
            .single()
            .ok_or("bad timestamp")?;
        let out = default_output(Path::new("/tmp/clips"), fixed);
        let s = out.to_string_lossy();
        assert!(s.starts_with("/tmp/clips/vlog-"));
        assert!(s.ends_with(".mp4"));
        Ok(())
    }

    #[test]
    fn sidecar_for_simple_mp4() {
        let side = sidecar_for(Path::new("/foo/bar/vlog-2026-04-12.mp4"));
        assert_eq!(
            side,
            PathBuf::from("/foo/bar/vlog-2026-04-12.decisions.json")
        );
    }

    #[test]
    fn sidecar_for_custom_out() {
        let side = sidecar_for(Path::new("/movies/saturday.mp4"));
        assert_eq!(side, PathBuf::from("/movies/saturday.decisions.json"));
    }

    #[test]
    fn sidecar_for_no_extension() {
        let side = sidecar_for(Path::new("vlog"));
        assert_eq!(side, PathBuf::from("vlog.decisions.json"));
    }
}
