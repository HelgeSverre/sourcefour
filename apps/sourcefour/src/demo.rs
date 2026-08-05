//! Deterministic demo fixture rendered through the real window (§12.4).
//!
//! `--demo` seeds the same state a live repository would produce, so the
//! screenshot fixture regression-tests the real rendering path instead of a
//! parallel hardcoded UI.

use std::{collections::BTreeMap, path::PathBuf, time::Instant};

use smallvec::SmallVec;
use sourcefour_graph::GraphState;
use sourcefour_model::{
    AheadBehindState, BranchSnapshot, ChangeKind, ChangedFile, CommitDetail, CommitFiles,
    CommitFlags, CommitRow, DiffContent, DiffParent, GitTime, GraphRow, HeadSnapshot, Oid, RefKind,
    RefLabel, RemoteBranchSnapshot, RemoteSnapshot, RepoKind, RepoLocation, RepoPath, RepoSnapshot,
    Signature, UpstreamSnapshot, WorktreeAccessibility, WorktreeId, WorktreeSnapshot,
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
/// The §12.4 working-tree fixture: a believable mid-feature state.
pub(crate) fn working_tree_status() -> sourcefour_model::WorkingTreeStatus {
    use sourcefour_model::{ChangeKind, ChangedFile, RepoPath};
    let file = |old: Option<&str>, new: Option<&str>, status| ChangedFile {
        old_path: old.map(|path| RepoPath(path.as_bytes().to_vec())),
        new_path: new.map(|path| RepoPath(path.as_bytes().to_vec())),
        status,
        additions: None,
        deletions: None,
        is_binary: false,
    };
    sourcefour_model::WorkingTreeStatus {
        staged: vec![
            file(
                Some("apps/sourcefour/src/views.rs"),
                Some("apps/sourcefour/src/views.rs"),
                ChangeKind::Modified,
            ),
            file(
                None,
                Some("crates/sourcefour-git/src/status.rs"),
                ChangeKind::Added,
            ),
        ],
        unstaged: vec![
            file(
                Some("apps/sourcefour/src/theme.rs"),
                Some("apps/sourcefour/src/theme.rs"),
                ChangeKind::Modified,
            ),
            file(None, Some("docs/commit-notes.md"), ChangeKind::Added),
        ],
    }
}

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
    rows[1]
        .labels
        .push(chip("origin/main", RefKind::RemoteBranch));
    rows[2]
        .labels
        .push(chip("feature/worktrees", RefKind::LocalBranch));
    rows[2]
        .labels
        .push(chip("origin/feature/worktrees", RefKind::RemoteBranch));
    rows[8]
        .labels
        .push(chip("release/v1", RefKind::LocalBranch));
    rows[9].labels.push(chip("prototype", RefKind::LocalBranch));
    rows[10].labels.push(chip("v0.1.0", RefKind::Tag));
    rows[11]
        .labels
        .push(chip("feature/history", RefKind::LocalBranch));

    let mut state = GraphState::default();
    let layout = rows
        .iter()
        .map(|row| state.push(row.oid, &row.parents))
        .collect();
    (rows, layout)
}

/// Full metadata for the fixture's initially selected commit (§12.4).
pub(crate) fn detail() -> CommitDetail {
    let signature = Signature {
        name: AUTHOR.to_owned(),
        email: String::from("helge@sourcefour.dev"),
        time: GitTime {
            seconds_since_epoch: NOW_SECONDS - 2 * 3600,
            offset_minutes: 0,
        },
    };
    CommitDetail {
        oid: oid("9f3e21a"),
        subject: String::from("Merge branch 'feature/worktrees' into main"),
        body: String::from(
            "Brings linked-worktree enumeration into the git layer and the\nworktree section of the sidebar.",
        ),
        author: signature.clone(),
        committer: signature,
        parents: SmallVec::from_iter([oid("7c1d9b4"), oid("4b8e0f2")]),
    }
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

/// A capture scene: the extra state `--demo` seeds before the first frame.
///
/// Views that normally need a click — the diff overlay and its layouts — are
/// unreachable from a screenshot run, so each scene names one and seeds it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Scene {
    /// The window as it opens: sidebar, graph, history, details.
    #[default]
    Overview,
    /// The diff overlay over a changed file, unified.
    Diff,
    /// The same diff, laid out side by side.
    Split,
    /// The image diff overlay, juxtapose slider centred.
    Image,
    /// The settings overlay on its first section.
    Settings,
    /// The Actions run detail overlay on a failed run.
    Actions,
    /// The working-tree row selected, staged and unstaged files listed.
    Commit,
    /// The diff overlay on a Markdown file, rendered rather than diffed.
    Preview,
}

impl Scene {
    /// Every scene name `--scene` accepts, for the usage line.
    pub(crate) const NAMES: &'static str =
        "overview, diff, split, image, settings, actions, commit, preview";

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "overview" => Some(Self::Overview),
            "diff" => Some(Self::Diff),
            "split" => Some(Self::Split),
            "image" => Some(Self::Image),
            "settings" => Some(Self::Settings),
            "actions" => Some(Self::Actions),
            "commit" => Some(Self::Commit),
            "preview" => Some(Self::Preview),
            _ => None,
        }
    }

    /// The changed file the scene's overlay is opened over. The text scenes
    /// name the modified entry of [`files`], so the overlay and the list
    /// behind it describe the same change; the preview scene names a document
    /// instead, since the toggle only appears for one.
    pub(crate) fn file(self) -> &'static str {
        match self {
            Self::Image => "assets/icon.png",
            Self::Preview => "README.md",
            _ => "apps/sourcefour/src/views.rs",
        }
    }

    /// The overlay's content, formatted by the shipping diff code so a capture
    /// can never show lines the product would not produce.
    pub(crate) fn content(self) -> DiffContent {
        let (before, after) = match self {
            Self::Image => {
                return DiffContent::Image {
                    before: Some(include_bytes!("../assets/demo/icon-before.png").to_vec()),
                    after: Some(include_bytes!("../assets/demo/icon-after.png").to_vec()),
                    format: String::from("png"),
                };
            }
            // The document arriving nearly whole, so Source has real lines to
            // show when the capture's Preview is toggled off.
            Self::Preview => ("# Sourcefour\n", PREVIEW_MARKDOWN),
            _ => (SIDEBAR_BEFORE, SIDEBAR_AFTER),
        };
        sourcefour_git::unified(
            before.as_bytes(),
            after.as_bytes(),
            sourcefour_git::DiffLimits::default(),
        )
    }
}

/// The image [`PREVIEW_MARKDOWN`] resolves, reusing the image scene's bytes.
pub(crate) const PREVIEW_IMAGE: &[u8] = include_bytes!("../assets/demo/icon-after.png");
/// The reference [`PREVIEW_IMAGE`] answers.
pub(crate) const PREVIEW_IMAGE_PATH: &str = "assets/demo/icon-after.png";
/// A reference nothing answers, so the capture covers the not-found frame.
pub(crate) const PREVIEW_MISSING_IMAGE_PATH: &str = "assets/demo/retired-logo.png";
/// An http reference, which the preview reports and never fetches.
pub(crate) const PREVIEW_REMOTE_IMAGE_URL: &str =
    "https://img.shields.io/badge/build-passing-green.svg";

/// The document `--scene preview` renders: one of every block the pane draws,
/// and one of every way an image reference can resolve.
pub(crate) const PREVIEW_MARKDOWN: &str = r"# Sourcefour

A fast, native Git history browser you launch from your terminal. Run
`sourcefour` in any repository — it opens on the **current worktree**, reads
through [gix](https://github.com/GitoxideLabs/gitoxide), and never blocks the
window on a *walk*. ~~Electron~~ not included.

![The application window](assets/demo/icon-after.png)

## Getting started

```rust
fn main() {
    sourcefour::launch(std::env::current_dir()?)?;
}
```

1. Install with `cargo install sourcefour`.
2. Open a repository:
   - `sourcefour` uses the working directory
   - `sourcefour ~/code/project` takes a path
3. Press `?` for the key map.

> History loads in batches, so the first rows paint before the walk finishes.

---

![build status](https://img.shields.io/badge/build-passing-green.svg)

![The retired logo](assets/demo/retired-logo.png)
";

/// The two sides of [`Scene::file`], diffed live to build the overlay's lines.
const SIDEBAR_BEFORE: &str = r#"    /// One worktree row: its name, the branch it has checked out, its path.
    fn worktree_row(&self, worktree: &WorktreeSnapshot) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .h(px(ROW_HEIGHT))
            .px(px(10.0))
            .when(worktree.is_current, |row| {
                row.bg(self.theme.bg_selected)
            })
            .child(self.icon("icons/folder.svg", self.theme.text_secondary))
            .child(
                div()
                    .flex_1()
                    .text_color(self.theme.text_primary)
                    .child(worktree.display_name.clone()),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(self.theme.text_faint)
                    .child(head_label(&worktree.head)),
            )
    }

    /// The worktree section: a header carrying the count, then a row each.
    fn worktree_section(&self, cx: &mut Context<Self>) -> Div {
        let worktrees = self.repo.value().map(|repo| repo.worktrees.clone());
        let count = worktrees.as_ref().map_or(0, Vec::len);
        div()
            .flex()
            .flex_col()
            .child(self.section_header("WORKTREES", count, Section::Worktrees, cx))
            .when(!self.sections.worktrees, |section| {
                section.children(
                    worktrees
                        .into_iter()
                        .flatten()
                        .map(|worktree| self.worktree_row(&worktree)),
                )
            })
    }

/// What a worktree's HEAD points at, in the shortest honest form.
fn head_label(head: &HeadSnapshot) -> String {
    match head {
        HeadSnapshot::Branch { short_name, .. } => short_name.clone(),
        HeadSnapshot::Detached { oid } => oid.to_short_hex(),
        HeadSnapshot::Unborn { short_name } => format!("{short_name} (unborn)"),
    }
}
"#;

const SIDEBAR_AFTER: &str = r#"    /// One worktree row: its name, the branch it has checked out, its path,
    /// and — when the checkout is locked or gone — why it cannot be used.
    fn worktree_row(&self, worktree: &WorktreeSnapshot) -> Div {
        let missing = worktree.accessibility != WorktreeAccessibility::Accessible;
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .h(px(ROW_HEIGHT))
            .px(px(10.0))
            .when(worktree.is_current, |row| {
                row.bg(self.theme.bg_selected)
            })
            .child(self.icon("icons/folder.svg", self.theme.text_secondary))
            .child(
                div()
                    .flex_1()
                    .text_color(if missing {
                        self.theme.text_faint
                    } else {
                        self.theme.text_primary
                    })
                    .child(worktree.display_name.clone()),
            )
            .when(worktree.is_locked, |row| {
                row.child(self.icon("icons/lock.svg", self.theme.orange))
            })
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(self.theme.text_faint)
                    .child(head_label(&worktree.head)),
            )
    }

    /// The worktree section: a header carrying the count, then a row each.
    fn worktree_section(&self, cx: &mut Context<Self>) -> Div {
        let worktrees = self.repo.value().map(|repo| repo.worktrees.clone());
        let count = worktrees.as_ref().map_or(0, Vec::len);
        div()
            .flex()
            .flex_col()
            .child(self.section_header("WORKTREES", count, Section::Worktrees, cx))
            .when(!self.sections.worktrees, |section| {
                section.children(
                    worktrees
                        .into_iter()
                        .flatten()
                        .map(|worktree| self.worktree_row(&worktree)),
                )
            })
    }

/// What a worktree's HEAD points at, in the shortest honest form.
fn head_label(head: &HeadSnapshot) -> String {
    match head {
        HeadSnapshot::Branch { short_name, .. } => short_name.clone(),
        HeadSnapshot::Detached { oid } => format!("detached at {}", oid.to_short_hex()),
        HeadSnapshot::Unborn { short_name } => format!("{short_name} (unborn)"),
    }
}
"#;

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
