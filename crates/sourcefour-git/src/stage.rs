//! Staging and unstaging, through the user's own Git.
//!
//! Always explicit paths after a `--` separator — never `-A`, never a bare
//! `.`: the user edits the tree while Sourcefour is open, and only the rows
//! they acted on may move. These are instant index edits, so they run a
//! plain `Command` like the status read; the §10 runner is for operations
//! with progress and a meaningful cancel.

use std::process::Command;

use sourcefour_model::{RepoFailure, RepoLocation, RepoPath, UserFacingError};

use crate::operation::classify_stderr;

/// `git add -- <paths>`.
fn stage_argv(paths: &[RepoPath]) -> Vec<std::ffi::OsString> {
    argv_with_paths(&["add"], paths)
}

/// `git restore --staged -- <paths>`.
fn unstage_argv(paths: &[RepoPath]) -> Vec<std::ffi::OsString> {
    argv_with_paths(&["restore", "--staged"], paths)
}

fn argv_with_paths(command: &[&str], paths: &[RepoPath]) -> Vec<std::ffi::OsString> {
    let mut argv: Vec<std::ffi::OsString> = command.iter().map(std::ffi::OsString::from).collect();
    argv.push(std::ffi::OsString::from("--"));
    argv.extend(paths.iter().map(|path| path_as_os(&path.0)));
    argv
}

/// Repository-relative byte paths pass to Git byte-exact on Unix.
fn path_as_os(path: &[u8]) -> std::ffi::OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(path.to_vec())
    }
    #[cfg(not(unix))]
    {
        std::ffi::OsString::from(String::from_utf8_lossy(path).into_owned())
    }
}

/// Stages exactly `paths`.
///
/// # Errors
///
/// Returns a typed failure when Git cannot run or rejects the request.
pub fn stage_paths(location: &RepoLocation, paths: &[RepoPath]) -> Result<(), RepoFailure> {
    run_index_edit(location, stage_argv(paths), "Stage failed")
}

/// Unstages exactly `paths`.
///
/// # Errors
///
/// Returns a typed failure when Git cannot run or rejects the request.
pub fn unstage_paths(location: &RepoLocation, paths: &[RepoPath]) -> Result<(), RepoFailure> {
    run_index_edit(location, unstage_argv(paths), "Unstage failed")
}

fn run_index_edit(
    location: &RepoLocation,
    argv: Vec<std::ffi::OsString>,
    failed_title: &str,
) -> Result<(), RepoFailure> {
    let Some(worktree) = &location.active_worktree_path else {
        return Ok(());
    };
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(argv)
        .output()
        .map_err(|error| failure(failed_title, &error.to_string()))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(failure(failed_title, stderr.trim()))
}

fn failure(title: &str, detail: &str) -> RepoFailure {
    RepoFailure {
        kind: classify_stderr(detail),
        user: UserFacingError {
            title: title.to_owned(),
            message: detail.to_owned(),
            details: None,
            retryable: true,
        },
        operation: None,
        session: None,
        generation: None,
    }
}

#[cfg(test)]
mod tests {
    use sourcefour_model::RepoPath;
    use sourcefour_test_support::TempRepo;

    use super::{stage_argv, stage_paths, unstage_argv, unstage_paths};
    use crate::discover;

    fn paths(names: &[&str]) -> Vec<RepoPath> {
        names
            .iter()
            .map(|name| RepoPath(name.as_bytes().to_vec()))
            .collect()
    }

    #[test]
    fn argv_is_explicit_paths_after_a_separator_never_dash_a() {
        let argv = stage_argv(&paths(&["a.txt", "dir/b.rs"]));
        assert_eq!(argv, ["add", "--", "a.txt", "dir/b.rs"]);

        let argv = unstage_argv(&paths(&["-weird-name"]));
        assert_eq!(
            argv,
            ["restore", "--staged", "--", "-weird-name"],
            "the separator keeps hostile names from parsing as flags"
        );
    }

    #[test]
    fn staging_and_unstaging_move_one_file_and_only_it() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.commit("empty");
        std::fs::write(repository.path().join("wanted.txt"), "yes\n")?;
        std::fs::write(repository.path().join("bystander.txt"), "no\n")?;
        let location = discover(repository.path())?;

        stage_paths(&location, &paths(&["wanted.txt"]))?;
        let status = repository.git(&["status", "--porcelain"]);
        assert!(status.contains("A  wanted.txt"), "staged: {status}");
        assert!(
            status.contains("?? bystander.txt"),
            "the file not asked about must not move: {status}"
        );

        unstage_paths(&location, &paths(&["wanted.txt"]))?;
        let status = repository.git(&["status", "--porcelain"]);
        assert!(
            status.contains("?? wanted.txt"),
            "unstaged back to untracked: {status}"
        );
        Ok(())
    }
}
