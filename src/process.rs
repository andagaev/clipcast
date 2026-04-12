//! Shared subprocess runner used by every pipeline stage.
//!
//! Wraps `tokio::process::Command` with:
//! - `kill_on_drop(true)` so a dropped future kills the child
//! - Piped stdin/stdout/stderr
//! - An optional stdin payload written in a background tokio task so
//!   writing to stdin doesn't deadlock against the child writing to
//!   stdout
//! - Captures stdout + stderr into `Vec<u8>` on success, or into a
//!   `ProcessError::NonZeroExit` with the captured stderr on failure
//!
//! Based on the `spawn_optional_stdin_write` pattern from agentty at
//! `crates/agentty/src/infra/agent/cli/stdin.rs`.

use std::ffi::OsStr;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Errors returned from running a subprocess.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProcessError {
    /// The binary could not be spawned (not on PATH, permission denied,
    /// or similar OS-level failure).
    #[error("failed to spawn `{program}`: {source}")]
    SpawnFailed {
        program: String,
        #[source]
        source: std::io::Error,
    },

    /// Writing the stdin payload to the child failed for a reason other
    /// than the child closing its stdin early (broken pipes are
    /// silently tolerated).
    #[error("failed to write stdin for `{program}`: {source}")]
    StdinFailed {
        program: String,
        #[source]
        source: std::io::Error,
    },

    /// The child exited with a non-zero status code.
    #[error(
        "`{program}` exited with status {code}\n\
         stderr: {stderr}"
    )]
    NonZeroExit {
        program: String,
        code: i32,
        stderr: String,
    },

    /// The child was terminated by a signal (no exit code available).
    #[error("`{program}` was killed by a signal")]
    KilledBySignal { program: String },
}

/// Output captured from a successful subprocess run.
#[derive(Debug)]
pub(crate) struct Output {
    /// Raw bytes written to the child's stdout.
    pub(crate) stdout: Vec<u8>,
    /// Raw bytes written to the child's stderr.
    pub(crate) stderr: Vec<u8>,
}

/// Run a subprocess to completion, optionally piping `stdin_payload` to
/// its stdin. On success returns the captured stdout + stderr. On
/// failure returns a [`ProcessError`] with the program name and reason.
///
/// - `program`: binary name (resolved via PATH or absolute path).
/// - `args`: command-line arguments.
/// - `envs`: env vars to set IN ADDITION to inherited env. Pass an empty
///   iterator for none.
/// - `stdin_payload`: if `Some`, written to the child's stdin in a
///   background task (avoids deadlock if the child writes a lot to
///   stdout while we're still writing its stdin).
pub(crate) async fn run<I, S, E, K, V>(
    program: &str,
    args: I,
    envs: E,
    stdin_payload: Option<Vec<u8>>,
) -> Result<Output, ProcessError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
    E: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut cmd = Command::new(program);
    cmd.args(args)
        .envs(envs)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| ProcessError::SpawnFailed {
        program: program.to_string(),
        source: e,
    })?;

    // If there's a stdin payload, spawn a task that writes it and drops
    // the handle (closing stdin so the child sees EOF). Otherwise drop
    // stdin immediately so the child doesn't hang waiting on it.
    let stdin_task = if let Some(payload) = stdin_payload {
        let stdin = child.stdin.take();
        let program_owned = program.to_string();
        Some(tokio::spawn(async move {
            let Some(mut stdin) = stdin else {
                return Err(ProcessError::StdinFailed {
                    program: program_owned,
                    source: std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "child had no stdin",
                    ),
                });
            };
            if let Err(e) = stdin.write_all(&payload).await {
                // Broken pipe is OK — means the child closed its stdin
                // early (which is fine for e.g. streaming pipelines).
                if e.kind() != std::io::ErrorKind::BrokenPipe {
                    return Err(ProcessError::StdinFailed {
                        program: program_owned,
                        source: e,
                    });
                }
            }
            if let Err(e) = stdin.shutdown().await {
                if e.kind() != std::io::ErrorKind::BrokenPipe {
                    return Err(ProcessError::StdinFailed {
                        program: program_owned,
                        source: e,
                    });
                }
            }
            Ok(())
        }))
    } else {
        drop(child.stdin.take());
        None
    };

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| ProcessError::SpawnFailed {
            program: program.to_string(),
            source: e,
        })?;

    if let Some(task) = stdin_task {
        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_join_err) => {
                return Err(ProcessError::StdinFailed {
                    program: program.to_string(),
                    source: std::io::Error::other("stdin write task panicked"),
                });
            }
        }
    }

    let stderr_string = String::from_utf8_lossy(&output.stderr).into_owned();

    if let Some(code) = output.status.code() {
        if code != 0 {
            return Err(ProcessError::NonZeroExit {
                program: program.to_string(),
                code,
                stderr: stderr_string,
            });
        }
    } else {
        return Err(ProcessError::KilledBySignal {
            program: program.to_string(),
        });
    }

    Ok(Output {
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[tokio::test]
    async fn run_echoes_stdin_to_stdout_via_cat() -> TestResult {
        // `cat` with no args copies stdin to stdout — universally
        // available on macOS/Linux, perfect for a wiring test.
        let out = run(
            "cat",
            std::iter::empty::<&str>(),
            std::iter::empty::<(&str, &str)>(),
            Some(b"hello world\n".to_vec()),
        )
        .await?;
        assert_eq!(out.stdout, b"hello world\n");
        Ok(())
    }

    #[tokio::test]
    async fn run_captures_nonzero_exit_with_stderr() -> TestResult {
        let err = run(
            "sh",
            ["-c", "echo bad >&2; exit 7"],
            std::iter::empty::<(&str, &str)>(),
            None,
        )
        .await
        .err()
        .ok_or("expected error, got success")?;
        match err {
            ProcessError::NonZeroExit {
                program,
                code,
                stderr,
            } => {
                assert_eq!(program, "sh");
                assert_eq!(code, 7);
                assert!(stderr.contains("bad"), "stderr was: {stderr}");
            }
            other => return Err(format!("expected NonZeroExit, got {other:?}").into()),
        }
        Ok(())
    }

    #[tokio::test]
    async fn run_spawn_fails_for_missing_binary() -> TestResult {
        let err = run(
            "definitely-not-a-real-binary-xyzzy",
            std::iter::empty::<&str>(),
            std::iter::empty::<(&str, &str)>(),
            None,
        )
        .await
        .err()
        .ok_or("expected error, got success")?;
        assert!(
            matches!(err, ProcessError::SpawnFailed { .. }),
            "expected SpawnFailed, got {err:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn run_passes_envs_to_child() -> TestResult {
        let out = run(
            "sh",
            ["-c", "printf %s \"$MY_TEST_VAR\""],
            [("MY_TEST_VAR", "hello-from-env")],
            None,
        )
        .await?;
        assert_eq!(out.stdout, b"hello-from-env");
        Ok(())
    }

    #[tokio::test]
    async fn run_captures_stderr_on_success() -> TestResult {
        let out = run(
            "sh",
            ["-c", "echo to-stderr >&2; echo to-stdout"],
            std::iter::empty::<(&str, &str)>(),
            None,
        )
        .await?;
        assert_eq!(out.stdout, b"to-stdout\n");
        assert_eq!(out.stderr, b"to-stderr\n");
        Ok(())
    }
}
