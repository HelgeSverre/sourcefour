//! Changed-file summaries for a selected commit (§6.10).

use gix::diff::tree_with_rewrites::Change;
use gix_imara_diff::{Algorithm, Diff, InternedInput};
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
pub fn commit_files(
    location: &RepoLocation,
    oid: Oid,
    parent: DiffParent,
) -> Result<CommitFiles, RepoFailure> {
    let repository =
        gix::open(&location.git_dir).map_err(|error| open_failure(&location.git_dir, &error))?;
    let commit = repository
        .find_commit(gix::ObjectId::from_bytes_or_panic(oid.as_bytes()))
        .map_err(|error| missing(oid, &error))?;
    let tree = commit.tree().map_err(|error| missing(oid, &error))?;
    let parent_tree = parent_tree(&repository, &commit, parent)?;

    let changes = repository
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
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
    let mut files: Vec<ChangedFile> = changes
        .into_iter()
        .filter_map(|change| convert(&repository, change))
        .collect();
    files.sort_by_key(sort_path);

    Ok(CommitFiles {
        oid,
        parent: if parent_tree.is_some() {
            parent
        } else {
            DiffParent::EmptyTree
        },
        files,
    })
}

/// Resolves the tree a comparison runs against, per the §6.10 policy.
pub(crate) fn parent_tree<'repo>(
    repository: &'repo gix::Repository,
    commit: &gix::Commit<'repo>,
    parent: DiffParent,
) -> Result<Option<gix::Tree<'repo>>, RepoFailure> {
    let commit_oid = crate::history::convert_oid(commit.id.as_ref()).unwrap_or(Oid::sha1([0; 20]));
    match parent {
        DiffParent::EmptyTree => Ok(None),
        DiffParent::FirstParent => match commit.parent_ids().next() {
            Some(id) => {
                let parent = repository
                    .find_commit(id)
                    .map_err(|error| missing(commit_oid, &error))?;
                Ok(Some(
                    parent.tree().map_err(|error| missing(commit_oid, &error))?,
                ))
            }
            None => Ok(None),
        },
        DiffParent::Parent(chosen) => {
            let parent = repository
                .find_commit(gix::ObjectId::from_bytes_or_panic(chosen.as_bytes()))
                .map_err(|error| missing(chosen, &error))?;
            Ok(Some(
                parent.tree().map_err(|error| missing(chosen, &error))?,
            ))
        }
    }
}

/// Maps one gix change onto the transport shape with line statistics,
/// dropping directory entries. §6.10 allows deferring the counts if they
/// prove slow; measured on real repositories they have not.
fn convert(repository: &gix::Repository, change: Change) -> Option<ChangedFile> {
    let file = match change {
        Change::Addition {
            location,
            entry_mode,
            id,
            ..
        } => {
            if entry_mode.is_tree() {
                return None;
            }
            let (additions, deletions, is_binary) = one_sided_stats(repository, id, false);
            ChangedFile {
                old_path: None,
                new_path: Some(RepoPath(location.into())),
                status: ChangeKind::Added,
                additions,
                deletions,
                is_binary,
            }
        }
        Change::Deletion {
            location,
            entry_mode,
            id,
            ..
        } => {
            if entry_mode.is_tree() {
                return None;
            }
            let (additions, deletions, is_binary) = one_sided_stats(repository, id, true);
            ChangedFile {
                old_path: Some(RepoPath(location.into())),
                new_path: None,
                status: ChangeKind::Deleted,
                additions,
                deletions,
                is_binary,
            }
        }
        Change::Modification {
            location,
            previous_entry_mode,
            entry_mode,
            previous_id,
            id,
        } => {
            // A changed subtree surfaces as a Modification too; a directory
            // has no diff and never belongs in the file list.
            if entry_mode.is_tree() || previous_entry_mode.is_tree() {
                return None;
            }
            let (additions, deletions, is_binary) = modification_stats(repository, previous_id, id);
            ChangedFile {
                old_path: Some(RepoPath(location.clone().into())),
                new_path: Some(RepoPath(location.into())),
                status: ChangeKind::Modified,
                additions,
                deletions,
                is_binary,
            }
        }
        Change::Rewrite {
            source_location,
            location,
            diff,
            copy,
            entry_mode,
            ..
        } => {
            if entry_mode.is_tree() {
                return None;
            }
            ChangedFile {
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
            }
        }
    };
    Some(file)
}

/// Stats for a purely added or purely deleted blob.
fn one_sided_stats(
    repository: &gix::Repository,
    id: gix::ObjectId,
    deleted: bool,
) -> (Option<u32>, Option<u32>, bool) {
    let Some(bytes) = blob_bytes(repository, id) else {
        return (None, None, false);
    };
    if is_binary(&bytes) {
        return (None, None, true);
    }
    let lines = line_count(&bytes);
    if deleted {
        (Some(0), Some(lines), false)
    } else {
        (Some(lines), Some(0), false)
    }
}

/// Real ± counts for a modified blob pair via imara-diff.
fn modification_stats(
    repository: &gix::Repository,
    old_id: gix::ObjectId,
    new_id: gix::ObjectId,
) -> (Option<u32>, Option<u32>, bool) {
    let (Some(old), Some(new)) = (
        blob_bytes(repository, old_id),
        blob_bytes(repository, new_id),
    ) else {
        return (None, None, false);
    };
    if is_binary(&old) || is_binary(&new) {
        return (None, None, true);
    }
    let old_text = String::from_utf8_lossy(&old);
    let new_text = String::from_utf8_lossy(&new);
    let input = InternedInput::new(old_text.as_ref(), new_text.as_ref());
    let diff = Diff::compute(Algorithm::Histogram, &input);
    (
        Some(diff.count_additions()),
        Some(diff.count_removals()),
        false,
    )
}

fn blob_bytes(repository: &gix::Repository, id: gix::ObjectId) -> Option<Vec<u8>> {
    let object = repository.find_object(id).ok()?;
    (object.kind == gix::object::Kind::Blob).then(|| object.data.clone())
}

/// Git's own heuristic: a NUL in the first 8000 bytes means binary.
pub(crate) fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8000)].contains(&0)
}

/// Lines in a text blob; a trailing unterminated line still counts.
#[expect(
    clippy::naive_bytecount,
    reason = "counting once per changed file is nowhere near hot"
)]
fn line_count(bytes: &[u8]) -> u32 {
    if bytes.is_empty() {
        return 0;
    }
    let newlines = bytes.iter().filter(|&&byte| byte == b'\n').count();
    let terminated = bytes.last() == Some(&b'\n');
    u32::try_from(newlines + usize::from(!terminated)).unwrap_or(u32::MAX)
}

/// The path a file sorts and displays under: its new name, or its old one.
fn sort_path(file: &ChangedFile) -> Vec<u8> {
    file.new_path
        .as_ref()
        .or(file.old_path.as_ref())
        .map(|path| path.0.clone())
        .unwrap_or_default()
}

pub(crate) fn missing(oid: Oid, error: &impl std::fmt::Display) -> RepoFailure {
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

        let files = commit_files(
            &discover(repository.path())?,
            head(&repository)?,
            DiffParent::FirstParent,
        )?;

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
        let by_name = |name: &str| {
            files
                .files
                .iter()
                .find(|file| {
                    file.new_path
                        .as_ref()
                        .or(file.old_path.as_ref())
                        .is_some_and(|path| path.display_lossy() == name)
                })
                .cloned()
                .expect("the file is listed")
        };
        assert_eq!(
            (by_name("a.txt").additions, by_name("a.txt").deletions),
            (Some(1), Some(1)),
            "a one-line change counts one addition and one deletion"
        );
        assert_eq!(
            (by_name("c.txt").additions, by_name("c.txt").deletions),
            (Some(1), Some(0)),
            "an added file counts its lines"
        );
        assert_eq!(
            (by_name("b.txt").additions, by_name("b.txt").deletions),
            (Some(0), Some(1)),
            "a deleted file counts its old lines"
        );
        Ok(())
    }

    #[test]
    fn directories_never_appear_as_changed_files() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        std::fs::create_dir(repository.path().join("src"))?;
        std::fs::write(repository.path().join("src").join("lib.rs"), "one\n")?;
        repository.git(&["add", "."]);
        repository.commit("add src");
        std::fs::write(repository.path().join("src").join("lib.rs"), "two\n")?;
        repository.git(&["add", "."]);
        repository.commit("change inside src");

        let files = commit_files(
            &discover(repository.path())?,
            head(&repository)?,
            DiffParent::FirstParent,
        )?;

        let paths: Vec<String> = files
            .files
            .iter()
            .filter_map(|file| file.new_path.as_ref())
            .map(sourcefour_model::RepoPath::display_lossy)
            .collect();
        assert_eq!(
            paths,
            ["src/lib.rs"],
            "the directory itself must not be listed as modified"
        );
        Ok(())
    }

    #[test]
    fn binary_files_are_flagged_and_left_uncounted() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        std::fs::write(repository.path().join("logo.bin"), b"\x00\x01binary")?;
        repository.git(&["add", "."]);
        repository.commit("binary");

        let files = commit_files(
            &discover(repository.path())?,
            head(&repository)?,
            DiffParent::FirstParent,
        )?;

        let file = files.files.first().expect("the binary is listed");
        assert!(file.is_binary);
        assert_eq!((file.additions, file.deletions), (None, None));
        Ok(())
    }

    #[test]
    fn a_merge_can_be_compared_against_its_second_parent() -> Result<(), Box<dyn std::error::Error>>
    {
        let repository = TempRepo::init();
        repository.git(&["checkout", "-q", "-b", "side"]);
        std::fs::write(repository.path().join("side.txt"), "side")?;
        repository.git(&["add", "."]);
        repository.commit("side work");
        repository.git(&["checkout", "-q", "main"]);
        std::fs::write(repository.path().join("mainline.txt"), "main")?;
        repository.git(&["add", "."]);
        repository.commit("mainline work");
        repository.git(&["merge", "-q", "--no-ff", "side", "-m", "merge side"]);
        let side_tip = Oid::from_hex(&repository.git(&["rev-parse", "side"]))?;

        let against_side = commit_files(
            &discover(repository.path())?,
            head(&repository)?,
            DiffParent::Parent(side_tip),
        )?;

        let paths: Vec<String> = against_side
            .files
            .iter()
            .filter_map(|file| file.new_path.as_ref())
            .map(sourcefour_model::RepoPath::display_lossy)
            .collect();
        assert_eq!(
            paths,
            ["mainline.txt"],
            "against the side branch, the merge brought the mainline work"
        );
        Ok(())
    }

    #[test]
    fn a_root_commit_diffs_against_the_empty_tree() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init_unborn();
        std::fs::write(repository.path().join("first.txt"), "hello")?;
        repository.git(&["add", "."]);
        repository.commit("root with a file");

        let files = commit_files(
            &discover(repository.path())?,
            head(&repository)?,
            DiffParent::FirstParent,
        )?;

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

        let files = commit_files(
            &discover(repository.path())?,
            head(&repository)?,
            DiffParent::FirstParent,
        )?;

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

        let failure = commit_files(&location, absent, DiffParent::FirstParent)
            .expect_err("the object does not exist");

        assert_eq!(
            failure.kind,
            sourcefour_model::RepoFailureKind::MissingObject
        );
    }
}
