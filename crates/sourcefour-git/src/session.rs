//! Assembly of the one metadata snapshot the interface opens with.

use std::time::Instant;

use sourcefour_model::{HeadSnapshot, RepoFailure, RepoLocation, RepoSnapshot};

use crate::{display_name, refs::references, worktrees::worktrees};

/// Reads worktrees and references into the snapshot the sidebar renders.
///
/// This is the whole of the first metadata pass. It deliberately excludes
/// ahead/behind counts (§6.7) and history (§6.9), both of which arrive later so
/// the first window is never waiting on them.
///
/// # Errors
///
/// Returns a typed failure when the repository or its references cannot be read.
pub fn snapshot(location: &RepoLocation) -> Result<RepoSnapshot, RepoFailure> {
    let worktrees = worktrees(location)?;
    let references = references(location, &worktrees)?;
    let active = worktrees.iter().find(|tree| tree.is_current);
    Ok(RepoSnapshot {
        display_name: display_name(location),
        active_worktree: active.map(|tree| tree.id.clone()),
        head: active.map_or(HeadSnapshot::Missing, |tree| tree.head.clone()),
        location: location.clone(),
        worktrees,
        local_branches: references.local_branches,
        remotes: references.remotes,
        tags_count: references.tags_count,
        labels_by_object: references.labels_by_object,
        refreshed_at: Instant::now(),
    })
}

#[cfg(test)]
mod tests {
    use sourcefour_model::HeadSnapshot;
    use sourcefour_test_support::TempRepo;

    use super::snapshot;
    use crate::discover;

    #[test]
    fn a_snapshot_carries_everything_the_sidebar_renders() -> Result<(), Box<dyn std::error::Error>>
    {
        let origin = TempRepo::init();
        let repository = TempRepo::clone_of(&origin);
        let linked = repository.add_worktree("side");

        let found = snapshot(&discover(repository.path())?)?;

        assert_eq!(found.display_name, "clone");
        assert_eq!(found.worktrees.len(), 2);
        assert_eq!(found.remotes.len(), 1);
        assert!(!found.local_branches.is_empty());
        assert!(
            found
                .worktrees
                .iter()
                .any(|tree| tree.path == linked && !tree.is_current)
        );
        assert!(matches!(
            &found.head,
            HeadSnapshot::Branch { short_name, .. } if short_name == "main"
        ));
        Ok(())
    }

    #[test]
    fn the_active_worktree_follows_the_invocation_path() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let linked = repository.add_worktree("side");

        let found = snapshot(&discover(&linked)?)?;

        let active = found
            .active_worktree
            .as_ref()
            .expect("a worktree invocation has an active worktree");
        let tree = found
            .worktrees
            .iter()
            .find(|tree| &tree.id == active)
            .expect("the active worktree is listed");
        assert_eq!(tree.path, linked);
        assert!(matches!(
            &found.head,
            HeadSnapshot::Branch { short_name, .. } if short_name == "side"
        ));
        Ok(())
    }

    #[test]
    fn a_bare_repository_has_no_active_worktree() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init_bare();

        let found = snapshot(&discover(repository.path())?)?;

        assert_eq!(found.active_worktree, None);
        assert!(found.worktrees.is_empty());
        assert!(
            !found.local_branches.is_empty(),
            "a bare repository still has refs"
        );
        Ok(())
    }
}
