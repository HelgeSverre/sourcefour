//! Branch creation, optionally checking out, through the user's Git (§6.13).

use std::process::Command;

use sourcefour_model::{
    CreateBranchRequest, OperationKind, OperationOutcome, RepoFailure, RepoFailureKind,
    RepoLocation,
};

/// The exact §6.13 argv; arguments are passed discretely, never joined into
/// a command string.
fn branch_argv(request: &CreateBranchRequest) -> Vec<String> {
    let start = request.start.to_hex();
    if request.checkout {
        vec![
            String::from("switch"),
            String::from("-c"),
            request.name.clone(),
            start,
        ]
    } else {
        vec![String::from("branch"), request.name.clone(), start]
    }
}

/// A Git-compatible branch-name check (§6.13), mirroring
/// `git check-ref-format --branch` for one-level names under `refs/heads/`.
#[must_use]
pub fn is_valid_branch_name(name: &str) -> bool {
    if name.is_empty() || name == "@" || name.starts_with('-') {
        return false;
    }
    if name.starts_with('/') || name.ends_with('/') || name.ends_with('.') {
        return false;
    }
    if name.contains("..") || name.contains("//") || name.contains("@{") {
        return false;
    }
    if name
        .chars()
        .any(|c| c.is_ascii_control() || " ~^:?*[\\".contains(c))
    {
        return false;
    }
    name.split('/').all(valid_segment)
}

/// One `/`-separated component of a ref name.
///
/// Git's `.lock` rule is case-sensitive by design, unlike file extensions.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "ref names are not file paths; Git compares .lock exactly"
)]
fn valid_segment(segment: &str) -> bool {
    !segment.is_empty() && !segment.starts_with('.') && !segment.ends_with(".lock")
}

/// Creates a branch at the requested start, optionally switching to it.
///
/// # Errors
///
/// Returns a typed failure only when Git cannot be started; a run that failed
/// is an [`OperationOutcome::Failed`] carrying Git's exact words — which is
/// how a branch checked out in another worktree explains itself (§6.13).
pub fn create_branch(
    location: &RepoLocation,
    request: &CreateBranchRequest,
) -> Result<OperationOutcome, RepoFailure> {
    if !is_valid_branch_name(&request.name) {
        return Ok(OperationOutcome::Failed {
            kind: OperationKind::CreateBranch,
            error: RepoFailure::new(
                RepoFailureKind::Internal,
                "Invalid branch name",
                format!("'{}' is not a valid branch name.", request.name),
            ),
        });
    }
    let kind = if request.checkout {
        OperationKind::CreateAndCheckoutBranch
    } else {
        OperationKind::CreateBranch
    };
    let workdir = location
        .active_worktree_path
        .as_deref()
        .unwrap_or(&location.common_dir);
    let output = Command::new("git")
        .args(branch_argv(request))
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
            kind,
            summary: if request.checkout {
                format!("Created and switched to {}", request.name)
            } else {
                format!("Created {}", request.name)
            },
        })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Ok(OperationOutcome::Failed {
            kind,
            error: RepoFailure::new(
                RepoFailureKind::OperationConflict,
                "Branch could not be created",
                stderr.trim().to_owned(),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{CreateBranchRequest, Oid, OperationOutcome, WorktreeId};
    use sourcefour_test_support::TempRepo;

    use super::{branch_argv, create_branch, is_valid_branch_name};
    use crate::discover;

    fn request(name: &str, start: Oid, checkout: bool) -> CreateBranchRequest {
        CreateBranchRequest {
            worktree: WorktreeId(String::from("test")),
            name: name.to_owned(),
            start,
            checkout,
        }
    }

    fn head(repository: &TempRepo) -> Result<Oid, Box<dyn std::error::Error>> {
        Ok(Oid::from_hex(&repository.git(&["rev-parse", "HEAD"]))?)
    }

    #[test]
    fn the_exact_argv_is_stable() {
        let start = Oid::sha1([0xAB; 20]);
        let hex = start.to_hex();

        assert_eq!(
            branch_argv(&request("feature/x", start, false)),
            ["branch", "feature/x", hex.as_str()]
        );
        assert_eq!(
            branch_argv(&request("feature/x", start, true)),
            ["switch", "-c", "feature/x", hex.as_str()]
        );
    }

    #[test]
    fn branch_names_validate_like_git() {
        for valid in ["main", "feature/deep/name", "v1.2.3", "fix-things", "UPPER"] {
            assert!(is_valid_branch_name(valid), "{valid} is a fine name");
        }
        for invalid in [
            "",
            "-dash-start",
            "has space",
            "double..dot",
            "trailing/",
            "/leading",
            "a//b",
            "ends.lock",
            "dot.",
            ".hidden",
            "seg/.hidden",
            "tilde~1",
            "caret^",
            "colon:name",
            "quest?",
            "star*",
            "bracket[",
            "back\\slash",
            "at@{brace",
            "@",
            "control\u{7}char",
        ] {
            assert!(
                !is_valid_branch_name(invalid),
                "{invalid:?} must be rejected"
            );
        }
    }

    #[test]
    fn a_branch_is_created_at_the_start_point_without_moving_head()
    -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let older = head(&repository)?;
        repository.commit("newer work");
        let newer = repository.git(&["rev-parse", "HEAD"]);

        let outcome = create_branch(
            &discover(repository.path())?,
            &request("fixture/at-older", older, false),
        )?;

        assert!(matches!(outcome, OperationOutcome::Succeeded { .. }));
        assert_eq!(
            repository.git(&["rev-parse", "fixture/at-older"]),
            older.to_hex()
        );
        assert_eq!(
            repository.git(&["rev-parse", "HEAD"]),
            newer,
            "plain creation must not move HEAD"
        );
        Ok(())
    }

    #[test]
    fn checkout_creates_and_switches() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let start = head(&repository)?;

        let outcome = create_branch(
            &discover(repository.path())?,
            &request("fixture/switched", start, true),
        )?;

        assert!(matches!(outcome, OperationOutcome::Succeeded { .. }));
        assert_eq!(
            repository.git(&["branch", "--show-current"]),
            "fixture/switched"
        );
        Ok(())
    }

    #[test]
    fn a_duplicate_name_surfaces_gits_exact_error() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let start = head(&repository)?;

        let outcome = create_branch(
            &discover(repository.path())?,
            &request("main", start, false),
        )?;

        let OperationOutcome::Failed { error, .. } = outcome else {
            panic!("creating an existing branch must fail");
        };
        assert!(
            error.user.message.contains("main"),
            "Git's own words name the branch: {}",
            error.user.message
        );
        Ok(())
    }
}
