//! Repository discovery: turn an invocation path into a repository identity.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use gix::discover::upwards::Error as UpwardsError;
use sourcefour_model::{RepoFailure, RepoFailureKind, RepoKind, RepoLocation};

/// Discovers the repository containing `invocation_path`.
///
/// A file path is discovered from its parent directory. The returned
/// `invocation_path` is the caller's own path, unmodified, so the interface can
/// show what the user typed; every Git directory is canonicalized so that
/// repository identity survives symlinked temporary and home directories.
///
/// # Errors
///
/// Returns a typed failure when no repository contains the path, when the path
/// cannot be read, or when Git's trust policy rejects the repository.
pub fn discover(invocation_path: &Path) -> Result<RepoLocation, RepoFailure> {
    let repository = gix::ThreadSafeRepository::discover(discovery_start(invocation_path))
        .map_err(|error| classify(invocation_path, &error))?
        .to_thread_local();

    let git_dir = canonical(repository.git_dir());
    let common_dir = canonical(repository.common_dir());
    let is_bare = repository.is_bare();
    let kind = if is_bare {
        RepoKind::Bare
    } else if git_dir == common_dir {
        RepoKind::Worktree
    } else {
        RepoKind::LinkedWorktree
    };

    Ok(RepoLocation {
        invocation_path: invocation_path.to_path_buf(),
        active_worktree_path: repository.workdir().map(canonical),
        git_dir,
        common_dir,
        is_bare,
        kind,
    })
}

/// Repository name suitable for a title bar, stable across linked worktrees.
///
/// Derived from the common directory rather than the active worktree, so every
/// linked worktree of one repository reports the same name.
#[must_use]
pub fn display_name(location: &RepoLocation) -> String {
    let common = location.common_dir.as_path();
    let name = if common.file_name() == Some(OsStr::new(".git")) {
        common
            .parent()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
    } else {
        // A bare repository is conventionally named `<name>.git`.
        common.file_name().map(|name| {
            let name = name.to_string_lossy();
            name.strip_suffix(".git").unwrap_or(&name).to_owned()
        })
    };
    name.filter(|name| !name.is_empty())
        .unwrap_or_else(|| String::from("repository"))
}

/// Discovery starts at a directory, so a file is discovered from its parent.
fn discovery_start(invocation_path: &Path) -> &Path {
    if invocation_path.is_file() {
        invocation_path.parent().unwrap_or(invocation_path)
    } else {
        invocation_path
    }
}

/// Resolves symlinks so repository identity compares reliably, and keeps the
/// original path when resolution is not permitted rather than failing the open.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn classify(invocation_path: &Path, error: &gix::discover::Error) -> RepoFailure {
    let path = invocation_path.display();
    let (kind, title, message) = match error {
        gix::discover::Error::Discover(
            UpwardsError::NoGitRepository { .. }
            | UpwardsError::NoGitRepositoryWithinCeiling { .. }
            | UpwardsError::NoGitRepositoryWithinFs { .. }
            | UpwardsError::NoMatchingCeilingDir
            | UpwardsError::InvalidInput { .. },
        ) => (
            RepoFailureKind::NotARepository,
            "Not a Git repository",
            format!("No Git repository contains {path}."),
        ),
        gix::discover::Error::Discover(UpwardsError::NoTrustedGitRepository { .. }) => (
            RepoFailureKind::UntrustedRepository,
            "Repository not trusted",
            format!(
                "Git refused the repository containing {path} because it is owned by another user. Add it to `safe.directory` to open it."
            ),
        ),
        gix::discover::Error::Discover(
            UpwardsError::InaccessibleDirectory { .. }
            | UpwardsError::CurrentDir(_)
            | UpwardsError::CheckTrust { .. },
        ) => (
            RepoFailureKind::PermissionDenied,
            "Path could not be read",
            format!("Sourcefour is not allowed to read {path}."),
        ),
        gix::discover::Error::Open(_) => (
            RepoFailureKind::CorruptRepository,
            "Repository could not be opened",
            format!("The repository containing {path} could not be read."),
        ),
    };
    RepoFailure::new(kind, title, message).with_details(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{discover, display_name};
    use sourcefour_model::{RepoFailureKind, RepoKind};
    use sourcefour_test_support::TempRepo;

    #[test]
    fn discovers_a_working_tree_repository_from_its_root() -> Result<(), Box<dyn std::error::Error>>
    {
        let repository = TempRepo::init();

        let location = discover(repository.path())?;

        assert_eq!(location.kind, RepoKind::Worktree);
        assert_eq!(
            location.active_worktree_path.as_deref(),
            Some(repository.path())
        );
        assert_eq!(location.git_dir, repository.path().join(".git"));
        assert_eq!(location.common_dir, location.git_dir);
        assert!(!location.is_bare);
        Ok(())
    }

    #[test]
    fn discovers_upward_from_a_nested_directory() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let nested = repository.path().join("crates/deep");
        std::fs::create_dir_all(&nested)?;

        let location = discover(&nested)?;

        assert_eq!(location.invocation_path, nested);
        assert_eq!(
            location.active_worktree_path.as_deref(),
            Some(repository.path())
        );
        Ok(())
    }

    #[test]
    fn discovers_from_a_file_path_using_its_parent() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let file = repository.path().join("README.md");
        std::fs::write(&file, "fixture")?;

        let location = discover(&file)?;

        assert_eq!(location.invocation_path, file);
        assert_eq!(
            location.active_worktree_path.as_deref(),
            Some(repository.path())
        );
        Ok(())
    }

    #[test]
    fn discovers_from_the_git_directory_itself() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();

        let location = discover(&repository.path().join(".git"))?;

        assert_eq!(location.common_dir, repository.path().join(".git"));
        Ok(())
    }

    #[test]
    fn marks_a_linked_worktree_as_the_active_context() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let linked = repository.add_worktree("side");

        let location = discover(&linked)?;

        assert_eq!(location.kind, RepoKind::LinkedWorktree);
        assert_eq!(
            location.active_worktree_path.as_deref(),
            Some(linked.as_path())
        );
        assert_eq!(location.common_dir, repository.path().join(".git"));
        assert_ne!(location.git_dir, location.common_dir);
        Ok(())
    }

    #[test]
    fn discovers_a_bare_repository_without_a_working_tree() -> Result<(), Box<dyn std::error::Error>>
    {
        let repository = TempRepo::init_bare();

        let location = discover(repository.path())?;

        assert_eq!(location.kind, RepoKind::Bare);
        assert_eq!(location.active_worktree_path, None);
        assert!(location.is_bare);
        assert_eq!(location.common_dir, repository.path());
        Ok(())
    }

    #[test]
    fn reports_a_clear_failure_outside_any_repository() {
        let directory = std::env::temp_dir();

        let failure =
            discover(&directory).expect_err("the temporary directory is not a repository");

        assert_eq!(failure.kind, RepoFailureKind::NotARepository);
        assert!(!failure.user.title.is_empty());
        assert!(
            failure
                .user
                .message
                .contains(&directory.display().to_string()),
            "the message should name the rejected path: {}",
            failure.user.message
        );
    }

    #[test]
    fn display_name_is_stable_across_worktrees_and_bare_clones()
    -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let linked = repository.add_worktree("side");

        assert_eq!(display_name(&discover(repository.path())?), "repository");
        assert_eq!(display_name(&discover(&linked)?), "repository");
        assert_eq!(
            display_name(&discover(TempRepo::init_bare().path())?),
            "repository"
        );
        Ok(())
    }
}
