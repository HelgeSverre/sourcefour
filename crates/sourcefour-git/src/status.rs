//! The working tree's uncommitted state, read through the user's own Git.
//!
//! `git status --porcelain=v2 -z` is the one status source whose semantics —
//! ignores, rename detection, submodules — always match the user's terminal,
//! and the format is documented as stable. This is a quick read, not a
//! progress operation, so it runs a plain `Command` rather than the §10
//! runner. `--no-optional-locks` keeps a background read from contending
//! with the user's own Git for the index lock.

use std::process::Command;

use sourcefour_model::{
    ChangeKind, ChangedFile, RepoFailure, RepoFailureKind, RepoLocation, RepoPath, UserFacingError,
    WorkingTreeStatus,
};

/// Reads the current staged/unstaged state of the active worktree.
///
/// A bare repository has no working tree and reports clean.
///
/// # Errors
///
/// Returns a typed failure when Git cannot be spawned or exits nonzero.
pub fn working_tree_status(location: &RepoLocation) -> Result<WorkingTreeStatus, RepoFailure> {
    let Some(worktree) = &location.active_worktree_path else {
        return Ok(WorkingTreeStatus::default());
    };
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args([
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
        ])
        .output()
        .map_err(|error| failure(&error.to_string()))?;
    if !output.status.success() {
        return Err(failure(String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(parse_porcelain_v2(&output.stdout))
}

fn failure(detail: &str) -> RepoFailure {
    RepoFailure {
        kind: RepoFailureKind::Internal,
        user: UserFacingError {
            title: String::from("Working tree status could not be read"),
            message: detail.to_owned(),
            details: None,
            retryable: true,
        },
        operation: None,
        session: None,
        generation: None,
    }
}

/// Parses `git status --porcelain=v2 -z` output.
///
/// Records are NUL-terminated; a rename record (`2`) is followed by one
/// extra NUL-terminated token holding the original path. Paths stay raw
/// bytes end to end ([`RepoPath`]), so nothing here assumes UTF-8.
fn parse_porcelain_v2(bytes: &[u8]) -> WorkingTreeStatus {
    let mut status = WorkingTreeStatus::default();
    let mut tokens = bytes.split(|&byte| byte == 0).filter(|t| !t.is_empty());
    while let Some(token) = tokens.next() {
        match token.first() {
            Some(b'1') => {
                let Some((fields, path)) = split_fields(token, 8) else {
                    continue;
                };
                push_sides(&mut status, fields[1], path, None);
            }
            Some(b'2') => {
                let Some((fields, path)) = split_fields(token, 9) else {
                    continue;
                };
                let original = tokens.next();
                push_sides(&mut status, fields[1], path, original);
            }
            Some(b'u') => {
                if let Some((_, path)) = split_fields(token, 10) {
                    status
                        .unstaged
                        .push(file(Some(path), Some(path), ChangeKind::Unknown));
                }
            }
            Some(b'?') => {
                if let Some((_, path)) = split_fields(token, 1) {
                    status
                        .unstaged
                        .push(file(None, Some(path), ChangeKind::Added));
                }
            }
            _ => {}
        }
    }
    status
}

/// One entry's `XY` pair fans out to the staged and unstaged lists.
fn push_sides(status: &mut WorkingTreeStatus, xy: &[u8], path: &[u8], original: Option<&[u8]>) {
    let (x, y) = (xy.first().copied(), xy.get(1).copied());
    if let Some(x) = x.filter(|&x| x != b'.') {
        let kind = kind_of(x);
        let (old, new) = match kind {
            ChangeKind::Added => (None, Some(path)),
            ChangeKind::Deleted => (Some(path), None),
            ChangeKind::Renamed | ChangeKind::Copied => (original.or(Some(path)), Some(path)),
            _ => (Some(path), Some(path)),
        };
        status.staged.push(file(old, new, kind));
    }
    if let Some(y) = y.filter(|&y| y != b'.') {
        let kind = kind_of(y);
        let (old, new) = match kind {
            ChangeKind::Deleted => (Some(path), None),
            _ => (Some(path), Some(path)),
        };
        status.unstaged.push(file(old, new, kind));
    }
}

fn kind_of(byte: u8) -> ChangeKind {
    match byte {
        b'A' => ChangeKind::Added,
        b'M' | b'T' => ChangeKind::Modified,
        b'D' => ChangeKind::Deleted,
        b'R' => ChangeKind::Renamed,
        b'C' => ChangeKind::Copied,
        _ => ChangeKind::Unknown,
    }
}

fn file(old: Option<&[u8]>, new: Option<&[u8]>, status: ChangeKind) -> ChangedFile {
    ChangedFile {
        old_path: old.map(|path| RepoPath(path.to_vec())),
        new_path: new.map(|path| RepoPath(path.to_vec())),
        status,
        additions: None,
        deletions: None,
        is_binary: false,
    }
}

/// Splits `count` space-separated header fields off a record; the remainder
/// is the path.
fn split_fields(record: &[u8], count: usize) -> Option<(Vec<&[u8]>, &[u8])> {
    let mut fields = Vec::with_capacity(count);
    let mut rest = record;
    for _ in 0..count {
        let space = rest.iter().position(|&byte| byte == b' ')?;
        fields.push(&rest[..space]);
        rest = &rest[space + 1..];
    }
    Some((fields, rest))
}

#[cfg(test)]
mod tests {
    use sourcefour_model::ChangeKind;

    use super::parse_porcelain_v2;

    fn line(text: &str) -> Vec<u8> {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        bytes
    }

    #[test]
    fn a_clean_tree_parses_to_clean() {
        let status = parse_porcelain_v2(b"");
        assert!(status.is_clean());
        assert_eq!(status.summary().staged, 0);
    }

    #[test]
    fn ordinary_entries_fan_out_to_both_sides() {
        // One file modified in the index AND further modified in the tree.
        let mut bytes = line("1 MM N... 100644 100644 100644 aaaa bbbb src/lib.rs");
        // One file staged as new, untouched since.
        bytes.extend(line("1 A. N... 000000 100644 100644 0000 cccc docs/new.md"));
        // One file deleted in the tree, not staged.
        bytes.extend(line("1 .D N... 100644 100644 000000 dddd dddd gone.txt"));

        let status = parse_porcelain_v2(&bytes);

        assert_eq!(status.summary().staged, 2);
        assert_eq!(status.summary().unstaged, 2);
        assert_eq!(status.staged[0].status, ChangeKind::Modified);
        assert_eq!(status.staged[1].status, ChangeKind::Added);
        assert_eq!(
            status.staged[1].old_path, None,
            "an addition has no old side"
        );
        assert_eq!(status.unstaged[0].status, ChangeKind::Modified);
        assert_eq!(status.unstaged[1].status, ChangeKind::Deleted);
        assert_eq!(
            status.unstaged[1].new_path, None,
            "a deletion has no new side"
        );
    }

    #[test]
    fn a_staged_rename_carries_both_paths() {
        // -z rename records: header+new path, then the original as its own
        // NUL-terminated token.
        let mut bytes = b"2 R. N... 100644 100644 100644 aaaa aaaa R100 new/name.rs".to_vec();
        bytes.push(0);
        bytes.extend(b"old/name.rs");
        bytes.push(0);

        let status = parse_porcelain_v2(&bytes);

        assert_eq!(status.summary().staged, 1);
        assert_eq!(status.staged[0].status, ChangeKind::Renamed);
        assert_eq!(
            status.staged[0].old_path.as_ref().map(|p| p.0.as_slice()),
            Some(b"old/name.rs".as_slice())
        );
        assert_eq!(
            status.staged[0].new_path.as_ref().map(|p| p.0.as_slice()),
            Some(b"new/name.rs".as_slice())
        );
        assert!(status.unstaged.is_empty());
    }

    #[test]
    fn untracked_and_conflicted_paths_land_unstaged() {
        let mut bytes = line("? scratch/notes.txt");
        bytes.extend(line(
            "u UU N... 100644 100644 100644 100644 aaaa bbbb cccc src/clash.rs",
        ));

        let status = parse_porcelain_v2(&bytes);

        assert!(status.staged.is_empty());
        assert_eq!(status.unstaged[0].status, ChangeKind::Added);
        assert_eq!(status.unstaged[0].old_path, None);
        assert_eq!(
            status.unstaged[1].status,
            ChangeKind::Unknown,
            "conflicts have no stage story this milestone; they only warn"
        );
    }

    #[test]
    fn paths_with_spaces_survive_field_splitting() {
        let status = parse_porcelain_v2(&line(
            "1 .M N... 100644 100644 100644 aaaa aaaa dir with spaces/a file.txt",
        ));
        assert_eq!(
            status.unstaged[0].new_path.as_ref().map(|p| p.0.as_slice()),
            Some(b"dir with spaces/a file.txt".as_slice())
        );
    }
}
