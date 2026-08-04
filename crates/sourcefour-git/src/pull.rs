//! The pull operation, run through the user's own Git.
//!
//! A bare `git pull` so the user's `pull.rebase`/`pull.ff` configuration
//! decides how histories reconcile — the same philosophy as fetch (§6.12).
//! Conflicts and divergent-branch refusals surface with Git's own words;
//! resolving them stays in the user's hands for v1.

use std::sync::atomic::AtomicBool;

use sourcefour_model::{
    OperationKind, OperationOutcome, RepoFailure, RepoFailureKind, RepoLocation,
};

use crate::{
    OperationSink,
    operation::{GitOperation, classify_stderr, run},
};

/// The exact pull argv (§12.5 pins it); no reconciliation flags, because
/// the user's configuration owns that decision.
fn pull_argv() -> Vec<String> {
    vec![String::from("pull"), String::from("--progress")]
}

/// Phrases success from pull's human stdout: the merge stat line when files
/// changed, Git's own words when there was nothing to do.
fn summarize_stdout(stdout: &str) -> String {
    if stdout.contains("Already up to date") {
        return String::from("Already up to date");
    }
    stdout
        .lines()
        .map(str::trim)
        .find(|line| line.contains("changed"))
        .map_or_else(|| String::from("Pull complete"), str::to_owned)
}

/// Conflicts and reconciliation refusals are state to resolve, not a broken
/// network.
fn classify(combined: &str) -> RepoFailureKind {
    if combined.contains("CONFLICT")
        || combined.contains("Automatic merge failed")
        || combined.contains("divergent branches")
    {
        RepoFailureKind::OperationConflict
    } else {
        classify_stderr(combined)
    }
}

/// Runs `git pull`, forwarding rate-limited progress and honoring
/// cancellation (§6.12).
///
/// # Errors
///
/// Returns a typed failure only when the process cannot be started; a pull
/// that ran and failed — conflicts, divergence, offline — is an
/// [`OperationOutcome::Failed`], preserving the browsing session.
pub fn pull(
    location: &RepoLocation,
    sink: &dyn OperationSink,
    cancelled: &AtomicBool,
) -> Result<OperationOutcome, RepoFailure> {
    run(
        &GitOperation {
            kind: OperationKind::Pull,
            argv: pull_argv(),
            summarize: &summarize_stdout,
            classify: &classify,
            failed_title: "Pull failed",
        },
        location,
        sink,
        cancelled,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, atomic::AtomicBool};

    use sourcefour_model::{OperationOutcome, OperationProgress, RepoFailureKind};
    use sourcefour_test_support::TempRepo;

    use super::{classify, pull, pull_argv, summarize_stdout};
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

    #[test]
    fn the_exact_argv_is_stable() {
        assert_eq!(pull_argv(), ["pull", "--progress"]);
    }

    #[test]
    fn stdout_summarizes_the_merge() {
        assert_eq!(
            summarize_stdout(
                "Updating 1111111..2222222\nFast-forward\n a.txt | 2 +-\n 1 file changed, 1 insertion(+), 1 deletion(-)\n"
            ),
            "1 file changed, 1 insertion(+), 1 deletion(-)"
        );
        assert_eq!(
            summarize_stdout("Already up to date.\n"),
            "Already up to date"
        );
        assert_eq!(summarize_stdout(""), "Pull complete");
    }

    #[test]
    fn conflicts_and_divergence_classify_as_conflicts() {
        assert_eq!(
            classify(
                "CONFLICT (content): Merge conflict in a.txt\nAutomatic merge failed; fix conflicts and then commit the result."
            ),
            RepoFailureKind::OperationConflict
        );
        assert_eq!(
            classify("fatal: Need to specify how to reconcile divergent branches."),
            RepoFailureKind::OperationConflict
        );
        assert_eq!(
            classify("fatal: Could not resolve host: github.com"),
            RepoFailureKind::Network
        );
    }

    #[test]
    fn pulling_from_a_local_origin_advances_the_clone() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let clone = TempRepo::clone_of(&origin);
        origin.commit("made upstream while cloned");
        let sink = CollectingSink(Mutex::new(Vec::new()));

        let outcome = pull(&discover(clone.path())?, &sink, &AtomicBool::new(false))?;

        assert!(
            matches!(outcome, OperationOutcome::Succeeded { .. }),
            "pull from a file:// origin succeeds offline: {outcome:?}"
        );
        assert_eq!(
            clone.git(&["rev-parse", "main"]),
            origin.git(&["rev-parse", "main"]),
            "the clone's branch moved to the upstream tip"
        );
        Ok(())
    }

    #[test]
    fn a_conflicting_pull_reports_a_conflict_and_keeps_the_session()
    -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let clone = TempRepo::clone_of(&origin);
        // Merge behavior pinned so the fixture conflicts the same way under
        // any user configuration.
        clone.git(&["config", "pull.rebase", "false"]);
        std::fs::write(origin.path().join("shared.txt"), "upstream words\n")?;
        origin.git(&["add", "."]);
        origin.commit("upstream edit");
        std::fs::write(clone.path().join("shared.txt"), "local words\n")?;
        clone.git(&["add", "."]);
        clone.commit("local edit");
        let sink = CollectingSink(Mutex::new(Vec::new()));

        let outcome = pull(&discover(clone.path())?, &sink, &AtomicBool::new(false))?;

        let OperationOutcome::Failed { error, .. } = outcome else {
            panic!("a conflicting pull must fail: {outcome:?}");
        };
        assert_eq!(error.kind, RepoFailureKind::OperationConflict);
        // The repository is mid-merge now; abort so the fixture drops clean.
        clone.git(&["merge", "--abort"]);
        Ok(())
    }

    #[test]
    fn a_cancelled_pull_reports_cancelled() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let clone = TempRepo::clone_of(&origin);
        let sink = CollectingSink(Mutex::new(Vec::new()));

        let outcome = pull(&discover(clone.path())?, &sink, &AtomicBool::new(true))?;

        assert!(matches!(outcome, OperationOutcome::Cancelled { .. }));
        Ok(())
    }
}
