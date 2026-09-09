//! One metadata pass over references: branches, remotes, tags, and labels.
//!
//! Ahead/behind is deliberately left `Pending` here. §6.7 forbids blocking the
//! first render on it, so the counts arrive later as their own event.

use std::collections::HashMap;

use gix::refs::TargetRef;
use smallvec::SmallVec;
use sourcefour_model::{
    AheadBehindState, BranchSnapshot, HeadSnapshot, LabelsByObject, Oid, RefKind, RefLabel,
    RemoteBranchSnapshot, RemoteSnapshot, RepoFailure, RepoFailureKind, RepoLocation,
    UpstreamSnapshot, WorktreeId, WorktreeSnapshot,
};

/// Everything one reference pass produces.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct References {
    /// Local branches in alphabetical order.
    pub local_branches: Vec<BranchSnapshot>,
    /// Remotes in alphabetical order, each with its own branches.
    pub remotes: Vec<RemoteSnapshot>,
    /// Number of tags, which the sidebar shows as a count only.
    pub tags_count: usize,
    /// Labels to paint on history rows, keyed by the commit they point at.
    pub labels_by_object: LabelsByObject,
}

/// Reads every reference the sidebar and history labels need.
///
/// `worktrees` supplies the branch-to-worktree mapping, so a branch checked out
/// elsewhere can be shown as such rather than appearing freely checkoutable.
///
/// # Errors
///
/// Returns a typed failure when the reference store cannot be read.
pub fn references(
    location: &RepoLocation,
    worktrees: &[WorktreeSnapshot],
) -> Result<References, RepoFailure> {
    // Open the invocation's own Git directory, not the common one: refs are
    // shared but HEAD is private to the active worktree (§6.2).
    let repository = gix::open(&location.git_dir).map_err(|error| {
        failure(
            "Repository could not be opened",
            format!(
                "The repository at {} could not be read.",
                location.git_dir.display()
            ),
            &error,
        )
    })?;

    let checked_out = checked_out_branches(worktrees);
    let current_branch = worktrees
        .iter()
        .find(|tree| tree.is_current)
        .and_then(|tree| match &tree.head {
            HeadSnapshot::Branch { full_name, .. } => Some(full_name.clone()),
            _ => None,
        });

    let platform = repository.references().map_err(|error| {
        failure(
            "References could not be read",
            String::from("The repository's reference store could not be opened."),
            &error,
        )
    })?;

    let mut labels: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
    let local_branches = read_local_branches(
        &repository,
        &platform,
        current_branch.as_deref(),
        &checked_out,
        &mut labels,
    );

    let remote_names: Vec<String> = repository
        .remote_names()
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    let (mut remote_branches, mut default_branches) =
        read_remote_branches(&platform, &remote_names, &mut labels);
    let tags_count = read_tags(&platform, &mut labels);

    if let Some(tip) = head_commit(&repository) {
        labels.entry(tip).or_default().push(RefLabel {
            full_name: Some(String::from("HEAD")),
            name: String::from("HEAD"),
            kind: RefKind::Head,
            is_head: true,
            is_current: false,
        });
    }

    let remotes = remote_names
        .into_iter()
        .map(|name| RemoteSnapshot {
            fetch_url: fetch_url(&repository, &name),
            default_branch: default_branches.remove(&name),
            branches: {
                let mut branches = remote_branches.remove(&name).unwrap_or_default();
                branches.sort_by(|left, right| left.short_name.cmp(&right.short_name));
                branches
            },
            name,
        })
        .collect();

    Ok(References {
        local_branches,
        remotes,
        tags_count,
        labels_by_object: ordered_labels(labels),
    })
}

type Platform<'a> = gix::reference::iter::Platform<'a>;
type Labels = HashMap<Oid, Vec<RefLabel>>;

fn read_local_branches(
    repository: &gix::Repository,
    platform: &Platform<'_>,
    current_branch: Option<&str>,
    checked_out: &HashMap<String, WorktreeId>,
    labels: &mut Labels,
) -> Vec<BranchSnapshot> {
    let Ok(iter) = platform.local_branches() else {
        return Vec::new();
    };
    let mut branches = Vec::new();
    for mut reference in iter.filter_map(Result::ok) {
        let Some(tip) = peeled(&mut reference) else {
            continue;
        };
        let full_name = reference.name().as_bstr().to_string();
        let short_name = reference.name().shorten().to_string();
        let is_current = current_branch == Some(full_name.as_str());
        let upstream = upstream_of(repository, &reference);
        labels.entry(tip).or_default().push(RefLabel {
            full_name: Some(full_name.clone()),
            name: short_name.clone(),
            kind: RefKind::LocalBranch,
            is_head: false,
            is_current,
        });
        branches.push(BranchSnapshot {
            // §6.7: a branch with an upstream is queued, never computed here.
            ahead_behind: if upstream.is_some() {
                AheadBehindState::Pending
            } else {
                AheadBehindState::Unavailable
            },
            checked_out_in: checked_out.get(&full_name).cloned(),
            upstream,
            is_current,
            tip,
            full_name,
            short_name,
        });
    }
    branches.sort_by(|left, right| left.short_name.cmp(&right.short_name));
    branches
}

/// Returns branches grouped by remote, and each remote's default branch.
fn read_remote_branches(
    platform: &Platform<'_>,
    remote_names: &[String],
    labels: &mut Labels,
) -> (
    HashMap<String, Vec<RemoteBranchSnapshot>>,
    HashMap<String, String>,
) {
    let mut branches: HashMap<String, Vec<RemoteBranchSnapshot>> = HashMap::new();
    let mut defaults = HashMap::new();
    let Ok(iter) = platform.remote_branches() else {
        return (branches, defaults);
    };
    for mut reference in iter.filter_map(Result::ok) {
        let full_name = reference.name().as_bstr().to_string();
        let Some(remote) = owning_remote(remote_names, &full_name) else {
            continue;
        };
        // `<remote>/HEAD` is symbolic: it marks the remote's default branch
        // rather than being a branch of its own (§6.6).
        if let TargetRef::Symbolic(target) = reference.target() {
            defaults.insert(remote, target.shorten().to_string());
            continue;
        }
        let Some(tip) = peeled(&mut reference) else {
            continue;
        };
        let short_name = reference.name().shorten().to_string();
        labels.entry(tip).or_default().push(RefLabel {
            full_name: Some(full_name.clone()),
            name: short_name.clone(),
            kind: RefKind::RemoteBranch,
            is_head: false,
            is_current: false,
        });
        branches
            .entry(remote)
            .or_default()
            .push(RemoteBranchSnapshot {
                full_name,
                short_name,
                tip,
            });
    }
    (branches, defaults)
}

/// Counts tags and labels the commits they peel to.
fn read_tags(platform: &Platform<'_>, labels: &mut Labels) -> usize {
    let Ok(iter) = platform.tags() else {
        return 0;
    };
    let mut count = 0;
    for mut reference in iter.filter_map(Result::ok) {
        count += 1;
        // Annotated tags peel through the tag object onto the commit so the
        // label lands on a history row.
        let Some(tip) = peeled(&mut reference) else {
            continue;
        };
        labels.entry(tip).or_default().push(RefLabel {
            full_name: Some(reference.name().as_bstr().to_string()),
            name: reference.name().shorten().to_string(),
            kind: RefKind::Tag,
            is_head: false,
            is_current: false,
        });
    }
    count
}

/// Applies §6.6's stable order: HEAD, the current branch, other local branches,
/// remotes, then tags; alphabetical within each group.
fn ordered_labels(labels: HashMap<Oid, Vec<RefLabel>>) -> LabelsByObject {
    labels
        .into_iter()
        .map(|(oid, mut labels)| {
            labels.sort_by(|left, right| {
                rank(left)
                    .cmp(&rank(right))
                    .then_with(|| left.name.cmp(&right.name))
            });
            (oid, SmallVec::from_vec(labels))
        })
        .collect()
}

fn rank(label: &RefLabel) -> u8 {
    match label.kind {
        RefKind::Head => 0,
        RefKind::LocalBranch if label.is_current => 1,
        RefKind::LocalBranch => 2,
        RefKind::RemoteBranch => 3,
        RefKind::Tag => 4,
        RefKind::Other => 5,
    }
}

/// Maps each branch that is checked out somewhere to the worktree holding it.
fn checked_out_branches(worktrees: &[WorktreeSnapshot]) -> HashMap<String, WorktreeId> {
    worktrees
        .iter()
        .filter_map(|tree| match &tree.head {
            HeadSnapshot::Branch { full_name, .. } => Some((full_name.clone(), tree.id.clone())),
            _ => None,
        })
        .collect()
}

/// Longest matching remote name, so a remote named `a/b` beats one named `a`.
fn owning_remote(remote_names: &[String], full_name: &str) -> Option<String> {
    remote_names
        .iter()
        .filter(|remote| full_name.starts_with(&format!("refs/remotes/{remote}/")))
        .max_by_key(|remote| remote.len())
        .cloned()
}

fn upstream_of(
    repository: &gix::Repository,
    reference: &gix::Reference<'_>,
) -> Option<UpstreamSnapshot> {
    let tracking = repository
        .branch_remote_tracking_ref_name(reference.name(), gix::remote::Direction::Fetch)?
        .ok()?;
    let mut upstream = repository.find_reference(tracking.as_ref()).ok()?;
    let tip = peeled(&mut upstream)?;
    Some(UpstreamSnapshot {
        full_name: upstream.name().as_bstr().to_string(),
        short_name: upstream.name().shorten().to_string(),
        tip,
    })
}

fn fetch_url(repository: &gix::Repository, name: &str) -> Option<String> {
    repository
        .find_remote(name)
        .ok()?
        .url(gix::remote::Direction::Fetch)
        .map(std::string::ToString::to_string)
}

fn head_commit(repository: &gix::Repository) -> Option<Oid> {
    let mut head = repository.head().ok()?;
    head.peel_to_commit()
        .ok()
        .and_then(|commit| convert_oid(commit.id().as_ref()))
}

fn peeled(reference: &mut gix::Reference<'_>) -> Option<Oid> {
    reference
        .peel_to_id()
        .ok()
        .and_then(|id| convert_oid(id.as_ref()))
}

fn convert_oid(id: &gix::hash::oid) -> Option<Oid> {
    match id.as_bytes().len() {
        20 => Some(Oid::sha1(id.as_bytes().try_into().ok()?)),
        32 => Some(Oid::sha256(id.as_bytes().try_into().ok()?)),
        _ => None,
    }
}

fn failure(title: &str, message: String, error: &impl std::fmt::Display) -> RepoFailure {
    RepoFailure::new(RepoFailureKind::CorruptRepository, title, message)
        .with_details(error.to_string())
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{AheadBehindState, RefKind};
    use sourcefour_test_support::TempRepo;

    use super::{References, references};
    use crate::{discover, worktrees};

    fn read(repository: &TempRepo) -> Result<References, Box<dyn std::error::Error>> {
        let location = discover(repository.path())?;
        let trees = worktrees(&location)?;
        Ok(references(&location, &trees)?)
    }

    #[test]
    fn local_branches_are_alphabetical_and_mark_the_current_one()
    -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.git(&["branch", "zeta"]);
        repository.git(&["branch", "alpha"]);

        let found = read(&repository)?;

        let names: Vec<_> = found
            .local_branches
            .iter()
            .map(|branch| branch.short_name.as_str())
            .collect();
        assert_eq!(names, ["alpha", "main", "zeta"]);
        let current: Vec<_> = found
            .local_branches
            .iter()
            .filter(|branch| branch.is_current)
            .map(|branch| branch.short_name.as_str())
            .collect();
        assert_eq!(current, ["main"]);
        Ok(())
    }

    #[test]
    fn a_branch_checked_out_elsewhere_names_its_worktree() -> Result<(), Box<dyn std::error::Error>>
    {
        let repository = TempRepo::init();
        let linked = repository.add_worktree("side");
        let location = discover(repository.path())?;
        let trees = worktrees(&location)?;
        let linked_id = trees
            .iter()
            .find(|tree| tree.path == linked)
            .map(|tree| tree.id.clone())
            .expect("the linked worktree is listed");

        let found = references(&location, &trees)?;

        let side = found
            .local_branches
            .iter()
            .find(|branch| branch.short_name == "side")
            .expect("the linked branch is listed");
        assert_eq!(side.checked_out_in.as_ref(), Some(&linked_id));
        assert!(!side.is_current, "it is current only in its own worktree");
        Ok(())
    }

    #[test]
    fn ahead_behind_starts_pending_and_never_blocks_the_first_pass()
    -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let repository = TempRepo::clone_of(&origin);

        let found = read(&repository)?;

        let main = found
            .local_branches
            .iter()
            .find(|branch| branch.short_name == "main")
            .expect("main is listed");
        assert_eq!(
            main.upstream.as_ref().map(|up| up.short_name.as_str()),
            Some("origin/main")
        );
        assert_eq!(main.ahead_behind, AheadBehindState::Pending);
        Ok(())
    }

    #[test]
    fn a_branch_without_an_upstream_is_unavailable_not_pending()
    -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.git(&["branch", "local-only"]);

        let found = read(&repository)?;

        let local_only = found
            .local_branches
            .iter()
            .find(|branch| branch.short_name == "local-only")
            .expect("the branch is listed");
        assert_eq!(local_only.upstream, None);
        assert_eq!(local_only.ahead_behind, AheadBehindState::Unavailable);
        Ok(())
    }

    #[test]
    fn remotes_carry_their_fetch_url_and_branches() -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let repository = TempRepo::clone_of(&origin);

        let found = read(&repository)?;

        assert_eq!(found.remotes.len(), 1);
        let remote = &found.remotes[0];
        assert_eq!(remote.name, "origin");
        assert!(remote.fetch_url.is_some());
        let names: Vec<_> = remote
            .branches
            .iter()
            .map(|branch| branch.short_name.as_str())
            .collect();
        assert_eq!(names, ["origin/main"]);
        Ok(())
    }

    #[test]
    fn a_remote_symbolic_head_is_a_default_marker_not_a_branch()
    -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let repository = TempRepo::clone_of(&origin);
        // `git clone` writes refs/remotes/origin/HEAD; it must not be listed.
        repository.git(&["remote", "set-head", "origin", "main"]);

        let found = read(&repository)?;

        let remote = &found.remotes[0];
        assert!(
            !remote
                .branches
                .iter()
                .any(|branch| branch.short_name.ends_with("HEAD")),
            "origin/HEAD is not an ordinary remote branch"
        );
        assert_eq!(remote.default_branch.as_deref(), Some("origin/main"));
        Ok(())
    }

    #[test]
    fn annotated_tags_are_peeled_onto_their_commit() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.git(&["tag", "-a", "v1.0", "-m", "release"]);
        repository.git(&["tag", "lightweight"]);
        let head = repository.git(&["rev-parse", "HEAD"]);

        let found = read(&repository)?;

        assert_eq!(found.tags_count, 2);
        let labels = found
            .labels_by_object
            .iter()
            .find(|(oid, _)| oid.to_hex() == head)
            .map(|(_, labels)| labels)
            .expect("HEAD carries labels");
        let tags: Vec<_> = labels
            .iter()
            .filter(|label| label.kind == RefKind::Tag)
            .map(|label| label.name.as_str())
            .collect();
        assert_eq!(tags, ["lightweight", "v1.0"]);
        Ok(())
    }

    #[test]
    fn labels_are_ordered_head_then_branches_then_remotes_then_tags()
    -> Result<(), Box<dyn std::error::Error>> {
        let origin = TempRepo::init();
        let repository = TempRepo::clone_of(&origin);
        repository.git(&["branch", "aardvark"]);
        repository.git(&["tag", "v1.0"]);
        let head = repository.git(&["rev-parse", "HEAD"]);

        let found = read(&repository)?;

        let labels = found
            .labels_by_object
            .iter()
            .find(|(oid, _)| oid.to_hex() == head)
            .map(|(_, labels)| labels)
            .expect("HEAD carries labels");
        let kinds: Vec<_> = labels.iter().map(|label| label.kind).collect();
        assert_eq!(
            kinds,
            [
                RefKind::Head,
                RefKind::LocalBranch,
                RefKind::LocalBranch,
                RefKind::RemoteBranch,
                RefKind::Tag
            ]
        );
        assert!(labels[0].is_head);
        // The current branch precedes other local branches despite sorting later.
        assert_eq!(labels[1].name, "main");
        assert!(labels[1].is_current);
        assert_eq!(labels[2].name, "aardvark");
        Ok(())
    }

    #[test]
    fn an_unborn_repository_reads_no_references() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init_unborn();

        let found = read(&repository)?;

        assert!(found.local_branches.is_empty());
        assert!(found.remotes.is_empty());
        assert!(found.labels_by_object.is_empty());
        assert_eq!(found.tags_count, 0);
        Ok(())
    }
}
