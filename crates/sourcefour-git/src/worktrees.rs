//! Worktree enumeration: the main worktree, then every linked worktree.
//!
//! A linked worktree whose checkout has disappeared must still be listed with
//! its HEAD readable, so enumeration opens proxies with possibly-inaccessible
//! worktree semantics rather than assuming the checkout is present.

use std::path::Path;

use sourcefour_model::{
    HeadSnapshot, Oid, RepoFailure, RepoFailureKind, RepoLocation, WorktreeAccessibility,
    WorktreeId, WorktreeSnapshot,
};

use crate::path::canonical;

/// Enumerates the worktrees sharing `location`'s common directory.
///
/// The main worktree comes first when one exists; a bare repository has none.
/// Exactly one snapshot is marked current: the one whose private Git directory
/// matches the invocation's.
///
/// # Errors
///
/// Returns a typed failure when the repository itself cannot be opened. One
/// unreadable linked worktree is reported in its own snapshot rather than
/// failing the enumeration.
pub fn worktrees(location: &RepoLocation) -> Result<Vec<WorktreeSnapshot>, RepoFailure> {
    let common = gix::open(&location.common_dir).map_err(|error| {
        RepoFailure::new(
            RepoFailureKind::CorruptRepository,
            "Repository could not be opened",
            format!(
                "The repository at {} could not be read.",
                location.common_dir.display()
            ),
        )
        .with_details(error.to_string())
    })?;

    let mut snapshots = Vec::new();
    if let Some(workdir) = common.workdir() {
        let git_dir = canonical(common.git_dir());
        snapshots.push(WorktreeSnapshot {
            id: identity(&git_dir),
            display_name: display_name(workdir),
            is_current: git_dir == location.git_dir,
            is_main: true,
            head: head_snapshot(&common),
            accessibility: accessibility(workdir),
            path: canonical(workdir),
            git_dir,
            is_locked: false,
            lock_reason: None,
        });
    }

    // gix sorts proxies by private Git directory, which keeps enumeration order
    // stable across invocations regardless of checkout paths.
    let proxies = common.worktrees().map_err(|error| {
        RepoFailure::new(
            RepoFailureKind::CorruptRepository,
            "Worktrees could not be listed",
            format!(
                "The worktree registrations in {} could not be read.",
                location.common_dir.display()
            ),
        )
        .with_details(error.to_string())
    })?;

    for proxy in proxies {
        let git_dir = canonical(proxy.git_dir());
        let is_locked = proxy.is_locked();
        let lock_reason = proxy
            .lock_reason()
            .map(|reason| reason.to_string())
            .filter(|reason| !reason.is_empty());
        // The checkout may be gone; the private Git directory holding HEAD is not.
        let base = proxy.base().ok();
        let linked = proxy
            .into_repo_with_possibly_inaccessible_worktree()
            .ok()
            .filter(|repository| repository.workdir().is_some() || base.is_some());
        let path = base.unwrap_or_else(|| git_dir.clone());
        snapshots.push(WorktreeSnapshot {
            id: identity(&git_dir),
            display_name: display_name(&path),
            is_current: git_dir == location.git_dir,
            is_main: false,
            head: linked.as_ref().map_or(HeadSnapshot::Missing, head_snapshot),
            accessibility: accessibility(&path),
            path: canonical(&path),
            git_dir,
            is_locked,
            lock_reason,
        });
    }

    Ok(snapshots)
}

/// Identity is the private Git directory, which is what Git itself keys on and
/// what stays constant when a checkout is moved or disappears.
fn identity(git_dir: &Path) -> WorktreeId {
    WorktreeId(git_dir.to_string_lossy().into_owned())
}

/// The checkout directory's own name; never the branch it happens to hold.
fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn accessibility(path: &Path) -> WorktreeAccessibility {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => WorktreeAccessibility::Accessible,
        Ok(_) => WorktreeAccessibility::Inaccessible {
            reason: format!("{} is not a directory", path.display()),
        },
        Err(error) => WorktreeAccessibility::Inaccessible {
            reason: error.to_string(),
        },
    }
}

fn head_snapshot(repository: &gix::Repository) -> HeadSnapshot {
    let Ok(head) = repository.head() else {
        return HeadSnapshot::Missing;
    };
    match head.kind {
        gix::head::Kind::Symbolic(reference) => {
            let full_name = reference.name.as_bstr().to_string();
            let Some(oid) = reference.target.try_id().and_then(convert_oid) else {
                return HeadSnapshot::Missing;
            };
            HeadSnapshot::Branch {
                short_name: reference.name.shorten().to_string(),
                full_name,
                oid,
            }
        }
        gix::head::Kind::Unborn(name) => HeadSnapshot::Unborn {
            intended_branch: Some(name.shorten().to_string()),
        },
        gix::head::Kind::Detached { target, peeled } => {
            let id = peeled.unwrap_or(target);
            convert_oid(id.as_ref())
                .map_or(HeadSnapshot::Missing, |oid| HeadSnapshot::Detached { oid })
        }
    }
}

/// Converts a gix object ID into the transport-safe model ID.
///
/// Keeping this local is what stops gix hash types from reaching the UI.
fn convert_oid(id: &gix::hash::oid) -> Option<Oid> {
    match id.as_bytes().len() {
        20 => Some(Oid::sha1(id.as_bytes().try_into().ok()?)),
        32 => Some(Oid::sha256(id.as_bytes().try_into().ok()?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{HeadSnapshot, WorktreeAccessibility};
    use sourcefour_test_support::TempRepo;

    use super::worktrees;
    use crate::discover;

    #[test]
    fn a_plain_repository_has_one_current_main_worktree() -> Result<(), Box<dyn std::error::Error>>
    {
        let repository = TempRepo::init();

        let found = worktrees(&discover(repository.path())?)?;

        assert_eq!(found.len(), 1);
        let main = &found[0];
        assert!(main.is_main);
        assert!(main.is_current);
        assert_eq!(main.path, repository.path());
        assert_eq!(main.display_name, "repository");
        assert!(matches!(
            &main.head,
            HeadSnapshot::Branch { short_name, .. } if short_name == "main"
        ));
        Ok(())
    }

    #[test]
    fn the_invocation_worktree_is_the_current_one() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let linked = repository.add_worktree("side");

        let from_linked = worktrees(&discover(&linked)?)?;

        let current: Vec<_> = from_linked.iter().filter(|tree| tree.is_current).collect();
        assert_eq!(current.len(), 1, "exactly one worktree is current");
        assert_eq!(current[0].path, linked);
        assert!(!current[0].is_main);
        Ok(())
    }

    #[test]
    fn the_main_worktree_is_listed_first() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let _linked = repository.add_worktree("side");

        let found = worktrees(&discover(repository.path())?)?;

        assert_eq!(found.len(), 2);
        assert!(found[0].is_main);
        assert!(!found[1].is_main);
        Ok(())
    }

    #[test]
    fn a_worktree_directory_name_does_not_decide_its_branch()
    -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let linked = repository.add_worktree("side");
        // Rename the branch so the directory and the branch disagree.
        repository.git(&["branch", "-m", "side", "feature/renamed"]);

        let found = worktrees(&discover(&linked)?)?;
        let linked_tree = found
            .iter()
            .find(|tree| tree.path == linked)
            .expect("the linked worktree is listed");

        assert_eq!(linked_tree.display_name, "side");
        assert!(matches!(
            &linked_tree.head,
            HeadSnapshot::Branch { short_name, .. } if short_name == "feature/renamed"
        ));
        Ok(())
    }

    #[test]
    fn a_detached_worktree_reports_its_commit() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let detached = repository.add_detached_worktree("detached");

        let found = worktrees(&discover(repository.path())?)?;
        let detached_tree = found
            .iter()
            .find(|tree| tree.path == detached)
            .expect("the detached worktree is listed");

        assert!(matches!(detached_tree.head, HeadSnapshot::Detached { .. }));
        Ok(())
    }

    #[test]
    fn a_locked_worktree_reports_its_reason() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let linked = repository.add_worktree("side");
        repository.lock_worktree(&linked, "on an external disk");

        let found = worktrees(&discover(repository.path())?)?;
        let locked = found
            .iter()
            .find(|tree| tree.path == linked)
            .expect("the locked worktree is listed");

        assert!(locked.is_locked);
        assert_eq!(locked.lock_reason.as_deref(), Some("on an external disk"));
        Ok(())
    }

    #[test]
    fn one_inaccessible_worktree_does_not_hide_the_others() -> Result<(), Box<dyn std::error::Error>>
    {
        let repository = TempRepo::init();
        let present = repository.add_worktree("present");
        let removed = repository.add_worktree("removed");
        TempRepo::remove_worktree_checkout(&removed);

        let found = worktrees(&discover(repository.path())?)?;

        assert_eq!(found.len(), 3, "the removed checkout is still listed");
        let gone = found
            .iter()
            .find(|tree| tree.path == removed)
            .expect("the removed worktree is listed");
        assert!(matches!(
            gone.accessibility,
            WorktreeAccessibility::Inaccessible { .. }
        ));
        // Its HEAD is still readable because the private Git directory remains.
        assert!(matches!(gone.head, HeadSnapshot::Branch { .. }));
        let intact = found
            .iter()
            .find(|tree| tree.path == present)
            .expect("the intact worktree is listed");
        assert_eq!(intact.accessibility, WorktreeAccessibility::Accessible);
        Ok(())
    }

    #[test]
    fn an_unborn_repository_reports_its_intended_branch() -> Result<(), Box<dyn std::error::Error>>
    {
        let repository = TempRepo::init_unborn();

        let found = worktrees(&discover(repository.path())?)?;

        assert_eq!(found.len(), 1);
        assert!(matches!(
            &found[0].head,
            HeadSnapshot::Unborn { intended_branch } if intended_branch.as_deref() == Some("main")
        ));
        Ok(())
    }

    #[test]
    fn a_bare_repository_has_no_main_worktree() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init_bare();

        let found = worktrees(&discover(repository.path())?)?;

        assert!(found.is_empty());
        Ok(())
    }

    #[test]
    fn worktree_identity_is_stable_across_invocation_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let linked = repository.add_worktree("side");

        let from_main = worktrees(&discover(repository.path())?)?;
        let from_linked = worktrees(&discover(&linked)?)?;

        let main_ids: Vec<_> = from_main.iter().map(|tree| tree.id.clone()).collect();
        let linked_ids: Vec<_> = from_linked.iter().map(|tree| tree.id.clone()).collect();
        assert_eq!(main_ids, linked_ids);
        Ok(())
    }
}
