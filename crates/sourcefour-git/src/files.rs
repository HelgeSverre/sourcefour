//! Changed-file summaries for a selected commit (§6.10).

use gix::diff::tree_with_rewrites::Change;
use sourcefour_model::{
    ChangeKind, ChangedFile, CommitFiles, DiffParent, Oid, RepoFailure, RepoFailureKind,
    RepoLocation, RepoPath,
};

use crate::history::open_failure;

/// Reads the tree-to-tree change summary for one commit against its first
/// parent, or against the empty tree for a root (§6.10).
///
/// Line counts are only present where gix's rename tracking already computed
/// them; exact per-file counts arrive with the diff feature.
///
/// # Errors
///
/// Returns a typed failure when the repository, the commit, or its trees
/// cannot be read.
pub fn commit_files(location: &RepoLocation, oid: Oid) -> Result<CommitFiles, RepoFailure> {
    let repository =
        gix::open(&location.git_dir).map_err(|error| open_failure(&location.git_dir, &error))?;
    let commit = repository
        .find_commit(gix::ObjectId::from_bytes_or_panic(oid.as_bytes()))
        .map_err(|error| missing(oid, &error))?;
    let tree = commit.tree().map_err(|error| missing(oid, &error))?;
    let parent = match commit.parent_ids().next() {
        Some(id) => {
            let parent = repository
                .find_commit(id)
                .map_err(|error| missing(oid, &error))?;
            Some(parent.tree().map_err(|error| missing(oid, &error))?)
        }
        None => None,
    };

    let changes = repository
        .diff_tree_to_tree(parent.as_ref(), Some(&tree), None)
        .map_err(|error| {
            RepoFailure::new(
                RepoFailureKind::Internal,
                "Changed files could not be read",
                format!(
                    "The changes in commit {} could not be compared.",
                    oid.abbreviated(9)
                ),
            )
            .with_details(error.to_string())
        })?;
    let mut files: Vec<ChangedFile> = changes.into_iter().filter_map(convert).collect();
    files.sort_by_key(sort_path);

    Ok(CommitFiles {
        oid,
        parent: if parent.is_some() {
            DiffParent::FirstParent
        } else {
            DiffParent::EmptyTree
        },
        files,
    })
}

/// Maps one gix change onto the transport shape, dropping directory entries.
fn convert(change: Change) -> Option<ChangedFile> {
    let file = match change {
        Change::Addition {
            location,
            entry_mode,
            ..
        } => {
            if entry_mode.is_tree() {
                return None;
            }
            ChangedFile {
                old_path: None,
                new_path: Some(RepoPath(location.into())),
                status: ChangeKind::Added,
                additions: None,
                deletions: None,
                is_binary: false,
            }
        }
        Change::Deletion {
            location,
            entry_mode,
            ..
        } => {
            if entry_mode.is_tree() {
                return None;
            }
            ChangedFile {
                old_path: Some(RepoPath(location.into())),
                new_path: None,
                status: ChangeKind::Deleted,
                additions: None,
                deletions: None,
                is_binary: false,
            }
        }
        Change::Modification { location, .. } => ChangedFile {
            old_path: Some(RepoPath(location.clone().into())),
            new_path: Some(RepoPath(location.into())),
            status: ChangeKind::Modified,
            additions: None,
            deletions: None,
            is_binary: false,
        },
        Change::Rewrite {
            source_location,
            location,
            diff,
            copy,
            ..
        } => ChangedFile {
            old_path: Some(RepoPath(source_location.into())),
            new_path: Some(RepoPath(location.into())),
            status: if copy {
                ChangeKind::Copied
            } else {
                ChangeKind::Renamed
            },
            additions: diff.map(|diff| diff.insertions),
            deletions: diff.map(|diff| diff.removals),
            is_binary: false,
        },
    };
    Some(file)
}

/// The path a file sorts and displays under: its new name, or its old one.
fn sort_path(file: &ChangedFile) -> Vec<u8> {
    file.new_path
        .as_ref()
        .or(file.old_path.as_ref())
        .map(|path| path.0.clone())
        .unwrap_or_default()
}

fn missing(oid: Oid, error: &impl std::fmt::Display) -> RepoFailure {
    RepoFailure::new(
        RepoFailureKind::MissingObject,
        "Commit could not be read",
        format!(
            "Commit {} could not be found or decoded.",
            oid.abbreviated(9)
        ),
    )
    .with_details(error.to_string())
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{ChangeKind, DiffParent, Oid};
    use sourcefour_test_support::TempRepo;

    use super::commit_files;
    use crate::discover;

    fn head(repository: &TempRepo) -> Result<Oid, Box<dyn std::error::Error>> {
        Ok(Oid::from_hex(&repository.git(&["rev-parse", "HEAD"]))?)
    }

    #[test]
    fn a_commit_reports_added_modified_and_deleted_paths() -> Result<(), Box<dyn std::error::Error>>
    {
        let repository = TempRepo::init();
        std::fs::write(repository.path().join("a.txt"), "one")?;
        std::fs::write(repository.path().join("b.txt"), "two")?;
        repository.git(&["add", "."]);
        repository.commit("add files");
        std::fs::write(repository.path().join("a.txt"), "changed")?;
        std::fs::remove_file(repository.path().join("b.txt"))?;
        std::fs::write(repository.path().join("c.txt"), "three")?;
        repository.git(&["add", "-A"]);
        repository.commit("change files");

        let files = commit_files(&discover(repository.path())?, head(&repository)?)?;

        let summary: Vec<(String, ChangeKind)> = files
            .files
            .iter()
            .map(|file| {
                let path = file
                    .new_path
                    .as_ref()
                    .or(file.old_path.as_ref())
                    .expect("every change names a path");
                (path.display_lossy(), file.status)
            })
            .collect();
        assert_eq!(
            summary,
            [
                (String::from("a.txt"), ChangeKind::Modified),
                (String::from("b.txt"), ChangeKind::Deleted),
                (String::from("c.txt"), ChangeKind::Added),
            ],
            "statuses are correct and paths arrive sorted"
        );
        assert_eq!(files.parent, DiffParent::FirstParent);
        Ok(())
    }

    #[test]
    fn a_root_commit_diffs_against_the_empty_tree() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init_unborn();
        std::fs::write(repository.path().join("first.txt"), "hello")?;
        repository.git(&["add", "."]);
        repository.commit("root with a file");

        let files = commit_files(&discover(repository.path())?, head(&repository)?)?;

        assert_eq!(files.parent, DiffParent::EmptyTree);
        assert_eq!(files.files.len(), 1);
        assert_eq!(files.files[0].status, ChangeKind::Added);
        Ok(())
    }

    #[test]
    fn a_merge_shows_what_it_brought_into_the_first_parent()
    -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.git(&["checkout", "-q", "-b", "side"]);
        std::fs::write(repository.path().join("side.txt"), "from the side branch")?;
        repository.git(&["add", "."]);
        repository.commit("side work");
        repository.git(&["checkout", "-q", "main"]);
        repository.commit("mainline work");
        repository.git(&["merge", "-q", "--no-ff", "side", "-m", "merge side"]);

        let files = commit_files(&discover(repository.path())?, head(&repository)?)?;

        let paths: Vec<String> = files
            .files
            .iter()
            .filter_map(|file| file.new_path.as_ref())
            .map(sourcefour_model::RepoPath::display_lossy)
            .collect();
        assert_eq!(
            paths,
            ["side.txt"],
            "a merge is summarized against its first parent (§6.10)"
        );
        Ok(())
    }

    #[test]
    fn a_missing_object_is_a_typed_failure() {
        let repository = TempRepo::init();
        let location = discover(repository.path()).expect("fixture repository discovers");
        let absent = Oid::sha1([0xEE; 20]);

        let failure = commit_files(&location, absent).expect_err("the object does not exist");

        assert_eq!(
            failure.kind,
            sourcefour_model::RepoFailureKind::MissingObject
        );
    }
}
