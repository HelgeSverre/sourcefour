//! The fetch operation, run through the user's own Git (§6.12).
//!
//! Running the installed `git` inherits the user's credential helpers, SSH
//! configuration, URL rewriting, and hooks — none of which a gix fetch would
//! see. Progress arrives on stderr, machine-readable ref updates on stdout.

use std::sync::atomic::AtomicBool;

use sourcefour_model::{FetchRequest, OperationKind, OperationOutcome, RepoFailure, RepoLocation};

use crate::{
    OperationSink,
    operation::{GitOperation, classify_stderr, run},
};

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
    run(
        &GitOperation {
            kind: OperationKind::Fetch,
            argv: fetch_argv(request),
            summarize: &summarize_porcelain,
            classify: &classify_stderr,
            failed_title: "Fetch failed",
        },
        location,
        sink,
        cancelled,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, atomic::AtomicBool};

    use sourcefour_model::{
        FetchRequest, OperationOutcome, OperationProgress, RepoFailureKind, WorktreeId,
    };
    use sourcefour_test_support::TempRepo;

    use super::{fetch, fetch_argv, summarize_porcelain};
    use crate::{OperationSink, discover, operation::classify_stderr};

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
