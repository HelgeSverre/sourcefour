//! Metadata-only change detection.
//!
//! §6.14 forbids watching the working directory: this is a history browser, and
//! a recursive watcher is expensive on large repositories. Only the paths whose
//! contents the sidebar actually renders are examined.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::SystemTime,
};

use sourcefour_model::RepoLocation;

/// What changed since the last poll.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetadataChange {
    /// Reference names or targets changed.
    Refs,
    /// Worktree registration or metadata changed.
    Worktrees,
    /// Both metadata groups changed since the last poll.
    RefsAndWorktrees,
}

/// Polls the metadata paths §6.14 lists, and nothing else.
///
/// Detection compares a fingerprint of those paths rather than subscribing to
/// filesystem events, which keeps behavior identical on every platform and
/// makes the whole thing testable without a watcher daemon.
#[derive(Clone, Debug)]
pub struct MetadataWatcher {
    refs: Fingerprint,
    worktrees: Fingerprint,
    ref_paths: Vec<PathBuf>,
    worktree_paths: Vec<PathBuf>,
}

impl MetadataWatcher {
    /// Starts watching, taking the current state as the baseline.
    #[must_use]
    pub fn new(location: &RepoLocation) -> Self {
        let common = location.common_dir.clone();
        let ref_paths = vec![
            location.git_dir.join("HEAD"),
            // The index lives in the Git directory, so §6.14's no-worktree
            // rule holds: staging or committing in a terminal must surface,
            // edits to tracked files need not.
            location.git_dir.join("index"),
            common.join("refs"),
            common.join("packed-refs"),
            common.join("config"),
        ];
        let worktree_paths = vec![common.join("worktrees")];
        Self {
            refs: fingerprint(&ref_paths),
            worktrees: fingerprint(&worktree_paths),
            ref_paths,
            worktree_paths,
        }
    }

    /// Returns the coalesced change since the previous poll, if any.
    pub fn poll(&mut self) -> Option<MetadataChange> {
        let refs = fingerprint(&self.ref_paths);
        let worktrees = fingerprint(&self.worktree_paths);
        let refs_changed = refs != self.refs;
        let worktrees_changed = worktrees != self.worktrees;
        self.refs = refs;
        self.worktrees = worktrees;
        match (refs_changed, worktrees_changed) {
            (true, true) => Some(MetadataChange::RefsAndWorktrees),
            (true, false) => Some(MetadataChange::Refs),
            (false, true) => Some(MetadataChange::Worktrees),
            (false, false) => None,
        }
    }
}

fn fingerprint(roots: &[PathBuf]) -> Fingerprint {
    let mut entries = BTreeMap::new();
    for root in roots {
        collect(root, &mut entries);
    }
    Fingerprint(entries)
}

/// Modification times of every metadata file, keyed by path.
///
/// Sizes and times both participate so a same-second rewrite of a ref file is
/// still seen; refs are small and rewritten wholesale.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Fingerprint(BTreeMap<PathBuf, (SystemTime, u64)>);

/// Walks one metadata path, recursing only inside `refs/` and `worktrees/`.
fn collect(path: &Path, entries: &mut BTreeMap<PathBuf, (SystemTime, u64)>) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_dir() {
        let Ok(children) = std::fs::read_dir(path) else {
            return;
        };
        for child in children.filter_map(Result::ok) {
            collect(&child.path(), entries);
        }
        return;
    }
    let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    entries.insert(path.to_path_buf(), (modified, metadata.len()));
}

#[cfg(test)]
mod tests {
    use sourcefour_test_support::TempRepo;

    use super::{MetadataChange, MetadataWatcher};
    use crate::discover;

    #[test]
    fn an_unchanged_repository_reports_nothing() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let mut watcher = MetadataWatcher::new(&discover(repository.path())?);

        assert_eq!(watcher.poll(), None);
        assert_eq!(watcher.poll(), None);
        Ok(())
    }

    #[test]
    fn an_externally_created_branch_is_seen() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let mut watcher = MetadataWatcher::new(&discover(repository.path())?);

        repository.git(&["branch", "created-outside"]);

        assert_eq!(watcher.poll(), Some(MetadataChange::Refs));
        assert_eq!(watcher.poll(), None, "the change is reported once");
        Ok(())
    }

    #[test]
    fn an_externally_deleted_branch_is_seen() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.git(&["branch", "doomed"]);
        let mut watcher = MetadataWatcher::new(&discover(repository.path())?);

        repository.git(&["branch", "-D", "doomed"]);

        assert_eq!(watcher.poll(), Some(MetadataChange::Refs));
        Ok(())
    }

    #[test]
    fn an_externally_added_worktree_is_seen() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let mut watcher = MetadataWatcher::new(&discover(repository.path())?);

        let _linked = repository.add_worktree("side");

        assert_eq!(
            watcher.poll(),
            Some(MetadataChange::RefsAndWorktrees),
            "adding a worktree creates both its branch and its registration"
        );
        Ok(())
    }

    #[test]
    fn a_packed_ref_change_is_seen() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.git(&["branch", "to-pack"]);
        let mut watcher = MetadataWatcher::new(&discover(repository.path())?);

        repository.git(&["pack-refs", "--all"]);

        assert_eq!(watcher.poll(), Some(MetadataChange::Refs));
        Ok(())
    }

    #[test]
    fn working_tree_edits_are_ignored() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let mut watcher = MetadataWatcher::new(&discover(repository.path())?);

        std::fs::write(repository.path().join("scratch.txt"), "not metadata")?;

        assert_eq!(
            watcher.poll(),
            None,
            "a history browser must not react to working-copy edits (§6.14)"
        );
        Ok(())
    }
}
