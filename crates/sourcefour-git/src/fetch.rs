//! The fetch operation, run through the user's own Git (§6.12).
//!
//! Running the installed `git` inherits the user's credential helpers, SSH
//! configuration, URL rewriting, and hooks — none of which a gix fetch would
//! see. Progress arrives on stderr, machine-readable ref updates on stdout.

use std::{
    io::Read,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use sourcefour_model::{
    FetchRequest, OperationKind, OperationOutcome, OperationProgress, RepoFailure, RepoFailureKind,
    RepoLocation,
};

use crate::OperationSink;

/// Progress is forwarded at most this often (§6.12: rate-limited).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

/// The exact fetch argv (§12.5 pins it): progress on stderr, porcelain ref
/// updates on stdout, remote last when one is named.
fn fetch_argv(request: &FetchRequest) -> Vec<String> {
    let mut argv = vec![
        String::from("fetch"),
        String::from("--progress"),
        String::from("--porcelain"),
    ];
    if request.prune {
        argv.push(String::from("--prune"));
    }
    if let Some(remote) = &request.remote {
        argv.push(remote.clone());
    }
    argv
}

/// Maps Git's stderr onto the failure taxonomy (§6.12).
fn classify_stderr(stderr: &str) -> RepoFailureKind {
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

/// One line per updated ref in porcelain output; empty means up to date.
fn summarize_porcelain(stdout: &str) -> String {
    let updated = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    match updated {
        0 => String::from("Already up to date"),
        1 => String::from("1 ref updated"),
        refs => format!("{refs} refs updated"),
    }
}

/// Runs `git fetch`, forwarding rate-limited progress and honoring
/// cancellation by terminating and reaping the child (§6.12).
///
/// # Errors
///
/// Returns a typed failure only when the process cannot be started; a fetch
/// that ran and failed is an [`OperationOutcome::Failed`], preserving the
/// browsing session.
pub fn fetch(
    location: &RepoLocation,
    request: &FetchRequest,
    sink: &dyn OperationSink,
    cancelled: &AtomicBool,
) -> Result<OperationOutcome, RepoFailure> {
    if cancelled.load(Ordering::Acquire) {
        return Ok(OperationOutcome::Cancelled {
            kind: OperationKind::Fetch,
        });
    }
    let workdir = location
        .active_worktree_path
        .as_deref()
        .unwrap_or(&location.common_dir);
    let mut child = Command::new("git")
        .args(fetch_argv(request))
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

    let (stderr_text, was_cancelled) = pump_stderr(&mut child, sink, cancelled);

    let output = child.wait_with_output().map_err(|error| {
        RepoFailure::new(
            RepoFailureKind::Internal,
            "Git did not finish",
            "The fetch process could not be reaped.",
        )
        .with_details(error.to_string())
    })?;

    if was_cancelled {
        return Ok(OperationOutcome::Cancelled {
            kind: OperationKind::Fetch,
        });
    }
    if output.status.success() {
        Ok(OperationOutcome::Succeeded {
            kind: OperationKind::Fetch,
            summary: summarize_porcelain(&String::from_utf8_lossy(&output.stdout)),
        })
    } else {
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
            kind: OperationKind::Fetch,
            error: RepoFailure::new(
                classify_stderr(&stderr_text),
                "Fetch failed",
                if tail.is_empty() {
                    String::from("Git reported an error without output.")
                } else {
                    tail
                },
            ),
        })
    }
}

/// Streams stderr, forwarding rate-limited progress lines; returns the
/// collected text and whether cancellation killed the child.
///
/// Progress lines arrive `\r`-terminated while a phase counts up, so this
/// reads bytewise instead of waiting for the newline at phase end.
fn pump_stderr(
    child: &mut std::process::Child,
    sink: &dyn OperationSink,
    cancelled: &AtomicBool,
) -> (String, bool) {
    let mut stderr = child.stderr.take();
    let mut stderr_text = String::new();
    let mut line = String::new();
    let mut last_report: Option<Instant> = None;
    let mut buffer = [0_u8; 512];
    let Some(pipe) = stderr.as_mut() else {
        return (stderr_text, false);
    };
    loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = child.kill();
            return (stderr_text, true);
        }
        let Ok(read) = pipe.read(&mut buffer) else {
            break;
        };
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
                    sink.report(parse_progress(line.trim()));
                    last_report = Some(Instant::now());
                }
                line.clear();
            } else {
                line.push(character);
            }
        }
    }
    (stderr_text, false)
}

/// Extracts `completed`/`total` from lines like
/// `Receiving objects:  42% (123/292)`.
fn parse_progress(line: &str) -> OperationProgress {
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
        kind: OperationKind::Fetch,
        message: line.to_owned(),
        completed: counts.map(|(done, _)| done),
        total: counts.map(|(_, total)| total),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, atomic::AtomicBool};

    use sourcefour_model::{
        FetchRequest, OperationOutcome, OperationProgress, RepoFailureKind, WorktreeId,
    };
    use sourcefour_test_support::TempRepo;

    use super::{classify_stderr, fetch, fetch_argv, summarize_porcelain};
    use crate::{OperationSink, discover};

    struct CollectingSink(Mutex<Vec<OperationProgress>>);

    impl OperationSink for CollectingSink {
        fn report(&self, progress: OperationProgress) {
            self.0
                .lock()
                .expect("sink lock is never poisoned")
                .push(progress);
        }
    }

    fn request(remote: Option<&str>, prune: bool) -> FetchRequest {
        FetchRequest {
            worktree: WorktreeId(String::from("test")),
            remote: remote.map(str::to_owned),
            prune,
        }
    }

    #[test]
    fn the_exact_argv_is_stable() {
        assert_eq!(
            fetch_argv(&request(None, false)),
            ["fetch", "--progress", "--porcelain"],
            "no remote means all-remotes default"
        );
        assert_eq!(
            fetch_argv(&request(Some("origin"), true)),
            ["fetch", "--progress", "--porcelain", "--prune", "origin"]
        );
    }

    #[test]
    fn stderr_classifies_auth_and_network_failures() {
        assert_eq!(
            classify_stderr("fatal: Authentication failed for 'https://…'"),
            RepoFailureKind::Authentication
        );
        assert_eq!(
            classify_stderr("git@github.com: Permission denied (publickey)."),
            RepoFailureKind::Authentication
        );
        assert_eq!(
            classify_stderr("fatal: unable to access '…': Could not resolve host: github.com"),
            RepoFailureKind::Network
        );
        assert_eq!(
            classify_stderr("fatal: something else"),
            RepoFailureKind::Network
        );
    }

    #[test]
    fn porcelain_output_summarizes_ref_updates() {
        let output = "\
* 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 refs/remotes/origin/new\n\
  2222222222222222222222222222222222222222 3333333333333333333333333333333333333333 refs/remotes/origin/main\n";
        assert_eq!(summarize_porcelain(output), "2 refs updated");
        assert_eq!(summarize_porcelain(""), "Already up to date");
    }

    #[test]
    fn fetching_from_a_local_origin_updates_tracking_refs() -> Result<(), Box<dyn std::error::Error>>
    {
        let origin = TempRepo::init();
        let clone = TempRepo::clone_of(&origin);
        origin.commit("made upstream while cloned");
        let sink = CollectingSink(Mutex::new(Vec::new()));

        let outcome = fetch(
            &discover(clone.path())?,
            &request(Some("origin"), false),
            &sink,
            &AtomicBool::new(false),
        )?;

        assert!(
            matches!(outcome, OperationOutcome::Succeeded { .. }),
            "fetch from a file:// origin succeeds offline: {outcome:?}"
        );
        assert_eq!(
            clone.git(&["rev-parse", "origin/main"]),
            origin.git(&["rev-parse", "main"]),
            "the tracking ref moved to the upstream tip"
        );
        Ok(())
    }

    #[test]
    fn a_cancelled_fetch_reports_cancelled() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let clone = TempRepo::clone_of(&origin);
        let sink = CollectingSink(Mutex::new(Vec::new()));

        let outcome = fetch(
            &discover(clone.path())?,
            &request(Some("origin"), false),
            &sink,
            &AtomicBool::new(true),
        )?;

        assert!(matches!(outcome, OperationOutcome::Cancelled { .. }));
        Ok(())
    }
}
