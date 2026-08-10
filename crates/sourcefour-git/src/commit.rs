//! The commit operation, run through the user's own Git.
//!
//! `git commit -m <message>` through the §10 runner: hooks run and may be
//! slow, `commit.gpgsign` and identity configuration apply, and a failing
//! hook's own words surface verbatim.

use std::sync::atomic::AtomicBool;

use sourcefour_model::{OperationKind, OperationOutcome, RepoFailure, RepoLocation};

use crate::{
    OperationSink,
    operation::{GitOperation, classify_stderr, run},
};

/// The exact commit argv: the summary rides as one argument, never a shell.
fn commit_argv(message: &str) -> Vec<String> {
    vec![
        String::from("commit"),
        String::from("-m"),
        message.to_owned(),
    ]
}

/// Git's own first line — `[main abc1234] subject` — is the best summary.
fn summarize(stdout: &str) -> String {
    stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .map_or_else(|| String::from("Committed"), |line| line.trim().to_owned())
}

/// Runs `git commit -m <message>`, honoring cancellation.
///
/// # Errors
///
/// Returns a typed failure only when the process cannot be started; a commit
/// that ran and failed — a rejecting hook, a signing error — is an
/// [`OperationOutcome::Failed`], preserving the browsing session.
pub fn commit(
    location: &RepoLocation,
    message: &str,
    sink: &dyn OperationSink,
    cancelled: &AtomicBool,
) -> Result<OperationOutcome, RepoFailure> {
    run(
        &GitOperation {
            kind: OperationKind::Commit,
            argv: commit_argv(message),
            summarize: &summarize,
            classify: &classify_stderr,
            failed_title: "Commit failed",
        },
        location,
        sink,
        cancelled,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, atomic::AtomicBool};

    use sourcefour_model::{OperationOutcome, OperationProgress};
    use sourcefour_test_support::TempRepo;

    use super::{commit, commit_argv, summarize};
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
    fn the_summary_rides_as_one_argument() {
        assert_eq!(
            commit_argv("fix: spaces & symbols; no shell"),
            ["commit", "-m", "fix: spaces & symbols; no shell"]
        );
    }

    #[test]
    fn a_multiline_message_rides_as_one_argument() {
        assert_eq!(
            commit_argv("subject\n\nbody"),
            ["commit", "-m", "subject\n\nbody"]
        );
    }

    #[test]
    fn gits_first_line_becomes_the_summary() {
        assert_eq!(
            summarize("[main 1234abc] add the thing\n 1 file changed\n"),
            "[main 1234abc] add the thing"
        );
        assert_eq!(summarize(""), "Committed");
    }

    #[test]
    fn a_staged_file_commits_and_head_advances() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.commit("base");
        let before = repository.git(&["rev-parse", "HEAD"]);
        std::fs::write(repository.path().join("work.txt"), "done\n")?;
        repository.git(&["add", "work.txt"]);
        let sink = CollectingSink(Mutex::new(Vec::new()));

        let outcome = commit(
            &discover(repository.path())?,
            "feat: the work",
            &sink,
            &AtomicBool::new(false),
        )?;

        assert!(
            matches!(outcome, OperationOutcome::Succeeded { .. }),
            "{outcome:?}"
        );
        assert_ne!(repository.git(&["rev-parse", "HEAD"]), before);
        assert_eq!(
            repository.git(&["log", "-1", "--format=%s"]),
            "feat: the work"
        );
        Ok(())
    }

    #[test]
    fn a_rejecting_hook_fails_with_its_own_words() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let repository = TempRepo::init();
        repository.commit("base");
        let hooks = repository.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks)?;
        let hook = hooks.join("pre-commit");
        std::fs::write(&hook, "#!/bin/sh\necho 'the hook says no' >&2\nexit 1\n")?;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
        std::fs::write(repository.path().join("work.txt"), "blocked\n")?;
        repository.git(&["add", "work.txt"]);
        let sink = CollectingSink(Mutex::new(Vec::new()));

        let outcome = commit(
            &discover(repository.path())?,
            "never lands",
            &sink,
            &AtomicBool::new(false),
        )?;

        let OperationOutcome::Failed { error, .. } = outcome else {
            panic!("a rejecting hook must fail the commit: {outcome:?}");
        };
        assert!(
            error.user.message.contains("the hook says no"),
            "the hook's words surface: {}",
            error.user.message
        );
        Ok(())
    }
}
