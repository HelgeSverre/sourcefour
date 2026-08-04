//! The shared runner for network operations through the user's own Git.
//!
//! Fetch, push, and pull differ only in argv, how success summarizes, and
//! how a failure classifies; the spawn/progress/cancel/reap machinery here
//! is written once (§6.12).

use std::{
    io::Read,
    process::{Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use sourcefour_model::{
    OperationKind, OperationOutcome, OperationProgress, RepoFailure, RepoFailureKind, RepoLocation,
};

use crate::OperationSink;

/// Progress is forwarded at most this often (§6.12: rate-limited).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

/// How one operation runs and phrases its outcomes.
pub(crate) struct GitOperation<'a> {
    pub(crate) kind: OperationKind,
    pub(crate) argv: Vec<String>,
    /// Phrases success from the process stdout.
    pub(crate) summarize: &'a dyn Fn(&str) -> String,
    /// Classifies a failure from the combined stderr and stdout.
    pub(crate) classify: &'a dyn Fn(&str) -> RepoFailureKind,
    /// The failure headline, e.g. "Push failed".
    pub(crate) failed_title: &'a str,
}

/// Runs the operation, forwarding rate-limited progress and honoring
/// cancellation by terminating and reaping the child (§6.12).
///
/// # Errors
///
/// Returns a typed failure only when the process cannot be started or
/// reaped; a run that failed is an [`OperationOutcome::Failed`], preserving
/// the browsing session.
pub(crate) fn run(
    operation: &GitOperation<'_>,
    location: &RepoLocation,
    sink: &dyn OperationSink,
    cancelled: &AtomicBool,
) -> Result<OperationOutcome, RepoFailure> {
    if cancelled.load(Ordering::Acquire) {
        return Ok(OperationOutcome::Cancelled {
            kind: operation.kind,
        });
    }
    let workdir = location
        .active_worktree_path
        .as_deref()
        .unwrap_or(&location.common_dir);
    let mut child = Command::new("git")
        .args(&operation.argv)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            RepoFailure::new(
                RepoFailureKind::Internal,
                "Git could not be started",
                "The installed git executable could not be run.",
            )
            .with_details(error.to_string())
        })?;

    // Both pipes must drain concurrently: a child whose stdout fills the OS
    // pipe buffer blocks in write(2), produces no more stderr, and never
    // exits. Stdout gets its own reader thread while this thread pumps
    // stderr for progress; a watcher thread kills the child on cancellation
    // so Cancel works even while every pipe is silent.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let child = Mutex::new(child);
    let finished = AtomicBool::new(false);
    let (stderr_text, stdout_bytes) = std::thread::scope(|scope| {
        let stdout_reader = scope.spawn(move || {
            let mut buffer = Vec::new();
            if let Some(mut pipe) = stdout_pipe {
                pipe.read_to_end(&mut buffer).ok();
            }
            buffer
        });
        scope.spawn(|| {
            while !finished.load(Ordering::Acquire) {
                if cancelled.load(Ordering::Acquire) {
                    if let Ok(mut child) = child.lock() {
                        child.kill().ok();
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let stderr_text = pump_stderr(stderr_pipe, operation.kind, sink);
        // Killed or exited, the pipes have closed; release the watcher.
        finished.store(true, Ordering::Release);
        let stdout_bytes = stdout_reader.join().unwrap_or_default();
        (stderr_text, stdout_bytes)
    });

    let status = reap(child)?;

    if cancelled.load(Ordering::Acquire) {
        return Ok(OperationOutcome::Cancelled {
            kind: operation.kind,
        });
    }
    let stdout_text = String::from_utf8_lossy(&stdout_bytes);
    if status.success() {
        Ok(OperationOutcome::Succeeded {
            kind: operation.kind,
            summary: (operation.summarize)(&stdout_text),
        })
    } else {
        // Conflict details land on stdout while the advice lands on stderr,
        // so both feed classification and the shown tail.
        let combined = format!("{stderr_text}\n{stdout_text}");
        let tail: String = stderr_text
            .lines()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        Ok(OperationOutcome::Failed {
            kind: operation.kind,
            error: RepoFailure::new(
                (operation.classify)(&combined),
                operation.failed_title,
                if tail.is_empty() {
                    String::from("Git reported an error without output.")
                } else {
                    tail
                },
            ),
        })
    }
}

/// Waits the finished (or killed) child out of its mutex.
fn reap(child: Mutex<std::process::Child>) -> Result<std::process::ExitStatus, RepoFailure> {
    child
        .into_inner()
        .map_err(|_| {
            RepoFailure::new(
                RepoFailureKind::Internal,
                "Git did not finish",
                "The operation's watcher thread panicked.",
            )
        })?
        .wait()
        .map_err(|error| {
            RepoFailure::new(
                RepoFailureKind::Internal,
                "Git did not finish",
                "The operation process could not be reaped.",
            )
            .with_details(error.to_string())
        })
}

/// Maps Git's words onto the failure taxonomy shared by every network
/// operation (§6.12).
pub(crate) fn classify_stderr(stderr: &str) -> RepoFailureKind {
    let lowered = stderr.to_lowercase();
    if lowered.contains("authentication failed")
        || lowered.contains("permission denied")
        || lowered.contains("could not read username")
        || lowered.contains("could not read password")
    {
        RepoFailureKind::Authentication
    } else {
        RepoFailureKind::Network
    }
}

/// Streams stderr until it closes, forwarding rate-limited progress lines;
/// returns the collected text. Cancellation is the watcher thread's job —
/// killing the child closes this pipe and ends the loop.
///
/// Progress lines arrive `\r`-terminated while a phase counts up, so this
/// reads bytewise instead of waiting for the newline at phase end.
fn pump_stderr(
    pipe: Option<std::process::ChildStderr>,
    kind: OperationKind,
    sink: &dyn OperationSink,
) -> String {
    let mut stderr_text = String::new();
    let mut line = String::new();
    let mut last_report: Option<Instant> = None;
    let mut buffer = [0_u8; 512];
    let Some(mut pipe) = pipe else {
        return stderr_text;
    };
    while let Ok(read) = pipe.read(&mut buffer) {
        if read == 0 {
            break;
        }
        let chunk = String::from_utf8_lossy(&buffer[..read]).into_owned();
        stderr_text.push_str(&chunk);
        for character in chunk.chars() {
            if character == '\r' || character == '\n' {
                let due =
                    last_report.is_none_or(|reported| reported.elapsed() >= PROGRESS_INTERVAL);
                if !line.trim().is_empty() && due {
                    sink.report(parse_progress(kind, line.trim()));
                    last_report = Some(Instant::now());
                }
                line.clear();
            } else {
                line.push(character);
            }
        }
    }
    stderr_text
}

/// Extracts `completed`/`total` from lines like
/// `Receiving objects:  42% (123/292)`.
fn parse_progress(kind: OperationKind, line: &str) -> OperationProgress {
    let counts = line
        .rsplit_once('(')
        .and_then(|(_, tail)| tail.split_once(')'))
        .map(|(inside, _)| inside)
        .and_then(|inside| inside.split_once('/'))
        .and_then(|(done, total)| {
            Some((
                done.trim().parse::<u64>().ok()?,
                total.trim().parse::<u64>().ok()?,
            ))
        });
    OperationProgress {
        kind,
        message: line.to_owned(),
        completed: counts.map(|(done, _)| done),
        total: counts.map(|(_, total)| total),
    }
}
