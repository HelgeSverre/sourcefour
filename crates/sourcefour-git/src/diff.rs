//! Bounded unified diffs for one file of one commit (§6.11).
//!
//! The diff itself is computed by imara-diff through gix's blob-diff stack —
//! the fastest engine available, and the one the spec prescribes.

use std::path::Path;

use gix_imara_diff::{Algorithm, Diff, InternedInput};
use sourcefour_model::{
    DiffContent, DiffPaths, FileDiff, FileDiffRequest, LinePair, RepoFailure, RepoLocation,
    RepoPath, TextChange, TextDecoding, TextDiff, TextSide, VideoInfo,
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
/// `ffmpeg_dir` is where the user says their video tools live, consulted only
/// when the compared path is a video and only on the first one of the session
/// — see [`crate::media`].
///
/// # Errors
///
/// Returns a typed failure when the repository or the commit cannot be read.
/// A path missing from both sides is `DiffContent::Unavailable`, not an error.
pub fn file_diff(
    location: &RepoLocation,
    request: &FileDiffRequest,
    ffmpeg_dir: Option<&Path>,
) -> Result<FileDiff, RepoFailure> {
    file_diff_with_limits(location, request, DiffLimits::default(), ffmpeg_dir)
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
    ffmpeg_dir: Option<&Path>,
) -> Result<FileDiff, RepoFailure> {
    let _span = tracing::debug_span!("diff.read", oid = %request.oid).entered();
    let repository =
        gix::open(&location.git_dir).map_err(|error| open_failure(&location.git_dir, &error))?;
    let commit = repository
        .find_commit(gix::ObjectId::from_bytes_or_panic(request.oid.as_bytes()))
        .map_err(|error| missing(request.oid, &error))?;
    let new_tree = commit
        .tree()
        .map_err(|error| missing(request.oid, &error))?;
    let old_tree = crate::files::parent_tree(&repository, &commit, request.parent)?;

    let old_bytes = request
        .paths
        .old_path()
        .and_then(|path| blob_at(old_tree.as_ref(), &path.0));
    let new_bytes = request
        .paths
        .new_path()
        .and_then(|path| blob_at(Some(&new_tree), &path.0));

    Ok(FileDiff {
        request: request.clone(),
        content: content_for(
            request.paths.display_path(),
            old_bytes,
            new_bytes,
            limits,
            ffmpeg_dir,
        ),
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
    paths: &DiffPaths,
    staged: bool,
    ffmpeg_dir: Option<&Path>,
) -> Result<DiffContent, RepoFailure> {
    let repository =
        gix::open(&location.git_dir).map_err(|error| open_failure(&location.git_dir, &error))?;
    let (old, new) = if staged {
        // An unborn HEAD has no tree, so everything staged reads as added.
        let head_tree = repository
            .head_commit()
            .ok()
            .and_then(|commit| commit.tree().ok());
        let old = paths
            .old_path()
            .and_then(|path| blob_at(head_tree.as_ref(), &path.0));
        let new = paths
            .new_path()
            .map(|path| index_blob(&repository, &path.0))
            .transpose()?
            .flatten();
        (old, new)
    } else {
        let old = paths
            .old_path()
            .map(|path| index_blob(&repository, &path.0))
            .transpose()?
            .flatten();
        let new = paths
            .new_path()
            .and_then(|path| worktree_bytes(location, &path.0));
        (old, new)
    };
    Ok(content_for(
        paths.display_path(),
        old,
        new,
        DiffLimits::default(),
        ffmpeg_dir,
    ))
}

/// The blob-pair tail every diff shares: image, video, binary, text, or absent.
fn content_for(
    path: &RepoPath,
    old: Option<Vec<u8>>,
    new: Option<Vec<u8>>,
    limits: DiffLimits,
    ffmpeg_dir: Option<&Path>,
) -> DiffContent {
    match (old, new) {
        (None, None) => DiffContent::Unavailable {
            message: format!(
                "{} is not present in this comparison.",
                path.display_lossy()
            ),
        },
        (old, new) => {
            // SVGs keep their text diff; Source/Preview renders those same
            // revisions as images on demand.
            if let Some(format) = image_format(&path.0).filter(|format| format != "svg") {
                DiffContent::Image {
                    before: old,
                    after: new,
                    format,
                }
            } else if let Some(format) = crate::media::video_format(&path.0) {
                // ponytail: probing here keeps the overlay on "Computing diff…"
                // until both sides are read, which is tens of milliseconds for
                // a local clip. Split it into a second async phase, the way the
                // Markdown preview loads, if that ever reads as a stall.
                let (before, before_info) = probe_side(old.as_deref(), &format, ffmpeg_dir);
                let (after, after_info) = probe_side(new.as_deref(), &format, ffmpeg_dir);
                DiffContent::Video {
                    before,
                    after,
                    before_info,
                    after_info,
                }
            } else if old.as_deref().is_some_and(is_binary) || new.as_deref().is_some_and(is_binary)
            {
                DiffContent::Binary {
                    message: format!("{} is binary.", path.display_lossy()),
                }
            } else {
                semantic_diff(old.as_deref(), new.as_deref(), limits)
            }
        }
    }
}

/// One side of a video comparison, absent on the side where the file is not.
fn probe_side(
    bytes: Option<&[u8]>,
    format: &str,
    ffmpeg_dir: Option<&Path>,
) -> (Option<Vec<u8>>, Option<VideoInfo>) {
    match bytes {
        Some(bytes) => {
            let (poster, info) = crate::media::probe(bytes, format, ffmpeg_dir);
            (poster, Some(info))
        }
        None => (None, None),
    }
}

/// The staged blob bytes at `path`, `None` when the index has no such entry.
///
/// # Errors
///
/// Returns a typed failure when the index cannot be read.
pub(crate) fn index_blob(
    repository: &gix::Repository,
    path: &[u8],
) -> Result<Option<Vec<u8>>, RepoFailure> {
    let index = repository
        .index_or_empty()
        .map_err(|error| open_failure(repository.git_dir(), &error))?;
    Ok(index
        .entry_by_path(path.into())
        .and_then(|entry| repository.find_object(entry.id).ok())
        .map(|object| object.data.clone()))
}

/// Reads a worktree file, `None` when it is gone or the repository is bare.
pub(crate) fn worktree_bytes(location: &RepoLocation, path: &[u8]) -> Option<Vec<u8>> {
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
pub(crate) fn image_format(path: &[u8]) -> Option<String> {
    let dot = path.iter().rposition(|&byte| byte == b'.')?;
    let extension = std::str::from_utf8(&path[dot + 1..]).ok()?.to_lowercase();
    match extension.as_str() {
        "png" | "gif" | "webp" | "bmp" | "svg" => Some(extension),
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
pub(crate) fn blob_at(tree: Option<&gix::Tree<'_>>, path: &[u8]) -> Option<Vec<u8>> {
    let components = path.split(|byte| *byte == b'/');
    let entry = tree?.lookup_entry(components).ok()??;
    let object = entry.object().ok()?;
    (object.kind == gix::object::Kind::Blob).then(|| object.data.clone())
}

/// Builds the semantic text diff shared by every reader layout.
///
/// Public so callers holding two blobs — the `--demo` fixture, tests — take
/// the same direct hunk path as a repository read.
pub fn unified(old: &[u8], new: &[u8], limits: DiffLimits) -> DiffContent {
    semantic_diff(Some(old), Some(new), limits)
}

fn semantic_diff(old: Option<&[u8]>, new: Option<&[u8]>, limits: DiffLimits) -> DiffContent {
    let _span = tracing::debug_span!("diff.model").entered();
    let total_bytes = old
        .map_or(0, <[u8]>::len)
        .saturating_add(new.map_or(0, <[u8]>::len));
    let total_lines = old
        .map_or(0, line_count)
        .saturating_add(new.map_or(0, line_count));
    if total_bytes > limits.bytes || total_lines > limits.lines {
        return DiffContent::TooLarge {
            bytes: total_bytes,
            lines: total_lines,
            byte_limit: limits.bytes,
            line_limit: limits.lines,
        };
    }
    DiffContent::Text(std::sync::Arc::new(text_diff(old, new)))
}

fn line_count(bytes: &[u8]) -> usize {
    bytes.split_inclusive(|byte| *byte == b'\n').count()
}

fn decoded_side(bytes: &[u8]) -> TextSide {
    match String::from_utf8(bytes.to_vec()) {
        Ok(text) => TextSide::new(text, TextDecoding::Utf8),
        Err(error) => TextSide::new(
            String::from_utf8_lossy(error.as_bytes()).into_owned(),
            TextDecoding::LossyUtf8,
        ),
    }
}

fn text_diff(old: Option<&[u8]>, new: Option<&[u8]>) -> TextDiff {
    let old_side = old.map(decoded_side);
    let new_side = new.map(decoded_side);
    let old_text = old_side.as_ref().map_or("", TextSide::text);
    let new_text = new_side.as_ref().map_or("", TextSide::text);
    let input = InternedInput::new(old_text, new_text);
    let mut computed = Diff::compute(Algorithm::Histogram, &input);
    computed.postprocess_lines(&input);

    let mut changes = Vec::new();
    let mut old_cursor = 0;
    let mut new_cursor = 0;
    for hunk in computed.hunks() {
        let old_start = hunk.before.start as usize;
        let new_start = hunk.after.start as usize;
        if old_cursor < old_start || new_cursor < new_start {
            changes.push(TextChange::Equal {
                old_lines: old_cursor..old_start,
                new_lines: new_cursor..new_start,
            });
        }
        let old_lines = old_start..hunk.before.end as usize;
        let new_lines = new_start..hunk.after.end as usize;
        let alignment =
            align_lines(&old_lines, &new_lines, old_side.as_ref(), new_side.as_ref()).into();
        changes.push(TextChange::Replace {
            old_lines: old_lines.clone(),
            new_lines: new_lines.clone(),
            alignment,
        });
        old_cursor = old_lines.end;
        new_cursor = new_lines.end;
    }
    let old_end = old_side.as_ref().map_or(0, TextSide::line_count);
    let new_end = new_side.as_ref().map_or(0, TextSide::line_count);
    if old_cursor < old_end || new_cursor < new_end {
        changes.push(TextChange::Equal {
            old_lines: old_cursor..old_end,
            new_lines: new_cursor..new_end,
        });
    }

    TextDiff {
        old: old_side,
        new: new_side,
        changes: changes.into(),
    }
}

fn align_lines(
    old: &std::ops::Range<usize>,
    new: &std::ops::Range<usize>,
    old_side: Option<&TextSide>,
    new_side: Option<&TextSide>,
) -> Vec<LinePair> {
    if old.is_empty() || new.is_empty() || old.len() == new.len() {
        return positional_alignment(old, new, 0);
    }
    let (short, long, short_side, long_side) = if old.len() < new.len() {
        (old, new, old_side, new_side)
    } else {
        (new, old, new_side, old_side)
    };
    let candidates = long.len() - short.len() + 1;
    if candidates.saturating_mul(short.len()) > 4_096 {
        return positional_alignment(old, new, 0);
    }
    let score = |offset| {
        (0..short.len())
            .map(|index| {
                let left = short_side.and_then(|side| side.line(short.start + index));
                let right = long_side.and_then(|side| side.line(long.start + offset + index));
                line_similarity(left.unwrap_or_default(), right.unwrap_or_default())
            })
            .sum::<usize>()
    };
    let baseline = score(0);
    let (best_offset, best_score) = (0..candidates)
        .map(|offset| (offset, score(offset)))
        .max_by_key(|(_, score)| *score)
        .unwrap_or((0, baseline));
    let required_gain = 500 * short.len();
    let offset = if best_score >= baseline + required_gain {
        best_offset
    } else {
        0
    };
    positional_alignment(old, new, offset)
}

fn positional_alignment(
    old: &std::ops::Range<usize>,
    new: &std::ops::Range<usize>,
    long_offset: usize,
) -> Vec<LinePair> {
    let old_longer = old.len() > new.len();
    let new_longer = new.len() > old.len();
    let count = old.len().max(new.len());
    (0..count)
        .map(|row| {
            let old_index = if old_longer {
                Some(old.start + row)
            } else if new_longer {
                row.checked_sub(long_offset)
                    .filter(|index| *index < old.len())
                    .map(|index| old.start + index)
            } else if row < old.len() {
                Some(old.start + row)
            } else {
                None
            };
            let new_index = if new_longer {
                Some(new.start + row)
            } else if old_longer {
                row.checked_sub(long_offset)
                    .filter(|index| *index < new.len())
                    .map(|index| new.start + index)
            } else if row < new.len() {
                Some(new.start + row)
            } else {
                None
            };
            LinePair {
                old_line: old_index,
                new_line: new_index,
            }
        })
        .collect()
}

fn line_similarity(left: &str, right: &str) -> usize {
    let left: Vec<_> = left
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let right: Vec<_> = right
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let longest = left.len().max(right.len());
    if longest == 0 {
        return 1_000;
    }
    let prefix = left
        .iter()
        .zip(&right)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = left[prefix..]
        .iter()
        .rev()
        .zip(right[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    (prefix + suffix) * 1_000 / longest
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{
        DiffContent, DiffParent, DiffPaths, DiffSide, FileDiffRequest, Oid, RepoPath, TextChange,
        TextDiff, TextSide,
    };
    use sourcefour_test_support::TempRepo;

    use super::{DiffLimits, align_lines, file_diff, file_diff_with_limits, worktree_file_diff};
    use crate::discover;

    fn head(repository: &TempRepo) -> Result<Oid, Box<dyn std::error::Error>> {
        Ok(Oid::from_hex(&repository.git(&["rev-parse", "HEAD"]))?)
    }

    fn request(oid: Oid, path: &str) -> FileDiffRequest {
        FileDiffRequest {
            oid,
            parent: DiffParent::FirstParent,
            paths: DiffPaths::same(RepoPath(path.as_bytes().to_vec())),
        }
    }

    #[test]
    fn similarity_alignment_places_a_surplus_line_before_matching_rewrites() {
        let old = TextSide::new(
            "let alpha = 1;\nlet omega = 1;\n".into(),
            sourcefour_model::TextDecoding::Utf8,
        );
        let new = TextSide::new(
            "// note\nlet alpha = 2;\nlet omega = 2;\n".into(),
            sourcefour_model::TextDecoding::Utf8,
        );

        let alignment = align_lines(&(0..2), &(0..3), Some(&old), Some(&new));

        assert_eq!(alignment[0].old_line, None);
        assert_eq!(alignment[0].new_line, Some(0));
        assert_eq!(alignment[1].old_line, Some(0));
        assert_eq!(alignment[1].new_line, Some(1));
    }

    fn changed_lines(diff: &TextDiff, side: DiffSide) -> Vec<(usize, &str)> {
        let text = match side {
            DiffSide::Old => diff.old.as_ref(),
            DiffSide::New => diff.new.as_ref(),
        };
        diff.changes
            .iter()
            .filter_map(|change| match (change, side) {
                (TextChange::Replace { old_lines, .. }, DiffSide::Old) => Some(old_lines.clone()),
                (TextChange::Replace { new_lines, .. }, DiffSide::New) => Some(new_lines.clone()),
                (TextChange::Equal { .. }, _) => None,
            })
            .flatten()
            .filter_map(|line| {
                text.and_then(|text| text.line(line))
                    .map(|value| (line, value))
            })
            .collect()
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
            None,
        )?;

        let DiffContent::Text(diff) = diff.content else {
            panic!("a small text change renders as text");
        };
        assert_eq!(changed_lines(&diff, DiffSide::Old), vec![(1, "two")]);
        assert_eq!(changed_lines(&diff, DiffSide::New), vec![(1, "TWO")]);
        Ok(())
    }

    #[test]
    fn a_rename_reads_each_side_from_its_own_path() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        std::fs::write(repository.path().join("old.txt"), "before\n")?;
        repository.git(&["add", "."]);
        repository.commit("add");
        repository.git(&["mv", "old.txt", "new.txt"]);
        std::fs::write(repository.path().join("new.txt"), "after\n")?;
        repository.git(&["add", "."]);
        repository.commit("rename and edit");
        let request = FileDiffRequest {
            oid: head(&repository)?,
            parent: DiffParent::FirstParent,
            paths: DiffPaths::changed(RepoPath(b"old.txt".to_vec()), RepoPath(b"new.txt".to_vec())),
        };

        let diff = file_diff(&discover(repository.path())?, &request, None)?;
        let DiffContent::Text(diff) = diff.content else {
            panic!("a renamed text file renders as text");
        };
        assert_eq!(changed_lines(&diff, DiffSide::Old), vec![(0, "before")]);
        assert_eq!(changed_lines(&diff, DiffSide::New), vec![(0, "after")]);
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
            None,
        )?;

        let DiffContent::Text(diff) = diff.content else {
            panic!("an added text file renders as text");
        };
        assert_eq!(changed_lines(&diff, DiffSide::New).len(), 2);
        assert!(diff.old.is_none());
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
            None,
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
            None,
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
    fn a_modified_video_reports_both_sides() -> Result<(), Box<dyn std::error::Error>> {
        // The bytes are not a video, which is the point: the routing must not
        // depend on a decoder being installed, and the sizes have to survive
        // even when nothing else about the blob can be read.
        let repository = TempRepo::init();
        std::fs::write(repository.path().join("clip.mp4"), b"not-a-clip")?;
        repository.git(&["add", "."]);
        repository.commit("add clip");
        std::fs::write(repository.path().join("clip.mp4"), b"still-not-a-clip")?;
        repository.git(&["add", "."]);
        repository.commit("update clip");

        let diff = file_diff(
            &discover(repository.path())?,
            &request(head(&repository)?, "clip.mp4"),
            None,
        )?;

        let DiffContent::Video {
            before_info,
            after_info,
            ..
        } = diff.content
        else {
            panic!("a .mp4 comparison is a video, not a binary notice");
        };
        assert_eq!(before_info.map(|info| info.bytes), Some(10));
        assert_eq!(after_info.map(|info| info.bytes), Some(16));
        Ok(())
    }

    #[test]
    fn a_committed_clip_comes_back_with_a_poster() -> Result<(), Box<dyn std::error::Error>> {
        // The join the two halves leave untested: a blob that only exists in
        // the object database, spilled to disk and decoded. Skipped where
        // there is no decoder, which is also what the viewer does there.
        let Some(ffmpeg) = crate::media::ffmpeg_path(None) else {
            return Ok(());
        };
        let repository = TempRepo::init();
        let path = repository.path().join("clip.mp4");
        let made = std::process::Command::new(ffmpeg)
            .args(["-v", "error", "-nostdin", "-y"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=160x120:rate=10:duration=1",
            ])
            .args(["-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()?;
        assert!(made.success(), "ffmpeg could not generate the fixture");
        repository.git(&["add", "."]);
        repository.commit("add clip");

        let diff = file_diff(
            &discover(repository.path())?,
            &request(head(&repository)?, "clip.mp4"),
            None,
        )?;

        let DiffContent::Video {
            after, after_info, ..
        } = diff.content
        else {
            panic!("a .mp4 comparison is a video, not a binary notice");
        };
        let poster = after.expect("a poster frame decoded from the committed blob");
        assert_eq!(&poster[..4], b"\x89PNG", "the poster is not a PNG");
        assert_eq!(
            after_info.and_then(|info| info.dimensions),
            Some((160, 120))
        );
        Ok(())
    }

    #[test]
    fn a_deleted_video_has_no_after_side() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        std::fs::write(repository.path().join("clip.mp4"), b"not-a-clip")?;
        repository.git(&["add", "."]);
        repository.commit("add clip");
        std::fs::remove_file(repository.path().join("clip.mp4"))?;
        repository.git(&["add", "-A"]);
        repository.commit("remove clip");

        let diff = file_diff(
            &discover(repository.path())?,
            &request(head(&repository)?, "clip.mp4"),
            None,
        )?;

        let DiffContent::Video {
            before_info,
            after,
            after_info,
            ..
        } = diff.content
        else {
            panic!("a .mp4 comparison is a video, not a binary notice");
        };
        assert!(before_info.is_some());
        assert_eq!(after_info, None);
        assert_eq!(after, None);
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
            None,
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
            None,
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
            None,
        )?;

        let DiffContent::Text(diff) = diff.content else {
            panic!("no diff is too large to render by default");
        };
        assert_eq!(
            diff.new.as_ref().map_or(0, TextSide::line_count),
            25_000,
            "every source line is present"
        );
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
        let paths = DiffPaths::same(path);

        let DiffContent::Text(diff) = worktree_file_diff(&location, &paths, true, None)? else {
            panic!("the staged comparison renders as text");
        };
        assert!(
            changed_lines(&diff, DiffSide::New)
                .iter()
                .any(|(_, line)| *line == "TWO"),
            "staged compares HEAD to the index"
        );
        assert!(
            !diff
                .new
                .as_ref()
                .is_some_and(|side| side.text().contains("three")),
            "the unstaged edit is invisible to the staged side"
        );

        let DiffContent::Text(diff) = worktree_file_diff(&location, &paths, false, None)? else {
            panic!("the unstaged comparison renders as text");
        };
        assert!(
            changed_lines(&diff, DiffSide::New)
                .iter()
                .any(|(_, line)| *line == "three"),
            "unstaged compares the index to the filesystem"
        );
        assert!(
            !changed_lines(&diff, DiffSide::Old)
                .iter()
                .any(|(_, line)| *line == "two"),
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
        let paths = DiffPaths::added(path);

        let DiffContent::Text(diff) = worktree_file_diff(&location, &paths, false, None)? else {
            panic!("an untracked text file renders as text");
        };
        assert_eq!(changed_lines(&diff, DiffSide::New).len(), 2);
        assert!(
            matches!(
                worktree_file_diff(&location, &paths, true, None)?,
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
            None,
        )?;

        assert!(matches!(diff.content, DiffContent::Unavailable { .. }));
        Ok(())
    }
}
