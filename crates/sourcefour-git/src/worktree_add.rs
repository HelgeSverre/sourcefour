//! Linked-worktree creation through the user's installed Git.

use std::{fs::OpenOptions, io::Write, path::Path, process::Command};

use sourcefour_model::{
    AddWorktreeRequest, OperationKind, OperationOutcome, RepoFailure, RepoFailureKind,
    RepoLocation, WorktreeSource,
};

use crate::is_valid_branch_name;

/// Adds the application-managed worktree directory to the repository-local
/// exclude file without touching the tracked `.gitignore`.
///
/// # Errors
///
/// Returns an I/O failure when the common repository metadata is not writable.
pub fn ensure_internal_worktrees_excluded(location: &RepoLocation) -> std::io::Result<()> {
    let info = location.common_dir.join("info");
    std::fs::create_dir_all(&info)?;
    let path = info.join("exclude");
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    if current.lines().any(|line| line.trim() == ".worktrees/") {
        return Ok(());
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    if !current.is_empty() && !current.ends_with('\n') {
        writeln!(file)?;
    }
    writeln!(file, ".worktrees/")
}

fn argv(request: &AddWorktreeRequest) -> Vec<String> {
    let path = request.path.to_string_lossy().into_owned();
    match &request.source {
        WorktreeSource::LocalBranch { full_name } => vec![
            String::from("worktree"),
            String::from("add"),
            path,
            full_name.clone(),
        ],
        WorktreeSource::RemoteBranch {
            full_name,
            local_name,
        } => vec![
            String::from("worktree"),
            String::from("add"),
            String::from("--track"),
            String::from("-b"),
            local_name.clone(),
            path,
            full_name.clone(),
        ],
    }
}

fn target_is_available(path: &Path) -> bool {
    !path.exists() || std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

/// Creates a linked worktree and returns Git's failure without hiding the
/// currently loaded repository.
///
/// # Errors
///
/// Returns a typed failure when Git cannot be started.
pub fn add_worktree(
    location: &RepoLocation,
    request: &AddWorktreeRequest,
) -> Result<OperationOutcome, RepoFailure> {
    let invalid_source = match &request.source {
        WorktreeSource::LocalBranch { full_name } => !full_name.starts_with("refs/heads/"),
        WorktreeSource::RemoteBranch {
            full_name,
            local_name,
        } => !full_name.starts_with("refs/remotes/") || !is_valid_branch_name(local_name),
    };
    if invalid_source || !target_is_available(&request.path) {
        return Ok(OperationOutcome::Failed {
            kind: OperationKind::AddWorktree,
            error: RepoFailure::new(
                RepoFailureKind::OperationConflict,
                "Worktree could not be created",
                "Choose an empty or nonexistent path and a valid branch.",
            ),
        });
    }
    let workdir = location
        .active_worktree_path
        .as_deref()
        .unwrap_or(&location.common_dir);
    let output = Command::new("git")
        .args(argv(request))
        .current_dir(workdir)
        .output()
        .map_err(|error| {
            RepoFailure::new(
                RepoFailureKind::Internal,
                "Git could not be started",
                "The installed git executable could not be run.",
            )
            .with_details(error.to_string())
        })?;
    if output.status.success() {
        Ok(OperationOutcome::Succeeded {
            kind: OperationKind::AddWorktree,
            summary: format!("Created worktree at {}", request.path.display()),
        })
    } else {
        Ok(OperationOutcome::Failed {
            kind: OperationKind::AddWorktree,
            error: RepoFailure::new(
                RepoFailureKind::OperationConflict,
                "Worktree could not be created",
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{AddWorktreeRequest, OperationOutcome, WorktreeId, WorktreeSource};
    use sourcefour_test_support::TempRepo;

    use super::{add_worktree, argv, ensure_internal_worktrees_excluded};
    use crate::{discover, references, worktrees};

    #[test]
    fn local_branch_argv_is_discrete() {
        let request = AddWorktreeRequest {
            worktree: WorktreeId(String::from("test")),
            source: WorktreeSource::LocalBranch {
                full_name: String::from("refs/heads/feature/a"),
            },
            path: "/tmp/a b".into(),
        };
        assert_eq!(
            argv(&request),
            ["worktree", "add", "/tmp/a b", "refs/heads/feature/a"]
        );
    }

    #[test]
    fn internal_directory_is_excluded_once() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let location = discover(repository.path())?;
        ensure_internal_worktrees_excluded(&location)?;
        ensure_internal_worktrees_excluded(&location)?;
        let exclude = std::fs::read_to_string(location.common_dir.join("info/exclude"))?;
        assert_eq!(
            exclude
                .lines()
                .filter(|line| line.trim() == ".worktrees/")
                .count(),
            1
        );
        Ok(())
    }

    #[test]
    fn creates_a_worktree_for_a_local_branch() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.git(&["branch", "feature/worktree"]);
        let path = repository.path().parent().unwrap().join("created-worktree");
        let location = discover(repository.path())?;
        let outcome = add_worktree(
            &location,
            &AddWorktreeRequest {
                worktree: WorktreeId(String::from("test")),
                source: WorktreeSource::LocalBranch {
                    full_name: String::from("refs/heads/feature/worktree"),
                },
                path: path.clone(),
            },
        )?;
        assert!(matches!(outcome, OperationOutcome::Succeeded { .. }));
        assert!(worktrees(&location)?.iter().any(|tree| tree.path == path));
        Ok(())
    }

    #[test]
    fn creates_a_tracking_branch_for_a_remote_source() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        origin.git(&["branch", "feature/remote"]);
        let repository = TempRepo::clone_of(&origin);
        let path = repository.path().parent().unwrap().join("remote-worktree");
        let location = discover(repository.path())?;
        let outcome = add_worktree(
            &location,
            &AddWorktreeRequest {
                worktree: WorktreeId(String::from("test")),
                source: WorktreeSource::RemoteBranch {
                    full_name: String::from("refs/remotes/origin/feature/remote"),
                    local_name: String::from("feature/remote"),
                },
                path,
            },
        )?;
        assert!(matches!(outcome, OperationOutcome::Succeeded { .. }));
        let branch = references(&location, &worktrees(&location)?)?
            .local_branches
            .into_iter()
            .find(|branch| branch.short_name == "feature/remote")
            .expect("tracking branch exists");
        assert_eq!(branch.upstream.unwrap().short_name, "origin/feature/remote");
        Ok(())
    }
}
