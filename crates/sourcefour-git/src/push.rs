//! The push operation, run through the user's own Git.
//!
//! A bare `git push` so the user's `push.default` and upstream configuration
//! decide what moves — the same philosophy as fetch (§6.12). A branch with
//! no upstream fails with Git's own words, which name the exact command to
//! set one.

use std::sync::atomic::AtomicBool;

use sourcefour_model::{
    OperationKind, OperationOutcome, RepoFailure, RepoFailureKind, RepoLocation,
};

use crate::{
    OperationSink,
    operation::{GitOperation, classify_stderr, run},
};

/// The exact push argv (§12.5 pins it): progress on stderr, porcelain ref
/// results on stdout, everything else decided by the user's configuration.
fn push_argv() -> Vec<String> {
    vec![
        String::from("push"),
        String::from("--progress"),
        String::from("--porcelain"),
    ]
}

/// One `<flag>\t<refspec>\t<summary>` line per ref in porcelain output;
/// `=` flags mean the ref was already up to date.
fn summarize_porcelain(stdout: &str) -> String {
    let pushed = stdout
        .lines()
        .filter(|line| line.contains('\t') && !line.starts_with('='))
        .count();
    match pushed {
        0 => String::from("Everything up to date"),
        1 => String::from("1 ref pushed"),
        refs => format!("{refs} refs pushed"),
    }
}

/// A rejected push is a state conflict to resolve, not a broken network.
fn classify(combined: &str) -> RepoFailureKind {
    if combined.contains("[rejected]")
        || combined.contains("non-fast-forward")
        || combined.contains("no upstream branch")
    {
        RepoFailureKind::OperationConflict
    } else {
        classify_stderr(combined)
    }
}

/// Runs `git push`, forwarding rate-limited progress and honoring
/// cancellation (§6.12).
///
/// # Errors
///
/// Returns a typed failure only when the process cannot be started; a push
/// that ran and failed — rejected, no upstream, offline — is an
/// [`OperationOutcome::Failed`], preserving the browsing session.
pub fn push(
    location: &RepoLocation,
    sink: &dyn OperationSink,
    cancelled: &AtomicBool,
) -> Result<OperationOutcome, RepoFailure> {
    run(
        &GitOperation {
            kind: OperationKind::Push,
            argv: push_argv(),
            summarize: &summarize_porcelain,
            classify: &classify,
            failed_title: "Push failed",
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

    use super::{classify, push, push_argv, summarize_porcelain};
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
        assert_eq!(push_argv(), ["push", "--progress", "--porcelain"]);
    }

    #[test]
    fn porcelain_output_summarizes_pushed_refs() {
        let pushed = "To /tmp/origin.git\n\
             \trefs/heads/main:refs/heads/main\t1111111..2222222\n\
            Done\n";
        assert_eq!(summarize_porcelain(pushed), "1 ref pushed");

        let up_to_date =
            "To /tmp/origin.git\n=\trefs/heads/main:refs/heads/main\t[up to date]\nDone\n";
        assert_eq!(summarize_porcelain(up_to_date), "Everything up to date");
        assert_eq!(summarize_porcelain(""), "Everything up to date");
    }

    #[test]
    fn rejections_classify_as_conflicts_not_network() {
        assert_eq!(
            classify("!\trefs/heads/main:refs/heads/main\t[rejected] (fetch first)"),
            RepoFailureKind::OperationConflict
        );
        assert_eq!(
            classify("fatal: The current branch x has no upstream branch."),
            RepoFailureKind::OperationConflict
        );
        assert_eq!(
            classify("fatal: Authentication failed for 'https://…'"),
            RepoFailureKind::Authentication
        );
        assert_eq!(
            classify("fatal: Could not resolve host: github.com"),
            RepoFailureKind::Network
        );
    }

    #[test]
    fn pushing_to_a_local_origin_advances_it() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init_bare();
        let clone = TempRepo::clone_of(&origin);
        clone.commit("made locally");
        let sink = CollectingSink(Mutex::new(Vec::new()));

        let outcome = push(&discover(clone.path())?, &sink, &AtomicBool::new(false))?;

        assert!(
            matches!(outcome, OperationOutcome::Succeeded { .. }),
            "push to a file:// origin succeeds offline: {outcome:?}"
        );
        assert_eq!(
            origin.git(&["rev-parse", "main"]),
            clone.git(&["rev-parse", "main"]),
            "the origin's branch moved to the local tip"
        );
        Ok(())
    }

    #[test]
    fn a_non_fast_forward_push_reports_a_conflict() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init_bare();
        let clone = TempRepo::clone_of(&origin);
        let other = TempRepo::clone_of(&origin);
        other.commit("upstream moved first");
        other.git(&["push"]);
        clone.commit("diverging local work");
        let sink = CollectingSink(Mutex::new(Vec::new()));

        let outcome = push(&discover(clone.path())?, &sink, &AtomicBool::new(false))?;

        let OperationOutcome::Failed { error, .. } = outcome else {
            panic!("a non-fast-forward push must fail: {outcome:?}");
        };
        assert_eq!(error.kind, RepoFailureKind::OperationConflict);
        Ok(())
    }

    #[test]
    fn a_cancelled_push_reports_cancelled() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init_bare();
        let clone = TempRepo::clone_of(&origin);
        let sink = CollectingSink(Mutex::new(Vec::new()));

        let outcome = push(&discover(clone.path())?, &sink, &AtomicBool::new(true))?;

        assert!(matches!(outcome, OperationOutcome::Cancelled { .. }));
        Ok(())
    }
}
