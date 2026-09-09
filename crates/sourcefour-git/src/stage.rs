//! Staging and unstaging, through the user's own Git.
//!
//! Always explicit, literal paths from a NUL-delimited input file: the user
//! edits the tree while Sourcefour is open, and only the rows they acted on
//! may move. File input avoids the OS argument limit on large selections.
//! These are instant index edits, so they run a plain `Command` like the
//! status read; the §10 runner is for operations
//! with progress and a meaningful cancel.

use std::{
    io::{BufWriter, Seek, Write},
    process::Command,
};

use sourcefour_model::{RepoFailure, RepoLocation, RepoPath, UserFacingError};

use crate::operation::classify_stderr;

/// Spool paths before launching Git, keeping memory and pipe backpressure bounded.
fn pathspec_input(paths: &[RepoPath]) -> std::io::Result<std::fs::File> {
    let mut input = BufWriter::new(tempfile::tempfile()?);
    for path in paths {
        if path.0.is_empty() || path.0.contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Git paths must be nonempty and contain no NUL bytes",
            ));
        }
        input.write_all(&path.0)?;
        input.write_all(&[0])?;
    }
    let mut input = input
        .into_inner()
        .map_err(std::io::IntoInnerError::into_error)?;
    input.rewind()?;
    Ok(input)
}

/// Stages exactly `paths`.
///
/// # Errors
///
/// Returns a typed failure when Git cannot run or rejects the request.
pub fn stage_paths(location: &RepoLocation, paths: &[RepoPath]) -> Result<(), RepoFailure> {
    run_index_edit(location, &["add"], paths, "Stage failed")
}

/// Unstages exactly `paths`.
///
/// # Errors
///
/// Returns a typed failure when Git cannot run or rejects the request.
pub fn unstage_paths(location: &RepoLocation, paths: &[RepoPath]) -> Result<(), RepoFailure> {
    run_index_edit(location, &["restore", "--staged"], paths, "Unstage failed")
}

fn run_index_edit(
    location: &RepoLocation,
    command: &[&str],
    paths: &[RepoPath],
    failed_title: &str,
) -> Result<(), RepoFailure> {
    if paths.is_empty() {
        return Ok(());
    }
    let Some(worktree) = &location.active_worktree_path else {
        return Ok(());
    };
    let input = pathspec_input(paths).map_err(|error| failure(failed_title, &error.to_string()))?;
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("--literal-pathspecs")
        .args(command)
        .args(["--pathspec-from-file=-", "--pathspec-file-nul"])
        .stdin(input)
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

    use super::{stage_paths, unstage_paths};
    use crate::discover;

    fn paths(names: &[&str]) -> Vec<RepoPath> {
        names
            .iter()
            .map(|name| RepoPath(name.as_bytes().to_vec()))
            .collect()
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

    #[test]
    fn bulk_staging_and_unstaging_exceed_the_os_argument_limit()
    -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let names: Vec<String> = (0..5000)
            .map(|index| format!("file-{index:05}-{}.txt", "x".repeat(120)))
            .collect();
        for name in &names {
            std::fs::write(repository.path().join(name), "icon\n")?;
        }
        std::fs::write(repository.path().join("bystander.txt"), "leave alone\n")?;
        let paths: Vec<RepoPath> = names
            .iter()
            .map(|name| RepoPath(name.as_bytes().to_vec()))
            .collect();
        assert!(paths.iter().map(|path| path.0.len() + 1).sum::<usize>() > 512 * 1024);
        let location = discover(repository.path())?;

        stage_paths(&location, &paths)?;
        let status = crate::working_tree_status(&location)?;
        assert_eq!(status.staged.len(), paths.len());
        assert_eq!(status.unstaged.len(), 1);
        assert_eq!(
            status.unstaged[0].new_path.as_ref(),
            Some(&RepoPath(b"bystander.txt".to_vec()))
        );

        unstage_paths(&location, &paths)?;
        let status = crate::working_tree_status(&location)?;
        assert!(status.staged.is_empty());
        assert_eq!(status.unstaged.len(), paths.len() + 1);
        Ok(())
    }

    #[test]
    fn path_file_preserves_literal_names_and_empty_selections_do_nothing()
    -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let names = [
            "-dash.txt",
            "has space.txt",
            #[cfg(unix)]
            "line\nbreak.txt",
            "literal[1].txt",
            #[cfg(unix)]
            ":(glob)*.txt",
        ];
        for name in names.into_iter().chain(["literal1.txt", "bystander.txt"]) {
            std::fs::write(repository.path().join(name), "content\n")?;
        }
        let location = discover(repository.path())?;
        stage_paths(&location, &[])?;
        assert!(crate::working_tree_status(&location)?.staged.is_empty());
        stage_paths(&location, &paths(&names))?;
        let status = crate::working_tree_status(&location)?;
        assert_eq!(status.staged.len(), names.len());
        assert_eq!(status.unstaged.len(), 2);
        unstage_paths(&location, &[])?;
        assert_eq!(crate::working_tree_status(&location)?, status);
        unstage_paths(&location, &paths(&names))?;
        assert!(crate::working_tree_status(&location)?.staged.is_empty());
        Ok(())
    }

    #[test]
    fn path_file_preserves_non_utf8_bytes() -> Result<(), Box<dyn std::error::Error>> {
        use std::io::Read as _;
        // APFS cannot create non-UTF-8 filenames, but the transport must still
        // preserve Git's raw bytes for filesystems that support them.
        let mut input = super::pathspec_input(&[
            RepoPath(b"icon-\xff.txt".to_vec()),
            RepoPath(b"next.txt".to_vec()),
        ])?;
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"icon-\xff.txt\0next.txt\0");
        Ok(())
    }

    #[test]
    fn malformed_paths_fail_before_editing_the_index() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        std::fs::write(repository.path().join("wanted.txt"), "content\n")?;
        let location = discover(repository.path())?;
        for malformed in [b"".as_slice(), b"wanted.txt\0bystander.txt"] {
            assert!(
                stage_paths(
                    &location,
                    &[
                        RepoPath(b"wanted.txt".to_vec()),
                        RepoPath(malformed.to_vec())
                    ]
                )
                .is_err()
            );
            assert!(crate::working_tree_status(&location)?.staged.is_empty());
        }
        Ok(())
    }
}
