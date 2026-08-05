//! Bounded unified diffs for one file of one commit (§6.11).
//!
//! The diff itself is computed by imara-diff through gix's blob-diff stack —
//! the fastest engine available, and the one the spec prescribes.

use gix_imara_diff::{Algorithm, BasicLineDiffPrinter, Diff, InternedInput, UnifiedDiffConfig};
use sourcefour_model::{
    DiffContent, DiffLine, DiffLineKind, FileDiff, FileDiffRequest, RepoFailure, RepoLocation,
    RepoPath,
};

use crate::{
    files::{is_binary, missing},
    history::open_failure,
};

/// §6.11 safety thresholds for formatted diff output.
#[derive(Clone, Copy, Debug)]
pub struct DiffLimits {
    /// Maximum formatted bytes before degrading to a summary.
    pub bytes: usize,
    /// Maximum formatted lines before degrading to a summary.
    pub lines: usize,
}

impl Default for DiffLimits {
    /// Effectively unlimited: the viewer virtualizes its lines, so no diff is
    /// too large to render. §6.11's caps remain available through
    /// [`file_diff_with_limits`] for callers that want them.
    fn default() -> Self {
        Self {
            bytes: usize::MAX,
            lines: usize::MAX,
        }
    }
}

/// Reads one file's unified diff, uncapped: the interface virtualizes diff
/// lines, so size never prevents rendering.
///
/// # Errors
///
/// Returns a typed failure when the repository or the commit cannot be read.
/// A path missing from both sides is `DiffContent::Unavailable`, not an error.
pub fn file_diff(
    location: &RepoLocation,
    request: &FileDiffRequest,
) -> Result<FileDiff, RepoFailure> {
    file_diff_with_limits(location, request, DiffLimits::default())
}

/// [`file_diff`] with explicit output caps, primarily for tests.
///
/// # Errors
///
/// See [`file_diff`].
pub fn file_diff_with_limits(
    location: &RepoLocation,
    request: &FileDiffRequest,
    limits: DiffLimits,
) -> Result<FileDiff, RepoFailure> {
    let repository =
        gix::open(&location.git_dir).map_err(|error| open_failure(&location.git_dir, &error))?;
    let commit = repository
        .find_commit(gix::ObjectId::from_bytes_or_panic(request.oid.as_bytes()))
        .map_err(|error| missing(request.oid, &error))?;
    let new_tree = commit
        .tree()
        .map_err(|error| missing(request.oid, &error))?;
    let old_tree = crate::files::parent_tree(&repository, &commit, request.parent)?;

    let old_bytes = blob_at(old_tree.as_ref(), &request.path.0);
    let new_bytes = blob_at(Some(&new_tree), &request.path.0);

    Ok(FileDiff {
        request: request.clone(),
        content: content_for(&request.path, old_bytes, new_bytes, limits),
    })
}

/// One working-tree file's diff: the index against HEAD when `staged`, the
/// filesystem against the index otherwise — the two boundaries staging moves.
///
/// # Errors
///
/// Returns a typed failure when the repository or its index cannot be read.
/// A path missing from both sides is `DiffContent::Unavailable`, not an error.
pub fn worktree_file_diff(
    location: &RepoLocation,
    path: &RepoPath,
    staged: bool,
) -> Result<DiffContent, RepoFailure> {
    let repository =
        gix::open(&location.git_dir).map_err(|error| open_failure(&location.git_dir, &error))?;
    let index = repository
        .index_or_empty()
        .map_err(|error| open_failure(&location.git_dir, &error))?;
    let index_bytes = index
        .entry_by_path(path.0.as_slice().into())
        .and_then(|entry| repository.find_object(entry.id).ok())
        .map(|object| object.data.clone());
    let (old, new) = if staged {
        // An unborn HEAD has no tree, so everything staged reads as added.
        let head_tree = repository
            .head_commit()
            .ok()
            .and_then(|commit| commit.tree().ok());
        (blob_at(head_tree.as_ref(), &path.0), index_bytes)
    } else {
        (index_bytes, worktree_bytes(location, &path.0))
    };
    Ok(content_for(path, old, new, DiffLimits::default()))
}

/// The blob-pair tail every diff shares: image, binary, text, or absent.
fn content_for(
    path: &RepoPath,
    old: Option<Vec<u8>>,
    new: Option<Vec<u8>>,
    limits: DiffLimits,
) -> DiffContent {
    match (old, new) {
        (None, None) => DiffContent::Unavailable {
            message: format!(
                "{} is not present in this comparison.",
                path.display_lossy()
            ),
        },
        (old, new) => {
            if let Some(format) = image_format(&path.0) {
                DiffContent::Image {
                    before: old,
                    after: new,
                    format,
                }
            } else {
                let old = old.unwrap_or_default();
                let new = new.unwrap_or_default();
                if is_binary(&old) || is_binary(&new) {
                    DiffContent::Binary {
                        message: format!("{} is binary.", path.display_lossy()),
                    }
                } else {
                    unified(&old, &new, limits)
                }
            }
        }
    }
}

/// Reads a worktree file, `None` when it is gone or the repository is bare.
fn worktree_bytes(location: &RepoLocation, path: &[u8]) -> Option<Vec<u8>> {
    let root = location.active_worktree_path.as_deref()?;
    std::fs::read(root.join(bytes_as_path(path))).ok()
}

/// A repository-relative byte path as a filesystem component.
fn bytes_as_path(path: &[u8]) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::path::PathBuf::from(std::ffi::OsStr::from_bytes(path))
    }
    #[cfg(not(unix))]
    {
        std::path::PathBuf::from(String::from_utf8_lossy(path).into_owned())
    }
}

/// The lowercased image extension of `path` when the viewer can render it
/// (the formats gpui's image element decodes), normalized to one spelling.
fn image_format(path: &[u8]) -> Option<String> {
    let dot = path.iter().rposition(|&byte| byte == b'.')?;
    let extension = std::str::from_utf8(&path[dot + 1..]).ok()?.to_lowercase();
    match extension.as_str() {
        "png" | "gif" | "webp" | "bmp" => Some(extension),
        "jpg" | "jpeg" => Some(String::from("jpeg")),
        "tif" | "tiff" => Some(String::from("tiff")),
        _ => None,
    }
}

/// The blob bytes at `path` within a tree, when the entry exists and is a blob.
///
/// Walks the path as raw components rather than going through `Path`: Git
/// stores paths as bytes and separates them with `/` on every platform, and
/// only Unix can borrow arbitrary bytes as an `OsStr`.
fn blob_at(tree: Option<&gix::Tree<'_>>, path: &[u8]) -> Option<Vec<u8>> {
    let components = path.split(|byte| *byte == b'/');
    let entry = tree?.lookup_entry(components).ok()??;
    let object = entry.object().ok()?;
    (object.kind == gix::object::Kind::Blob).then(|| object.data.clone())
}

/// Formats a unified diff (3 context lines) and classifies each line.
///
/// Public so callers holding two blobs — the `--demo` fixture, tests — get
/// their lines from the same formatter a repository read goes through.
pub fn unified(old: &[u8], new: &[u8], limits: DiffLimits) -> DiffContent {
    let old_text = String::from_utf8_lossy(old);
    let new_text = String::from_utf8_lossy(new);
    let input = InternedInput::new(old_text.as_ref(), new_text.as_ref());
    let computed = Diff::compute(Algorithm::Histogram, &input);
    let printer = BasicLineDiffPrinter(&input.interner);
    let output = computed
        .unified_diff(&printer, UnifiedDiffConfig::default(), &input)
        .to_string();

    let total_lines = output.lines().count();
    if output.len() > limits.bytes || total_lines > limits.lines {
        return DiffContent::TooLarge {
            bytes: output.len(),
            lines: total_lines,
            byte_limit: limits.bytes,
            line_limit: limits.lines,
        };
    }

    let mut lines = Vec::with_capacity(total_lines);
    let mut old_line = 0_u32;
    let mut new_line = 0_u32;
    for raw in output.lines() {
        let line = if let Some(header) = raw.strip_prefix("@@") {
            (old_line, new_line) = hunk_starts(header).unwrap_or((old_line, new_line));
            DiffLine {
                kind: DiffLineKind::Hunk,
                text: raw.to_owned(),
                old_line: None,
                new_line: None,
            }
        } else if let Some(text) = raw.strip_prefix('+') {
            new_line += 1;
            DiffLine {
                kind: DiffLineKind::Addition,
                text: display_text(text),
                old_line: None,
                new_line: Some(new_line),
            }
        } else if let Some(text) = raw.strip_prefix('-') {
            old_line += 1;
            DiffLine {
                kind: DiffLineKind::Deletion,
                text: display_text(text),
                old_line: Some(old_line),
                new_line: None,
            }
        } else {
            old_line += 1;
            new_line += 1;
            DiffLine {
                kind: DiffLineKind::Context,
                text: display_text(raw.strip_prefix(' ').unwrap_or(raw)),
                old_line: Some(old_line),
                new_line: Some(new_line),
            }
        };
        lines.push(line);
    }
    DiffContent::Text { lines }
}

/// Reads the starting line numbers from a `-a,b +c,d @@` hunk header.
fn hunk_starts(header: &str) -> Option<(u32, u32)> {
    let mut parts = header.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let start = |field: &str| {
        field
            .split(',')
            .next()
            .and_then(|value| value.parse::<u32>().ok())
    };
    // The header names the first line; counters advance before use.
    Some((start(old)?.saturating_sub(1), start(new)?.saturating_sub(1)))
}

/// Normalizes a line for display only (§6.11): trailing CR is not content.
fn display_text(text: &str) -> String {
    text.strip_suffix('\r').unwrap_or(text).to_owned()
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{DiffContent, DiffLineKind, DiffParent, FileDiffRequest, Oid, RepoPath};
    use sourcefour_test_support::TempRepo;

    use super::{DiffLimits, file_diff, file_diff_with_limits, worktree_file_diff};
    use crate::discover;

    fn head(repository: &TempRepo) -> Result<Oid, Box<dyn std::error::Error>> {
        Ok(Oid::from_hex(&repository.git(&["rev-parse", "HEAD"]))?)
    }

    fn request(oid: Oid, path: &str) -> FileDiffRequest {
        FileDiffRequest {
            oid,
            parent: DiffParent::FirstParent,
            path: RepoPath(path.as_bytes().to_vec()),
        }
    }

    #[test]
    fn a_modified_file_diffs_with_line_numbers() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        std::fs::write(repository.path().join("a.txt"), "one\ntwo\nthree\n")?;
        repository.git(&["add", "."]);
        repository.commit("add");
        std::fs::write(repository.path().join("a.txt"), "one\nTWO\nthree\n")?;
        repository.git(&["add", "."]);
        repository.commit("change");

        let diff = file_diff(
            &discover(repository.path())?,
            &request(head(&repository)?, "a.txt"),
        )?;

        let DiffContent::Text { lines } = diff.content else {
            panic!("a small text change renders as text");
        };
        let deletion = lines
            .iter()
            .find(|line| line.kind == DiffLineKind::Deletion)
            .expect("the old line is present");
        assert_eq!(deletion.text, "two");
        assert_eq!(deletion.old_line, Some(2));
        assert_eq!(deletion.new_line, None);
        let addition = lines
            .iter()
            .find(|line| line.kind == DiffLineKind::Addition)
            .expect("the new line is present");
        assert_eq!(addition.text, "TWO");
        assert_eq!(addition.new_line, Some(2));
        assert!(
            lines.iter().any(|line| line.kind == DiffLineKind::Hunk),
            "a hunk header introduces the change"
        );
        Ok(())
    }

    #[test]
    fn an_added_file_is_all_additions() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        std::fs::write(repository.path().join("new.txt"), "fresh\nfile\n")?;
        repository.git(&["add", "."]);
        repository.commit("add file");

        let diff = file_diff(
            &discover(repository.path())?,
            &request(head(&repository)?, "new.txt"),
        )?;

        let DiffContent::Text { lines } = diff.content else {
            panic!("an added text file renders as text");
        };
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.kind == DiffLineKind::Addition)
                .count(),
            2
        );
        assert!(!lines.iter().any(|line| line.kind == DiffLineKind::Deletion));
        Ok(())
    }

    #[test]
    fn binary_content_is_reported_not_rendered() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        std::fs::write(repository.path().join("blob.bin"), b"\x00\x01\x02binary")?;
        repository.git(&["add", "."]);
        repository.commit("binary");

        let diff = file_diff(
            &discover(repository.path())?,
            &request(head(&repository)?, "blob.bin"),
        )?;

        assert!(matches!(diff.content, DiffContent::Binary { .. }));
        Ok(())
    }

    #[test]
    fn a_modified_image_carries_both_sides_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        std::fs::write(repository.path().join("logo.png"), b"\x89PNG-old")?;
        repository.git(&["add", "."]);
        repository.commit("add logo");
        std::fs::write(repository.path().join("logo.png"), b"\x89PNG-new")?;
        repository.git(&["add", "."]);
        repository.commit("update logo");

        let diff = file_diff(
            &discover(repository.path())?,
            &request(head(&repository)?, "logo.png"),
        )?;

        let DiffContent::Image {
            before,
            after,
            format,
        } = diff.content
        else {
            panic!("a .png comparison is an image, not a binary notice");
        };
        assert_eq!(before.as_deref(), Some(b"\x89PNG-old".as_slice()));
        assert_eq!(after.as_deref(), Some(b"\x89PNG-new".as_slice()));
        assert_eq!(format, "png");
        Ok(())
    }

    #[test]
    fn an_added_image_has_no_before_side() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.commit("empty");
        std::fs::write(repository.path().join("photo.JPEG"), b"\xFF\xD8fake")?;
        repository.git(&["add", "."]);
        repository.commit("add photo");

        let diff = file_diff(
            &discover(repository.path())?,
            &request(head(&repository)?, "photo.JPEG"),
        )?;

        let DiffContent::Image {
            before,
            after,
            format,
        } = diff.content
        else {
            panic!("extension matching is case-insensitive");
        };
        assert_eq!(before, None);
        assert_eq!(after.as_deref(), Some(b"\xFF\xD8fake".as_slice()));
        assert_eq!(format, "jpeg");
        Ok(())
    }

    #[test]
    fn oversized_output_is_capped_with_a_summary() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        std::fs::write(
            repository.path().join("big.txt"),
            "1\n2\n3\n4\n5\n6\n7\n8\n",
        )?;
        repository.git(&["add", "."]);
        repository.commit("big");

        let diff = file_diff_with_limits(
            &discover(repository.path())?,
            &request(head(&repository)?, "big.txt"),
            DiffLimits {
                bytes: 1024 * 1024,
                lines: 4,
            },
        )?;

        let DiffContent::TooLarge {
            lines, line_limit, ..
        } = diff.content
        else {
            panic!("output beyond the caps must degrade, not freeze");
        };
        assert!(lines > 4);
        assert_eq!(line_limit, 4);
        Ok(())
    }

    #[test]
    fn default_limits_render_a_very_large_diff_in_full() -> Result<(), Box<dyn std::error::Error>> {
        use std::fmt::Write as _;

        let repository = TempRepo::init();
        let big: String = (1..=25_000).fold(String::new(), |mut text, line| {
            let _ = writeln!(text, "line {line}");
            text
        });
        std::fs::write(repository.path().join("big.txt"), big)?;
        repository.git(&["add", "."]);
        repository.commit("vendor a generated file");

        let diff = file_diff(
            &discover(repository.path())?,
            &request(head(&repository)?, "big.txt"),
        )?;

        let DiffContent::Text { lines } = diff.content else {
            panic!("no diff is too large to render by default");
        };
        assert!(lines.len() > 25_000, "every line is present");
        Ok(())
    }

    #[test]
    fn worktree_diffs_track_both_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        std::fs::write(repository.path().join("a.txt"), "one\ntwo\n")?;
        repository.git(&["add", "."]);
        repository.commit("base");
        // Staged: two -> TWO. Unstaged on top: a third line.
        std::fs::write(repository.path().join("a.txt"), "one\nTWO\n")?;
        repository.git(&["add", "."]);
        std::fs::write(repository.path().join("a.txt"), "one\nTWO\nthree\n")?;

        let location = discover(repository.path())?;
        let path = RepoPath(b"a.txt".to_vec());

        let DiffContent::Text { lines } = worktree_file_diff(&location, &path, true)? else {
            panic!("the staged comparison renders as text");
        };
        assert!(
            lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Addition && line.text == "TWO"),
            "staged compares HEAD to the index"
        );
        assert!(
            !lines.iter().any(|line| line.text == "three"),
            "the unstaged edit is invisible to the staged side"
        );

        let DiffContent::Text { lines } = worktree_file_diff(&location, &path, false)? else {
            panic!("the unstaged comparison renders as text");
        };
        assert!(
            lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Addition && line.text == "three"),
            "unstaged compares the index to the filesystem"
        );
        assert!(
            !lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Deletion && line.text == "two"),
            "the staged edit is already in the unstaged baseline"
        );
        Ok(())
    }

    #[test]
    fn an_untracked_file_is_all_additions_unstaged() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.commit("empty");
        std::fs::write(repository.path().join("fresh.txt"), "brand\nnew\n")?;

        let location = discover(repository.path())?;
        let path = RepoPath(b"fresh.txt".to_vec());

        let DiffContent::Text { lines } = worktree_file_diff(&location, &path, false)? else {
            panic!("an untracked text file renders as text");
        };
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.kind == DiffLineKind::Addition)
                .count(),
            2
        );
        assert!(
            matches!(
                worktree_file_diff(&location, &path, true)?,
                DiffContent::Unavailable { .. }
            ),
            "an untracked path has no staged comparison"
        );
        Ok(())
    }

    #[test]
    fn a_missing_path_is_unavailable_not_an_error() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.commit("empty");

        let diff = file_diff(
            &discover(repository.path())?,
            &request(head(&repository)?, "ghost.txt"),
        )?;

        assert!(matches!(diff.content, DiffContent::Unavailable { .. }));
        Ok(())
    }
}
