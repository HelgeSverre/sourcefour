//! Lazy ahead/behind counts, computed after the sidebar is already visible.
//!
//! §6.7 forbids blocking the first render on these, so they are a separate pass
//! keyed by `(local_tip, upstream_tip)` and invalidated only when a tip moves.

use std::collections::HashMap;

use sourcefour_model::{AheadBehindState, AheadBehindUpdate, BranchSnapshot, Oid, RepoLocation};

/// Cache of divergence results keyed by the tips they were computed from.
///
/// A branch whose tips have not moved is never recomputed, which is what keeps
/// a metadata refresh from re-walking history it already knows about.
#[derive(Clone, Debug, Default)]
pub struct AheadBehindCache {
    entries: HashMap<(Oid, Oid), AheadBehindState>,
}

impl AheadBehindCache {
    /// Computes divergence for every branch that has an upstream.
    ///
    /// Branches without an upstream are skipped entirely: there is nothing to
    /// compare them against, and §6.7 requires no indicator at all.
    pub fn update(
        &mut self,
        location: &RepoLocation,
        branches: &[BranchSnapshot],
    ) -> Vec<AheadBehindUpdate> {
        let pending: Vec<(&BranchSnapshot, Oid)> = branches
            .iter()
            .filter_map(|branch| Some((branch, branch.upstream.as_ref()?.tip)))
            .collect();
        if pending.is_empty() {
            return Vec::new();
        }
        let Ok(repository) = gix::open(&location.git_dir) else {
            return pending
                .into_iter()
                .map(|(branch, _)| AheadBehindUpdate {
                    full_name: branch.full_name.clone(),
                    state: AheadBehindState::Failed,
                })
                .collect();
        };

        pending
            .into_iter()
            .map(|(branch, upstream_tip)| AheadBehindUpdate {
                full_name: branch.full_name.clone(),
                state: *self
                    .entries
                    .entry((branch.tip, upstream_tip))
                    .or_insert_with(|| divergence(&repository, branch.tip, upstream_tip)),
            })
            .collect()
    }

    /// Number of cached results, used to prove reuse rather than recomputation.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been computed yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Applies computed updates onto branch snapshots by full ref name.
pub fn apply(branches: &mut [BranchSnapshot], updates: &[AheadBehindUpdate]) {
    let by_name: HashMap<&str, AheadBehindState> = updates
        .iter()
        .map(|update| (update.full_name.as_str(), update.state))
        .collect();
    for branch in branches {
        if let Some(state) = by_name.get(branch.full_name.as_str()) {
            branch.ahead_behind = *state;
        }
    }
}

fn divergence(repository: &gix::Repository, local: Oid, upstream: Oid) -> AheadBehindState {
    let (Some(local_id), Some(upstream_id)) = (object_id(local), object_id(upstream)) else {
        return AheadBehindState::Failed;
    };
    if local_id == upstream_id {
        return AheadBehindState::Known {
            ahead: 0,
            behind: 0,
        };
    }
    // No merge base means the histories never met; a numeric count would be
    // technically true but misleading, so §6.7 asks for a marker instead.
    if repository.merge_base(local_id, upstream_id).is_err() {
        return AheadBehindState::Unrelated;
    }
    match (
        count_excluding(repository, local_id, upstream_id),
        count_excluding(repository, upstream_id, local_id),
    ) {
        (Some(ahead), Some(behind)) => AheadBehindState::Known { ahead, behind },
        _ => AheadBehindState::Failed,
    }
}

/// Commits reachable from `tip` but not from `hidden`, as `rev-list --count`.
fn count_excluding(
    repository: &gix::Repository,
    tip: gix::ObjectId,
    hidden: gix::ObjectId,
) -> Option<u32> {
    let walk = repository
        .rev_walk(Some(tip))
        .with_hidden(Some(hidden))
        .all()
        .ok()?;
    u32::try_from(walk.take_while(Result::is_ok).count()).ok()
}

fn object_id(oid: Oid) -> Option<gix::ObjectId> {
    gix::ObjectId::from_bytes_or_panic(oid.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use sourcefour_model::AheadBehindState;
    use sourcefour_test_support::TempRepo;

    use super::{AheadBehindCache, apply};
    use crate::{discover, references, worktrees};

    fn branches(
        repository: &TempRepo,
    ) -> Result<
        (
            sourcefour_model::RepoLocation,
            Vec<sourcefour_model::BranchSnapshot>,
        ),
        Box<dyn std::error::Error>,
    > {
        let location = discover(repository.path())?;
        let trees = worktrees(&location)?;
        let found = references(&location, &trees)?;
        Ok((location, found.local_branches))
    }

    fn state_of(
        updates: &[sourcefour_model::AheadBehindUpdate],
        short_name: &str,
    ) -> AheadBehindState {
        updates
            .iter()
            .find(|update| update.full_name.ends_with(short_name))
            .map(|update| update.state)
            .expect("the branch has an update")
    }

    #[test]
    fn a_branch_level_with_its_upstream_is_zero_zero() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let repository = TempRepo::clone_of(&origin);
        let (location, branches) = branches(&repository)?;

        let updates = AheadBehindCache::default().update(&location, &branches);

        assert_eq!(
            state_of(&updates, "main"),
            AheadBehindState::Known {
                ahead: 0,
                behind: 0
            }
        );
        Ok(())
    }

    #[test]
    fn local_commits_count_as_ahead() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let repository = TempRepo::clone_of(&origin);
        repository.git(&["commit", "--allow-empty", "-m", "local one"]);
        repository.git(&["commit", "--allow-empty", "-m", "local two"]);
        let (location, branches) = branches(&repository)?;

        let updates = AheadBehindCache::default().update(&location, &branches);

        assert_eq!(
            state_of(&updates, "main"),
            AheadBehindState::Known {
                ahead: 2,
                behind: 0
            }
        );
        Ok(())
    }

    #[test]
    fn upstream_commits_count_as_behind() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let repository = TempRepo::clone_of(&origin);
        origin.git(&["commit", "--allow-empty", "-m", "remote one"]);
        repository.git(&["fetch", "origin"]);
        let (location, branches) = branches(&repository)?;

        let updates = AheadBehindCache::default().update(&location, &branches);

        assert_eq!(
            state_of(&updates, "main"),
            AheadBehindState::Known {
                ahead: 0,
                behind: 1
            }
        );
        Ok(())
    }

    #[test]
    fn divergence_counts_both_directions() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let repository = TempRepo::clone_of(&origin);
        origin.git(&["commit", "--allow-empty", "-m", "remote one"]);
        repository.git(&["commit", "--allow-empty", "-m", "local one"]);
        repository.git(&["fetch", "origin"]);
        let (location, branches) = branches(&repository)?;

        let updates = AheadBehindCache::default().update(&location, &branches);

        assert_eq!(
            state_of(&updates, "main"),
            AheadBehindState::Known {
                ahead: 1,
                behind: 1
            }
        );
        Ok(())
    }

    #[test]
    fn a_branch_without_an_upstream_produces_no_update() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.git(&["branch", "local-only"]);
        let (location, branches) = branches(&repository)?;

        let updates = AheadBehindCache::default().update(&location, &branches);

        assert!(
            updates.is_empty(),
            "nothing to compare means nothing to report"
        );
        Ok(())
    }

    #[test]
    fn unmoved_tips_reuse_the_cached_result() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let repository = TempRepo::clone_of(&origin);
        let (location, branches) = branches(&repository)?;
        let mut cache = AheadBehindCache::default();

        let first = cache.update(&location, &branches);
        let cached_after_first = cache.len();
        let second = cache.update(&location, &branches);

        assert_eq!(first, second);
        assert_eq!(
            cache.len(),
            cached_after_first,
            "a second pass over unmoved tips adds no cache entries"
        );
        Ok(())
    }

    #[test]
    fn a_moved_tip_is_recomputed() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let repository = TempRepo::clone_of(&origin);
        let mut cache = AheadBehindCache::default();
        let (location, before) = branches(&repository)?;
        cache.update(&location, &before);

        repository.git(&["commit", "--allow-empty", "-m", "local one"]);
        let (location, after) = branches(&repository)?;
        let updates = cache.update(&location, &after);

        assert_eq!(
            state_of(&updates, "main"),
            AheadBehindState::Known {
                ahead: 1,
                behind: 0
            }
        );
        assert_eq!(cache.len(), 2, "the moved tip is a new cache key");
        Ok(())
    }

    #[test]
    fn updates_land_on_the_branches_they_name() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let repository = TempRepo::clone_of(&origin);
        repository.git(&["commit", "--allow-empty", "-m", "local one"]);
        let (location, mut branches) = branches(&repository)?;
        let updates = AheadBehindCache::default().update(&location, &branches);

        apply(&mut branches, &updates);

        let main = branches
            .iter()
            .find(|branch| branch.short_name == "main")
            .expect("main is listed");
        assert_eq!(
            main.ahead_behind,
            AheadBehindState::Known {
                ahead: 1,
                behind: 0
            }
        );
        Ok(())
    }
}
