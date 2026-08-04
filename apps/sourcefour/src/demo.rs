//! Deterministic demo fixture rendered through the real window (§12.4).
//!
//! `--demo` seeds the same state a live repository would produce, so the
//! screenshot fixture regression-tests the real rendering path instead of a
//! parallel hardcoded UI.

use std::{collections::BTreeMap, path::PathBuf, time::Instant};

use smallvec::SmallVec;
use sourcefour_graph::GraphState;
use sourcefour_model::{
    AheadBehindState, BranchSnapshot, ChangeKind, ChangedFile, CommitFiles, CommitFlags, CommitRow,
    DiffParent, GitTime, GraphRow, HeadSnapshot, Oid, RefKind, RefLabel, RemoteBranchSnapshot,
    RemoteSnapshot, RepoKind, RepoLocation, RepoPath, RepoSnapshot, UpstreamSnapshot,
    WorktreeAccessibility, WorktreeId, WorktreeSnapshot,
};

/// Repository identity shown by `--demo`, matching the screenshot fixture.
pub(crate) const REPOSITORY_NAME: &str = "sourcefour";
pub(crate) const REPOSITORY_PATH: &str = "~/code/sourcefour";

/// Fixed "current time" so relative dates render identically in every capture.
pub(crate) const NOW_SECONDS: i64 = 1_722_600_000;

const AUTHOR: &str = "Helge Sverre";

/// Subject, hash prefix, hours before [`NOW_SECONDS`], and parent row indices.
///
/// The topology deliberately exercises every painter case: two-lane stretches,
/// lane convergence (rows 5, 10, 15), merges (rows 0, 10), a branch tip
/// appearing mid-list (row 7), and a root (row 17). Lanes and segments come
/// from the real §7.3 algorithm, never from this table.
const COMMITS: [(&str, &str, i64, &[usize]); 18] = [
    (
        "Merge branch 'feature/worktrees' into main",
        "9f3e21a",
        2,
        &[1, 2],
    ),
    ("chore(deps): pin gpui to rev d637307", "7c1d9b4", 3, &[3]),
    (
        "List linked worktrees from common git directory",
        "4b8e0f2",
        5,
        &[4],
    ),
    ("Render metadata sidebar loading state", "e57a109", 26, &[5]),
    (
        "Preserve selected commit on a safe refresh",
        "6c9de83",
        28,
        &[5],
    ),
    ("Add deterministic history fixture", "0f2a49d", 30, &[6]),
    (
        "Improve repository discovery failure messages",
        "e6b1a95",
        50,
        &[8],
    ),
    ("Document the hybrid Git backend", "2a7ccf4", 52, &[9]),
    (
        "Use all unique references as default scope",
        "5cb3418",
        74,
        &[10],
    ),
    ("Add graph colour continuity goldens", "3d88c70", 76, &[10]),
    ("Merge branch 'feature/history'", "7f0e4f1", 98, &[11, 12]),
    (
        "Read commits through a persistent cursor",
        "66ea252",
        100,
        &[13],
    ),
    ("Implement typed repository sessions", "0ceef2b", 122, &[14]),
    ("Add sourcefour virtual workspace", "c4f4b7a", 124, &[15]),
    (
        "Promote implementation specification",
        "4f8d31b",
        146,
        &[15],
    ),
    ("Archive restart prototype", "1f202f1", 148, &[16]),
    ("Initial Sourcefour architecture", "128b3a6", 170, &[17]),
    ("Repository bootstrap", "ac7d153", 172, &[]),
];

/// The metadata sidebar's state, shaped like the supplied mockup.
pub(crate) fn snapshot() -> RepoSnapshot {
    RepoSnapshot {
        location: RepoLocation {
            invocation_path: PathBuf::from(REPOSITORY_PATH),
            active_worktree_path: Some(PathBuf::from(REPOSITORY_PATH)),
            git_dir: PathBuf::from(REPOSITORY_PATH).join(".git"),
            common_dir: PathBuf::from(REPOSITORY_PATH).join(".git"),
            is_bare: false,
            kind: RepoKind::Worktree,
        },
        display_name: String::from(REPOSITORY_NAME),
        active_worktree: Some(WorktreeId(String::from("sourcefour"))),
        worktrees: vec![
            worktree("sourcefour", REPOSITORY_PATH, "main", true),
            worktree(
                "sourcefour-history",
                "~/code/sourcefour-history",
                "feature/history",
                false,
            ),
            worktree("scratch", "~/code/scratch", "prototype", false),
        ],
        local_branches: vec![
            branch(
                "main",
                "9f3e21a",
                true,
                AheadBehindState::Known {
                    ahead: 2,
                    behind: 0,
                },
                Some("sourcefour"),
            ),
            branch(
                "feature/worktrees",
                "4b8e0f2",
                false,
                AheadBehindState::Unavailable,
                None,
            ),
            branch(
                "feature/history",
                "66ea252",
                false,
                AheadBehindState::Unavailable,
                Some("sourcefour-history"),
            ),
            branch(
                "release/v1",
                "5cb3418",
                false,
                AheadBehindState::Unavailable,
                None,
            ),
            branch(
                "prototype",
                "3d88c70",
                false,
                AheadBehindState::Unavailable,
                Some("scratch"),
            ),
        ],
        remotes: vec![RemoteSnapshot {
            name: String::from("origin"),
            fetch_url: Some(String::from("git@github.com:helge/sourcefour")),
            default_branch: Some(String::from("main")),
            branches: vec![
                remote_branch("main", "7c1d9b4"),
                remote_branch("feature/worktrees", "4b8e0f2"),
            ],
        }],
        tags_count: 3,
        labels_by_object: BTreeMap::new(),
        head: HeadSnapshot::Branch {
            full_name: String::from("refs/heads/main"),
            short_name: String::from("main"),
            oid: oid("9f3e21a"),
        },
        refreshed_at: Instant::now(),
    }
}

/// The loaded history, exactly as a completed traversal would deliver it.
///
/// Layout runs through the real §7.3 lane algorithm, so the fixture renders
/// whatever the shipping code produces for this topology — including the
/// curves the §7.4 painter draws between rows.
pub(crate) fn history() -> (Vec<CommitRow>, Vec<GraphRow>) {
    let rows: Vec<CommitRow> = COMMITS
        .iter()
        .enumerate()
        .map(|(index, &(subject, hash, hours, parent_rows))| {
            let parents: SmallVec<[Oid; 2]> = parent_rows
                .iter()
                .map(|&parent| oid(COMMITS[parent].1))
                .collect();
            let labels = if index == 0 {
                SmallVec::from_iter([RefLabel {
                    name: String::from("main"),
                    kind: RefKind::LocalBranch,
                    is_head: true,
                    is_current: true,
                }])
            } else {
                SmallVec::new()
            };
            CommitRow {
                oid: oid(hash),
                summary: subject.to_owned(),
                author_name: AUTHOR.to_owned(),
                commit_time: GitTime {
                    seconds_since_epoch: NOW_SECONDS - hours * 3600,
                    offset_minutes: 0,
                },
                labels,
                flags: CommitFlags {
                    is_merge: parents.len() > 1,
                    is_shallow_boundary: false,
                },
                parents,
            }
        })
        .collect();
    let mut rows = rows;
    // Ref chips matching the snapshot's branch tips, so §12.4 captures cover
    // every label kind: local, remote, and tag, plus the >3 overflow on row 2.
    let chip = |name: &str, kind: RefKind| RefLabel {
        name: name.to_owned(),
        kind,
        is_head: false,
        is_current: false,
    };
    rows[1].labels.push(chip("origin/main", RefKind::RemoteBranch));
    rows[2].labels.push(chip("feature/worktrees", RefKind::LocalBranch));
    rows[2]
        .labels
        .push(chip("origin/feature/worktrees", RefKind::RemoteBranch));
    rows[8].labels.push(chip("release/v1", RefKind::LocalBranch));
    rows[9].labels.push(chip("prototype", RefKind::LocalBranch));
    rows[10].labels.push(chip("v0.1.0", RefKind::Tag));
    rows[11].labels.push(chip("feature/history", RefKind::LocalBranch));

    let mut state = GraphState::default();
    let layout = rows
        .iter()
        .map(|row| state.push(row.oid, &row.parents))
        .collect();
    (rows, layout)
}

/// Changed files for the fixture's initially selected commit (§12.4).
pub(crate) fn files() -> CommitFiles {
    let changed = |old: Option<&str>, new: Option<&str>, status: ChangeKind| ChangedFile {
        old_path: old.map(|path| RepoPath(path.as_bytes().to_vec())),
        new_path: new.map(|path| RepoPath(path.as_bytes().to_vec())),
        status,
        additions: None,
        deletions: None,
        is_binary: false,
    };
    CommitFiles {
        oid: oid("9f3e21a"),
        parent: DiffParent::FirstParent,
        files: vec![
            changed(
                Some("apps/sourcefour/src/views.rs"),
                Some("apps/sourcefour/src/views.rs"),
                ChangeKind::Modified,
            ),
            changed(
                None,
                Some("crates/sourcefour-git/src/worktrees.rs"),
                ChangeKind::Added,
            ),
            changed(Some("docs/restart-notes.md"), None, ChangeKind::Deleted),
        ],
    }
}

/// Expands a mockup hash prefix into a full deterministic object ID.
fn oid(prefix: &str) -> Oid {
    Oid::from_hex(&format!("{prefix:0<40}")).expect("demo hashes are hexadecimal")
}

fn worktree(name: &str, path: &str, head_branch: &str, current: bool) -> WorktreeSnapshot {
    WorktreeSnapshot {
        id: WorktreeId(name.to_owned()),
        display_name: name.to_owned(),
        path: PathBuf::from(path),
        git_dir: PathBuf::from(path).join(".git"),
        head: HeadSnapshot::Branch {
            full_name: format!("refs/heads/{head_branch}"),
            short_name: head_branch.to_owned(),
            oid: oid("9f3e21a"),
        },
        is_current: current,
        is_main: current,
        is_locked: false,
        lock_reason: None,
        accessibility: WorktreeAccessibility::Accessible,
    }
}

fn branch(
    short_name: &str,
    tip: &str,
    current: bool,
    ahead_behind: AheadBehindState,
    checked_out_in: Option<&str>,
) -> BranchSnapshot {
    BranchSnapshot {
        full_name: format!("refs/heads/{short_name}"),
        short_name: short_name.to_owned(),
        tip: oid(tip),
        is_current: current,
        upstream: current.then(|| UpstreamSnapshot {
            full_name: String::from("refs/remotes/origin/main"),
            short_name: String::from("origin/main"),
            tip: oid("7c1d9b4"),
        }),
        ahead_behind,
        checked_out_in: checked_out_in.map(|name| WorktreeId(name.to_owned())),
    }
}

fn remote_branch(short_name: &str, tip: &str) -> RemoteBranchSnapshot {
    RemoteBranchSnapshot {
        full_name: format!("refs/remotes/origin/{short_name}"),
        short_name: short_name.to_owned(),
        tip: oid(tip),
    }
}

#[cfg(test)]
mod tests {
    use crate::history::relative_date;

    use super::{NOW_SECONDS, history, snapshot};

    #[test]
    fn the_history_fixture_is_deterministic_and_aligned() {
        let (rows, layout) = history();

        assert_eq!(
            rows.len(),
            layout.len(),
            "the renderer indexes them together"
        );
        assert_eq!(rows[0].oid.abbreviated(7), "9f3e21a");
        assert!(rows[0].flags.is_merge);
        assert_eq!(rows[0].parents.len(), 2, "a merge row carries both parents");

        let (again, _) = history();
        assert_eq!(rows, again, "two builds must be byte-identical");
    }

    #[test]
    fn relative_dates_are_anchored_so_captures_never_drift() {
        let (rows, _) = history();

        assert_eq!(
            relative_date(NOW_SECONDS, rows[0].commit_time),
            "2 hours ago"
        );
        assert_eq!(
            relative_date(NOW_SECONDS, rows[17].commit_time),
            "7 days ago"
        );
    }

    #[test]
    fn the_snapshot_matches_the_mockup_shape() {
        let snapshot = snapshot();

        assert_eq!(snapshot.worktrees.len(), 3);
        assert_eq!(snapshot.local_branches.len(), 5);
        assert_eq!(snapshot.remotes.len(), 1);
        assert!(snapshot.worktrees[0].is_current);
        assert!(snapshot.local_branches[0].is_current);
        assert_eq!(
            snapshot.local_branches[0].ahead_behind,
            sourcefour_model::AheadBehindState::Known {
                ahead: 2,
                behind: 0
            },
            "the mockup shows main at +2"
        );
    }

    #[test]
    fn the_fixture_exceeds_the_baseline_viewport() {
        const BASELINE_VISIBLE_ROWS: usize = 13;
        assert!(history().0.len() > BASELINE_VISIBLE_ROWS);
    }

    #[test]
    fn the_fixture_topology_exercises_every_painter_case() {
        use sourcefour_model::GraphSegment;

        let (rows, layout) = history();

        assert!(layout[0].flags.is_merge, "row 0 is a merge");
        assert!(
            layout[10]
                .segments
                .iter()
                .any(|segment| matches!(segment, GraphSegment::Fork { .. }))
                && layout[10]
                    .segments
                    .iter()
                    .any(|segment| matches!(segment, GraphSegment::Merge { .. })),
            "row 10 both converges one branch and starts another"
        );
        assert!(
            !layout[7].flags.continues_above,
            "row 7 is a branch tip appearing mid-list"
        );
        assert!(layout[17].flags.is_root, "the last row is the root");
        assert!(
            layout.iter().any(|row| row.node_lane > 0),
            "the fixture must actually leave lane 0"
        );
        assert!(
            rows[0].labels.iter().any(|label| label.is_head),
            "the newest commit carries HEAD so the halo renders"
        );
    }
}
