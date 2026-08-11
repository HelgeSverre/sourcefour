//! The Sourcefour window: state, data plumbing, and the render root.
//!
//! Each surface lives in its own child module — descendants of this module,
//! so they extend `SourcefourWindow` with their own `impl` blocks and reach
//! its private fields directly:
//!
//! - [`chrome`] — titlebar, toolbar, filter, status bar, splitters, fetch
//! - [`sidebar`] — the reorderable worktrees/branches/remotes/Actions panel
//! - [`history_pane`] — the virtualized commit list and graph canvas
//! - [`details`] — the commit message and changed-files pane
//! - [`diff`] — the full-window diff overlay, text and image
//! - [`preview`] — the overlay's rendered-Markdown pane and its toggle
//! - [`branch_dialog`] — the create-branch dialog
//! - [`github`] — the GitHub connection and its cached read surfaces
//!
//! The settings overlay is a sibling (`crate::settings_ui`) reaching in
//! through `pub(crate)` methods only.

use gpui::{
    Div, FocusHandle, FontWeight, IntoElement, Render, UniformListScrollHandle, Window, actions,
    div, prelude::*, px,
};
use std::sync::Arc;

use sourcefour_git::{GixHistoryCursor, HistoryCursor as _};
use sourcefour_model::{
    AheadBehindUpdate, ChangeKind, CommitFiles, DiffParent, Generation, HistoryBatch, HistoryQuery,
    HistoryScope, LoadState, RepoEnvelope, RepoFailure, RepoLocation, RepoSessionId, RepoSnapshot,
    RequestId,
};

use crate::{
    app::WindowLaunch,
    demo,
    history::{HistoryState, refreshed_scope},
    panels::{PanelSizes, Splitter},
    theme::{TITLEBAR_HEIGHT, Theme},
    ui_state::{UiState, WindowMode, WindowState},
};

mod actions;
mod branch_dialog;
mod chrome;
mod details;
mod diff;
mod github;
mod history_pane;
mod preview;
mod sidebar;

use branch_dialog::BranchDialog;
use diff::{DiffMode, DiffView};
use github::{Cached, GithubChecks};
use sidebar::{SidebarSections, head_label};

struct DiffCacheEntry {
    request: sourcefour_model::FileDiffRequest,
    diff: Arc<sourcefour_model::TextDiff>,
    bytes: usize,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent view-state flags on the one window root"
)]
pub(crate) struct SourcefourWindow {
    /// Repository name, stable across the repository's worktrees.
    name: String,
    /// Display-friendly active worktree path.
    path: String,
    demo: bool,
    window_state: Option<WindowState>,
    window_save_generation: u64,
    /// Identity every background result must match to be applied.
    session: RepoSessionId,
    generation: Generation,
    /// Kept so a refresh can re-read without re-discovering.
    location: Option<RepoLocation>,
    repo: LoadState<RepoSnapshot>,
    history: HistoryState,
    /// Owned between batches; moved to a worker while one is in flight.
    cursor: Option<GixHistoryCursor>,
    theme: Theme,
    sections: SidebarSections,
    /// Collapsed slash-delimited folders in the local branch tree.
    collapsed_branch_folders: std::collections::HashSet<String>,
    panels: PanelSizes,
    /// The splitter a mouse drag is currently moving.
    /// The one drag a held mouse button performs; splitters and the diff
    /// overlay's scrub controls are mutually exclusive by construction.
    drag: Option<Drag>,
    /// Whether the first-frame startup report has fired (§12.5).
    startup_reported: bool,
    /// Full metadata for the selected commit, when loaded (§6.10).
    detail: Option<sourcefour_model::CommitDetail>,
    /// Changed files for the selected commit, when loaded (§6.10).
    files: Option<CommitFiles>,
    /// The commit the current detail/files load belongs to, requested or done.
    files_for: Option<sourcefour_model::Oid>,
    /// The working tree's uncommitted state, when read (§6.14 limits how
    /// fresh it can be: reloads, activation, and operations refresh it).
    working_tree_status: Option<sourcefour_model::WorkingTreeStatus>,
    /// Whether the first successful status read may choose the dirty tree.
    initial_selection_pending: bool,
    /// The commit message being written.
    commit_input: gpui::Entity<crate::text_input::TextInput>,
    /// Whether a commit is running; the button disables while one is.
    committing: bool,
    /// Token for the newest status read; stale completions bail out.
    status_request: u64,
    /// Token for the newest detail/files request; stale completions bail out.
    files_request: u64,
    /// The open diff overlay, if any (§6.11).
    diff_view: Option<DiffView>,
    /// Token for the newest diff request.
    diff_request: u64,
    /// Small session LRU for immutable commit text diffs.
    diff_cache: std::collections::VecDeque<DiffCacheEntry>,
    /// Focus target while the diff overlay is open, so Escape closes it.
    diff_focus: FocusHandle,
    /// Search field shown by the diff overlay's file switcher.
    diff_file_input: gpui::Entity<crate::text_input::TextInput>,
    diff_switcher_open: bool,
    diff_switcher_selection: usize,
    /// Scroll position of the diff overlay's line list.
    diff_scroll: UniformListScrollHandle,
    /// The text one block of the rendered preview has selected, if any.
    preview_selection: Option<preview::PreviewSelection>,
    /// Text layouts of the preview blocks this frame drew, so a drag can map
    /// a window position back to a byte index.
    preview_layouts: preview::PreviewLayouts,
    /// The user's persisted configuration (settings.json).
    settings: crate::settings::AppSettings,
    /// The settings overlay's visible section, while open.
    settings_view: Option<crate::settings_ui::SettingsSection>,
    /// Focus target while the settings overlay is open, so Escape closes it.
    settings_focus: FocusHandle,
    /// Where the GitHub connection stands (§ settings, GitHub).
    github_connection: crate::settings_ui::GithubConnection,
    /// Token for the newest connection check, so stale results drop.
    github_request: u64,
    /// The masked personal-access-token field of the GitHub section.
    token_input: gpui::Entity<crate::text_input::TextInput>,
    /// The GitHub repository behind the remotes, when the integration is on.
    github_remote: Option<sourcefour_github::GithubRemote>,
    /// Open pull requests with their fetch time, for branch chips.
    github_pulls: Option<Cached<Vec<sourcefour_model::PrSummary>>>,
    /// Token for the newest pull-request load, so stale results drop.
    github_pulls_request: u64,
    /// The selected commit's check runs: commit, outcome, fetch time.
    github_checks: Option<Cached<(sourcefour_model::Oid, GithubChecks)>>,
    /// Token for the newest check-run load, so stale results drop.
    github_checks_request: u64,
    /// Rolled-up CI state per commit, for the history-row dots.
    github_states: Option<
        Cached<std::collections::HashMap<sourcefour_model::Oid, sourcefour_model::CheckStatus>>,
    >,
    /// Token for the newest rollup load, so stale results drop.
    github_states_request: u64,
    /// GitHub Actions overlay, prefetch, request, and polling state.
    actions: actions::ActionsState,
    /// Focus target while the Actions overlay is open, so Escape closes it.
    actions_focus: FocusHandle,
    /// Scroll position of the overlay's virtualized job rail.
    actions_jobs_scroll: UniformListScrollHandle,
    /// Scroll position of the overlay's log list.
    actions_log_scroll: UniformListScrollHandle,
    /// Labels and bars share this scroll position in the expanded timeline.
    actions_timeline_scroll: gpui::ScrollHandle,
    actions_timeline_expanded: bool,
    /// Recent Actions workflow runs with their fetch time.
    github_runs: Option<Cached<Vec<sourcefour_model::WorkflowRun>>>,
    /// Token for the newest workflow-run load, so stale results drop.
    github_runs_request: u64,
    /// Retires pending-only GitHub status polling loops.
    github_status_poll: u64,
    /// Juxtapose area bounds captured at paint, for mapping mouse X.
    juxtapose_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    /// The last chosen diff layout, persisted across launches.
    preferred_diff_mode: DiffMode,
    /// Which parent the selection's files and diffs compare against (§6.10).
    compare_parent: DiffParent,
    /// §4.6: Space toggles the details pane collapsed.
    details_collapsed: bool,
    /// The mutually exclusive toolbar network operation state (§6.12).
    network_operation: chrome::NetworkOperationState,
    /// The last operation outcome — any operation: success flag and message.
    op_status: Option<(bool, String)>,
    /// The §6.13 create-branch dialog, when open.
    branch_dialog: Option<BranchDialog>,
    /// Name field of the create-branch dialog.
    branch_input: gpui::Entity<crate::text_input::TextInput>,
    /// Focus target of the details pane (§4.6: Enter focuses it).
    details_focus: FocusHandle,
    list_scroll: UniformListScrollHandle,
    focus: FocusHandle,
    /// The §4.7 filter field.
    filter_input: gpui::Entity<crate::text_input::TextInput>,
}

actions!(
    sourcefour,
    [
        SelectNextCommit,
        SelectPreviousCommit,
        SelectFirstCommit,
        SelectLastLoadedCommit,
        PageDown,
        PageUp,
        FocusFilter,
        FilterEscape,
        FilterEnter,
        CloseDiff,
        CopyPreviewSelection,
        NextDiffHunk,
        PrevDiffHunk,
        NextDiffFile,
        PrevDiffFile,
        ShowUnifiedDiff,
        ShowSplitDiff,
        ToggleDiffWhitespace,
        ToggleDiffWrap,
        OpenDiffFileSwitcher,
        NextDiffSwitcherResult,
        PrevDiffSwitcherResult,
        ToggleDetails,
        FocusDetails,
        OpenSettings,
        CloseSettings,
        CloseActionsRun,
        NextActionsJob,
        PrevActionsJob,
        NextActionsStep,
        PrevActionsStep,
    ]
);

/// Rows a page key moves, matching the baseline viewport's row count.
const PAGE_ROWS: isize = 20;

/// What a held mouse button is dragging, window-wide.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Drag {
    /// A panel splitter handle.
    Splitter(Splitter),
    /// The diff overlay's scrollbar thumb.
    DiffBar,
    /// The image juxtapose divider.
    ImageSlider,
    /// A selection being drawn across one preview block's text.
    PreviewText,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ColumnVisibility {
    author: bool,
    hash: bool,
}

impl ColumnVisibility {
    /// Columns hide by the width left for the history pane, not the window:
    /// a widened sidebar must squeeze columns exactly like a narrow window.
    fn for_available_width(width: f32) -> Self {
        Self {
            author: width >= 764.0,
            hash: width >= 704.0,
        }
    }
}

/// §6.14 coalescing interval: long enough to batch a burst of ref writes, short
/// enough that an externally created branch appears without feeling delayed.
const METADATA_POLL: std::time::Duration = std::time::Duration::from_millis(150);

/// Diagnostic identities for the window's read requests (§5.5).
const SNAPSHOT_REQUEST: RequestId = RequestId(0);
const AHEAD_BEHIND_REQUEST: RequestId = RequestId(1);
const HISTORY_REQUEST: RequestId = RequestId(2);

/// §6.10 selection debounce before changed files are decoded.
const FILES_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(50);

/// §4.6: relative dates refresh once per minute, never per frame.
const MINUTE_TICK: u16 = 60;

/// Whether a background result still belongs to the window that asked for it.
///
/// A result from a superseded generation is dropped rather than applied; this
/// is how cheap reads are cancelled (§5.5).
fn belongs_to<T>(
    session: RepoSessionId,
    generation: Generation,
    envelope: &RepoEnvelope<T>,
) -> bool {
    envelope.session == session && envelope.generation == generation
}

/// The one-letter status a changed file shows, following Git's own letters.
fn change_letter(status: ChangeKind) -> &'static str {
    match status {
        ChangeKind::Added => "A",
        ChangeKind::Modified => "M",
        ChangeKind::Deleted => "D",
        ChangeKind::Renamed => "R",
        ChangeKind::Copied => "C",
        ChangeKind::Unknown => "?",
    }
}

/// Pluralizes a count, because "1 worktrees" reads like a bug.
fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else if let Some(stem) = noun.strip_suffix("ch") {
        format!("{count} {stem}ches")
    } else {
        format!("{count} {noun}s")
    }
}

/// The demo must render identically on every machine, so it never reads
/// this user's settings file (§12.4).
fn initial_settings(demo: bool) -> crate::settings::AppSettings {
    if demo {
        crate::settings::AppSettings::default()
    } else {
        crate::settings::AppSettings::load()
    }
}

/// The status color of one changed file, shared by list and diff header.
fn change_color(theme: &Theme, status: ChangeKind) -> gpui::Hsla {
    match status {
        ChangeKind::Added => theme.green,
        ChangeKind::Deleted => theme.red,
        ChangeKind::Modified => theme.orange,
        ChangeKind::Renamed | ChangeKind::Copied => theme.purple,
        ChangeKind::Unknown => theme.text_faint,
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "row indices stay far below f32's exact-integer range"
)]
fn row_count_as_f32(value: usize) -> f32 {
    value as f32
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is a non-negative floor/ceil of a visible row count"
)]
fn usize_from_f32(value: f32) -> usize {
    value.max(0.0) as usize
}

impl SourcefourWindow {
    #[expect(
        clippy::too_many_lines,
        reason = "the window constructor initializes one explicit field per view state"
    )]
    pub(crate) fn new(
        launch: WindowLaunch,
        ui_state: &UiState,
        gpui_window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        Self::watch_activation(gpui_window, cx);
        let session = RepoSessionId::new();
        let generation = Generation(0);
        let mut window = Self {
            name: launch.name,
            path: launch.path,
            demo: launch.demo,
            window_state: ui_state.window,
            window_save_generation: 0,
            session,
            generation,
            location: None,
            repo: LoadState::Idle,
            history: HistoryState::default(),
            cursor: None,
            theme: Theme::dark(),
            sections: SidebarSections::default(),
            collapsed_branch_folders: std::collections::HashSet::new(),
            panels: PanelSizes::default(),
            drag: None,
            startup_reported: false,
            detail: None,
            files: None,
            files_for: None,
            working_tree_status: None,
            initial_selection_pending: !launch.demo,
            commit_input: cx.new(|cx| {
                crate::text_input::TextInput::new("Describe the change", &Theme::dark(), cx)
                    .role(crate::text_input::InputRole::CommitMessage)
                    .multiline(4)
            }),
            committing: false,
            status_request: 0,
            files_request: 0,
            diff_view: None,
            diff_request: 0,
            diff_cache: std::collections::VecDeque::new(),
            diff_focus: cx.focus_handle(),
            diff_file_input: Self::input(
                "Switch file…",
                crate::text_input::InputRole::DiffSwitcher,
                cx,
            ),
            diff_switcher_open: false,
            diff_switcher_selection: 0,
            diff_scroll: UniformListScrollHandle::new(),
            preview_selection: None,
            preview_layouts: preview::PreviewLayouts::default(),
            settings: initial_settings(launch.demo),
            settings_view: None,
            settings_focus: cx.focus_handle(),
            github_connection: crate::settings_ui::GithubConnection::Idle,
            github_request: 0,
            github_remote: None,
            github_pulls: None,
            github_pulls_request: 0,
            github_checks: None,
            github_checks_request: 0,
            github_states: None,
            github_states_request: 0,
            actions: actions::ActionsState::default(),
            actions_focus: cx.focus_handle(),
            actions_jobs_scroll: UniformListScrollHandle::new(),
            actions_log_scroll: UniformListScrollHandle::new(),
            actions_timeline_scroll: gpui::ScrollHandle::new(),
            actions_timeline_expanded: false,
            github_runs: None,
            github_runs_request: 0,
            github_status_poll: 0,
            token_input: Self::masked_token_input(cx),
            juxtapose_bounds: std::rc::Rc::default(),
            preferred_diff_mode: DiffMode::Unified,
            compare_parent: DiffParent::FirstParent,
            details_collapsed: false,
            details_focus: cx.focus_handle(),
            network_operation: chrome::NetworkOperationState::default(),
            op_status: None,
            branch_dialog: None,
            branch_input: Self::input(
                "new-branch-name",
                crate::text_input::InputRole::BranchName,
                cx,
            ),
            list_scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            filter_input: Self::input(
                "Filter commits",
                crate::text_input::InputRole::HistoryFilter,
                cx,
            ),
        };
        Self::watch_filter(&window.filter_input, cx);
        cx.observe(&window.diff_file_input, |this, _, cx| {
            this.diff_switcher_selection = 0;
            cx.notify();
        })
        .detach();
        window.apply_ui_state(ui_state);
        Self::watch_window_bounds(gpui_window, cx);
        // §4.6: relative dates refresh once a minute, never per frame.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(u64::from(MINUTE_TICK)))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        })
        .detach();
        if launch.demo {
            window.seed_demo(launch.scene, cx);
        } else if let Some(location) = launch.location {
            window.repo = LoadState::Loading {
                started_at: std::time::Instant::now(),
            };
            window.location = Some(location.clone());
            window.load_metadata(location.clone(), cx);
            Self::watch_metadata(location, cx);
        }
        window.focus.focus(gpui_window);
        window
    }

    /// The input owns the text; the window derives the filtered view.
    fn watch_filter(
        filter_input: &gpui::Entity<crate::text_input::TextInput>,
        cx: &mut gpui::Context<Self>,
    ) {
        cx.observe(filter_input, |this, input, cx| {
            let text = input.read(cx).text().to_string();
            if this.history.filter != text {
                this.history.set_filter(&text);
                cx.notify();
            }
        })
        .detach();
    }

    /// Seeds the deterministic capture fixture as if a traversal had
    /// already completed, so every capture goes through the real rendering
    /// path (§12.4).
    fn seed_demo(&mut self, scene: demo::Scene, cx: &mut gpui::Context<Self>) {
        self.repo = LoadState::Ready(demo::snapshot());
        self.history.reset(HistoryScope::AllRefs);
        let (rows, layout) = demo::history();
        self.history.extend(rows, layout, false);
        self.detail = Some(demo::detail());
        self.files = Some(demo::files());
        self.files_for = self.history.selected_commit();
        self.seed_scene(scene, cx);
    }

    /// Refreshes the working tree when the window becomes active again.
    ///
    /// §6.14 forbids watching the worktree, so returning to the window is
    /// the moment edits made elsewhere become visible.
    fn watch_activation(gpui_window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        cx.observe_window_activation(gpui_window, |this, gpui_window, cx| {
            if gpui_window.is_window_active() && !this.demo {
                this.load_working_tree_status(cx);
            }
        })
        .detach();
    }

    /// Records move and resize events after they settle, avoiding a disk write
    /// for every intermediate frame of a drag.
    fn watch_window_bounds(gpui_window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        cx.observe_window_bounds(gpui_window, |this, gpui_window, cx| {
            if this.demo {
                return;
            }
            let bounds = gpui_window.window_bounds();
            let mode = match bounds {
                gpui::WindowBounds::Windowed(_) => WindowMode::Windowed,
                gpui::WindowBounds::Maximized(_) => WindowMode::Maximized,
                gpui::WindowBounds::Fullscreen(_) => WindowMode::Fullscreen,
            };
            let bounds = bounds.get_bounds();
            this.window_state = Some(WindowState {
                width: bounds.size.width.0,
                height: bounds.size.height.0,
                mode,
            });
            this.window_save_generation = this.window_save_generation.wrapping_add(1);
            let generation = this.window_save_generation;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.window_save_generation == generation {
                        this.persist_ui_state(cx);
                    }
                });
            })
            .detach();
        })
        .detach();
    }

    /// Re-reads metadata when refs or worktrees change outside the application.
    ///
    /// Polling at the §6.14 coalescing interval costs one stat pass over the
    /// metadata paths, which is far cheaper than a recursive worktree watcher
    /// and behaves identically on every platform.
    fn watch_metadata(location: RepoLocation, cx: &mut gpui::Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut watcher = sourcefour_git::MetadataWatcher::new(&location);
            loop {
                cx.background_executor().timer(METADATA_POLL).await;
                // The watcher moves to the background thread and back rather
                // than being cloned, so polling costs no allocation per tick.
                let (change, returned) = cx
                    .background_executor()
                    .spawn(async move {
                        let change = watcher.poll();
                        (change, watcher)
                    })
                    .await;
                watcher = returned;
                if change.is_none() {
                    continue;
                }
                // The window is gone when the update errs; nothing to serve.
                if this.update(cx, Self::begin_reload).is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Loads metadata, then divergence counts, without blocking either render.
    ///
    /// Reading refs and worktrees touches the filesystem, so it never runs on
    /// the render thread (§15.8). Ahead/behind follows as a second pass so the
    /// sidebar appears before any history is walked (§6.7).
    /// The worktree an operation targets: the snapshot's active one, or the
    /// stable placeholder used before metadata has arrived.
    pub(super) fn active_worktree_id(&self) -> sourcefour_model::WorktreeId {
        self.snapshot()
            .and_then(|snapshot| snapshot.active_worktree.clone())
            .unwrap_or_else(|| sourcefour_model::WorktreeId(String::from("active")))
    }

    /// Starts a metadata reload of the current location: a new generation
    /// retires every result still in flight (§5.5) and the loaded snapshot
    /// stays usable on screen as `Refreshing` until the fresh one arrives
    /// (§11.2). Every reload path routes through here.
    pub(super) fn begin_reload(&mut self, cx: &mut gpui::Context<Self>) {
        self.generation = Generation(self.generation.0 + 1);
        if let Some(current) = self.repo.value().cloned() {
            self.repo = LoadState::Refreshing {
                current,
                started_at: std::time::Instant::now(),
            };
        }
        if let Some(location) = self.location.clone() {
            self.load_metadata(location, cx);
        }
        cx.notify();
    }

    fn load_metadata(&mut self, location: RepoLocation, cx: &mut gpui::Context<Self>) {
        let session = self.session;
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let payload = cx
                .background_executor()
                .spawn({
                    let location = location.clone();
                    async move { sourcefour_git::snapshot(&location) }
                })
                .await;
            let loaded = payload.is_ok();
            let branches = payload
                .as_ref()
                .map(|snapshot| snapshot.local_branches.clone())
                .unwrap_or_default();
            let applied = this
                .update(cx, |this, cx| {
                    let applied = this.apply_snapshot(&RepoEnvelope {
                        session,
                        generation,
                        request: SNAPSHOT_REQUEST,
                        payload,
                    });
                    // Labels come from the snapshot, so history starts only once
                    // the reference pass has produced them (§6.6). A failed
                    // reload keeps the stale-but-usable history instead (§11.2).
                    if applied && loaded {
                        // A watcher-triggered reload must not discard the user's
                        // context: the scope survives with a re-resolved tip and
                        // the selection re-attaches when its commit reloads
                        // (§6.12).
                        let scope = refreshed_scope(this.history.scope.as_ref(), &branches);
                        let selected = this.history.selected;
                        this.start_history(scope, cx);
                        this.history.selected = selected;
                        this.refresh_github(cx);
                        this.load_working_tree_status(cx);
                    }
                    cx.notify();
                    applied
                })
                .unwrap_or(false);
            if !applied || branches.is_empty() {
                return;
            }

            let updates = cx
                .background_executor()
                .spawn(async move {
                    let mut cache = sourcefour_git::AheadBehindCache::default();
                    cache.update(&location, &branches)
                })
                .await;
            this.update(cx, |this, cx| {
                this.apply_ahead_behind(&RepoEnvelope {
                    session,
                    generation,
                    request: AHEAD_BEHIND_REQUEST,
                    payload: updates,
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Starts a traversal for `scope`, discarding anything already loaded.
    fn start_history(&mut self, scope: HistoryScope, cx: &mut gpui::Context<Self>) {
        let Some(location) = self.location.clone() else {
            return;
        };
        let labels = self
            .snapshot()
            .map(|snapshot| snapshot.labels_by_object.clone())
            .unwrap_or_default();
        // Dropping the previous cursor ends its worker thread.
        self.cursor = None;
        self.history.reset(scope.clone());
        self.detail = None;
        self.files = None;
        self.files_for = None;
        match GixHistoryCursor::start(&location, HistoryQuery { scope }, labels) {
            Ok(cursor) => {
                self.cursor = Some(cursor);
                self.request_batch(cx);
            }
            Err(failure) => {
                tracing::error!(%failure, "history could not start");
                self.history.has_more = false;
            }
        }
    }

    /// Requests the next batch, if one is warranted and none is in flight.
    fn request_batch(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(mut cursor) = self.cursor.take() else {
            return;
        };
        self.history.request_in_flight = true;
        let session = self.session;
        let generation = self.generation;
        let epoch = self.history.epoch;
        let rows = cursor.next_batch_size();
        cx.spawn(async move |this, cx| {
            // The cursor moves to the worker and back so the render thread never
            // waits on a traversal.
            let (batch, cursor) = cx
                .background_executor()
                .spawn(async move {
                    let batch = cursor.next_batch(rows);
                    (batch, cursor)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.history.epoch != epoch {
                    // The scope changed while this batch was in flight. Its rows
                    // belong to the abandoned traversal, and so does the cursor:
                    // restoring it would overwrite the new scope's cursor and
                    // feed old-scope rows into the new list. Dropping it here
                    // also stops its worker thread.
                    return;
                }
                this.cursor = Some(cursor);
                this.apply_batch(&RepoEnvelope {
                    session,
                    generation,
                    request: HISTORY_REQUEST,
                    payload: batch,
                });
                // The first batch selects the newest row; its files follow.
                this.load_selected_files(cx);
                // CI dots decorate loaded rows; the cache absorbs repeats.
                this.load_commit_states(false, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Appends a batch that still belongs to the current scope.
    fn apply_batch(&mut self, envelope: &RepoEnvelope<Result<HistoryBatch, RepoFailure>>) -> bool {
        if !belongs_to(self.session, self.generation, envelope) {
            return false;
        }
        self.history.request_in_flight = false;
        match &envelope.payload {
            Ok(batch) => {
                self.history
                    .extend(batch.rows.clone(), batch.graph_rows.clone(), batch.has_more);
                true
            }
            Err(failure) => {
                tracing::error!(%failure, "history batch failed");
                self.history.has_more = false;
                false
            }
        }
    }

    /// Moves the selection and keeps it visible.
    fn move_selection(&mut self, delta: isize, cx: &mut gpui::Context<Self>) {
        self.initial_selection_pending = false;
        if let Some(index) = self.history.move_selection(delta) {
            self.list_scroll
                .scroll_to_item(index, gpui::ScrollStrategy::Top);
            self.load_selected_files(cx);
            cx.notify();
        }
    }

    /// Selects one loaded row by index and keeps it visible.
    fn select_row(&mut self, index: usize, cx: &mut gpui::Context<Self>) {
        self.initial_selection_pending = false;
        if let Some(index) = self.history.select_index(index) {
            self.list_scroll
                .scroll_to_item(index, gpui::ScrollStrategy::Top);
            self.load_selected_files(cx);
            cx.notify();
        }
    }

    /// Reads the working tree's status off the render thread.
    ///
    /// Called on every snapshot apply, on window activation, and when the
    /// working-tree row is selected; a stale token drops superseded results.
    pub(super) fn load_working_tree_status(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(location) = self.location.clone() else {
            return;
        };
        self.status_request += 1;
        let token = self.status_request;
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async move { sourcefour_git::working_tree_status(&location) })
                .await;
            this.update(cx, |this, cx| {
                if this.status_request != token {
                    return;
                }
                match status {
                    Ok(status) => {
                        if this.apply_working_tree_status(status) {
                            this.load_selected_files(cx);
                        }
                    }
                    Err(failure) => {
                        tracing::error!(%failure, "working tree status could not load");
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn apply_working_tree_status(&mut self, status: sourcefour_model::WorkingTreeStatus) -> bool {
        let select_when_dirty = self.initial_selection_pending;
        self.initial_selection_pending = false;
        let selection_changed = self.history.apply_working_tree(
            (!status.is_clean()).then(|| status.summary()),
            select_when_dirty,
        );
        self.working_tree_status = Some(status);
        selection_changed
    }

    /// Runs `git commit` with the message, through the §10 runner:
    /// hooks and signing apply, and a failure lands in the status bar with
    /// the hook's own words.
    pub(super) fn start_commit(&mut self, cx: &mut gpui::Context<Self>) {
        if self.committing {
            return;
        }
        let summary = self.commit_input.read(cx).text().trim().to_string();
        if summary.is_empty() {
            return;
        }
        let Some(location) = self.location.clone() else {
            return;
        };
        self.committing = true;
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let cancelled = std::sync::atomic::AtomicBool::new(false);
                    sourcefour_git::commit(&location, &summary, &DiscardSink, &cancelled)
                })
                .await;
            this.update(cx, |this, cx| {
                this.committing = false;
                match outcome {
                    Ok(sourcefour_model::OperationOutcome::Succeeded { summary, .. }) => {
                        this.op_status = Some((true, summary));
                        this.commit_input
                            .update(cx, |input, cx| input.set_text("", cx));
                        // The new commit appears without waiting for the
                        // watcher's next poll; status re-reads with it.
                        this.begin_reload(cx);
                    }
                    Ok(sourcefour_model::OperationOutcome::Failed { error, .. }) => {
                        this.op_status = Some((false, error.user.message));
                        this.load_working_tree_status(cx);
                    }
                    Ok(sourcefour_model::OperationOutcome::Cancelled { .. }) => {}
                    Err(failure) => {
                        this.op_status = Some((false, failure.user.message));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Applies one stage or unstage through the user's Git, then re-reads
    /// status. Failures land in the status bar like any other operation.
    pub(super) fn edit_index(
        &mut self,
        paths: Vec<sourcefour_model::RepoPath>,
        unstage: bool,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(location) = self.location.clone() else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    if unstage {
                        sourcefour_git::unstage_paths(&location, &paths)
                    } else {
                        sourcefour_git::stage_paths(&location, &paths)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if let Err(failure) = outcome {
                    this.op_status = Some((false, failure.user.message));
                }
                this.load_working_tree_status(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Selects the pinned working-tree row.
    pub(super) fn select_working_tree(&mut self, cx: &mut gpui::Context<Self>) {
        self.initial_selection_pending = false;
        self.history.selected = Some(crate::history::Selection::WorkingTree);
        self.load_selected_files(cx);
        self.load_working_tree_status(cx);
        cx.notify();
    }

    /// Loads the selected commit's changed files after a short debounce.
    ///
    /// §6.10: rapid arrow-key travel must not decode every passed commit, so
    /// the read starts only if the selection still stands after ~50ms. A
    /// result is dropped when the selection or generation moved on.
    fn load_selected_files(&mut self, cx: &mut gpui::Context<Self>) {
        // Checks follow the selection with their own cache and TTL.
        self.load_selected_checks(false, cx);
        let Some(oid) = self.history.selected_commit() else {
            self.detail = None;
            self.files = None;
            self.files_for = None;
            self.files_request += 1;
            return;
        };
        if self.files_for == Some(oid) {
            return;
        }
        if self.files_for.is_some_and(|previous| previous != oid) {
            // A different commit resets the comparison to the first parent.
            self.compare_parent = DiffParent::FirstParent;
        }
        self.files_for = Some(oid);
        self.detail = None;
        self.files = None;
        self.files_request += 1;
        let token = self.files_request;
        let parent = self.compare_parent;
        let Some(location) = self.location.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FILES_DEBOUNCE).await;
            let wanted = this
                .update(cx, |this, _| this.files_request == token)
                .unwrap_or(false);
            if !wanted {
                return;
            }
            let (detail, files) = cx
                .background_executor()
                .spawn(async move {
                    (
                        sourcefour_git::commit_detail(&location, oid),
                        sourcefour_git::commit_files(&location, oid, parent),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                // Only the newest request may apply; anything older is stale.
                if this.files_request != token {
                    return;
                }
                match detail {
                    Ok(detail) => this.detail = Some(detail),
                    Err(failure) => {
                        // Release the claim so selecting this commit again
                        // retries instead of silently showing nothing.
                        this.files_for = None;
                        tracing::error!(%failure, "commit detail could not load");
                    }
                }
                match files {
                    Ok(files) => this.files = Some(files),
                    Err(failure) => {
                        this.files_for = None;
                        tracing::error!(%failure, "changed files could not load");
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Merges divergence counts into the loaded snapshot.
    fn apply_ahead_behind(&mut self, envelope: &RepoEnvelope<Vec<AheadBehindUpdate>>) -> bool {
        if !belongs_to(self.session, self.generation, envelope) {
            return false;
        }
        let Some(snapshot) = self.repo.value_mut() else {
            return false;
        };
        sourcefour_git::apply_ahead_behind(&mut snapshot.local_branches, &envelope.payload);
        true
    }

    /// Applies a metadata result, ignoring one belonging to a past generation.
    ///
    /// Stale rejection is the primary cancellation mechanism for cheap reads
    /// (§5.5); a slow result from a closed repository must never overwrite the
    /// current one.
    fn apply_snapshot(
        &mut self,
        envelope: &RepoEnvelope<Result<RepoSnapshot, RepoFailure>>,
    ) -> bool {
        if !belongs_to(self.session, self.generation, envelope) {
            return false;
        }
        self.repo = match &envelope.payload {
            Ok(snapshot) => LoadState::Ready(snapshot.clone()),
            Err(failure) => LoadState::Failed {
                error: failure.user.clone(),
                previous: self.repo.value().cloned(),
            },
        };
        true
    }

    /// The loaded snapshot, if metadata has arrived.
    fn snapshot(&self) -> Option<&RepoSnapshot> {
        self.repo.value()
    }

    /// Seconds since the epoch, anchored in demo mode so relative dates in
    /// §12.4 captures never drift between runs.
    fn now_seconds(&self) -> i64 {
        if self.demo {
            demo::NOW_SECONDS
        } else {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| {
                    i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
                })
        }
    }

    /// The monospace family every code, hash and log surface renders in,
    /// as the text system wants it. One call per element rather than one
    /// stored copy, so a family picked in the overlay is the next frame's.
    pub(super) fn mono_font(&self) -> gpui::SharedString {
        gpui::SharedString::from(self.settings.appearance.mono_font().to_owned())
    }

    /// Saves panel sizes, section order, and collapse state off-thread.
    fn persist_ui_state(&self, cx: &gpui::Context<Self>) {
        // Demo interactions must never overwrite this user's real state.
        if self.demo {
            return;
        }
        let mut collapsed_branch_folders: Vec<_> =
            self.collapsed_branch_folders.iter().cloned().collect();
        collapsed_branch_folders.sort();
        let state = crate::ui_state::UiState {
            window: self.window_state,
            sidebar_width: Some(self.panels.sidebar),
            graph_width: Some(self.panels.graph),
            details_height: Some(self.panels.details),
            actions_timeline_height: Some(self.panels.actions_timeline),
            section_order: Some(
                self.sections
                    .ordered()
                    .iter()
                    .map(|section| section.name().to_owned())
                    .collect(),
            ),
            collapsed_sections: self.sections.collapsed_names(),
            collapsed_branch_folders,
            diff_mode: Some(self.preferred_diff_mode.name().to_owned()),
            details_collapsed: self.details_collapsed,
            actions_timeline_expanded: self.actions_timeline_expanded,
        };
        cx.background_executor()
            .spawn(async move { state.save() })
            .detach();
    }

    /// Opens the overlay a capture scene asks for, with its content already
    /// loaded (§12.4). Demo mode has no repository to read a diff from, so the
    /// content comes from the fixture — through the shipping formatter.
    fn seed_scene(&mut self, scene: demo::Scene, cx: &mut gpui::Context<Self>) {
        let mode = match scene {
            demo::Scene::Overview => return,
            demo::Scene::Commit => {
                let status = demo::working_tree_status();
                self.history.working_tree = Some(status.summary());
                self.working_tree_status = Some(status);
                self.history.selected = Some(crate::history::Selection::WorkingTree);
                self.detail = None;
                self.files = None;
                return;
            }
            demo::Scene::Settings => {
                self.settings_view = Some(crate::settings_ui::SettingsSection::default());
                return;
            }
            demo::Scene::Actions => {
                self.actions.view = Some(actions::demo_view());
                if std::env::var_os("SOURCEFOUR_ACTIONS_STRESS").is_some() {
                    self.actions_timeline_expanded = true;
                }
                return;
            }
            demo::Scene::Split => DiffMode::Split,
            _ => DiffMode::Unified,
        };
        let preview = scene == demo::Scene::Preview;
        let path = sourcefour_model::RepoPath(scene.file().as_bytes().to_vec());
        let files = Arc::from([sourcefour_model::ChangedFile {
            old_path: Some(sourcefour_model::RepoPath(scene.file().as_bytes().to_vec())),
            new_path: Some(sourcefour_model::RepoPath(scene.file().as_bytes().to_vec())),
            status: ChangeKind::Modified,
            additions: None,
            deletions: None,
            is_binary: false,
        }]);
        // The scenes have no repository behind them; the origin exists so the
        // header can tell a document from a source file.
        let mut view = DiffView::for_working_tree(
            sourcefour_model::DiffPaths::same(path),
            false,
            ChangeKind::Modified,
            files,
            0,
            mode,
        );
        view.replace_content(scene.content());
        view.show_preview = preview;
        // Seeded rather than loaded: a capture cannot wait on an async read.
        view.preview = preview.then(|| preview::demo_state(cx));
        self.diff_view = Some(view);
        self.reset_diff_wrap_list(cx);
    }

    /// Applies everything the persisted interface state remembers.
    fn apply_ui_state(&mut self, state: &crate::ui_state::UiState) {
        self.sections.apply(state);
        self.collapsed_branch_folders = state.collapsed_branch_folders.iter().cloned().collect();
        self.panels.apply(state);
        self.preferred_diff_mode = DiffMode::from_name(state.diff_mode.as_deref());
        self.details_collapsed = state.details_collapsed;
        self.actions_timeline_expanded = state.actions_timeline_expanded;
    }

    fn input(
        placeholder: &'static str,
        role: crate::text_input::InputRole,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Entity<crate::text_input::TextInput> {
        cx.new(|cx| crate::text_input::TextInput::new(placeholder, &Theme::dark(), cx).role(role))
    }

    /// The GitHub section's token field: like every input, but masked.
    fn masked_token_input(
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Entity<crate::text_input::TextInput> {
        cx.new(|cx| {
            crate::text_input::TextInput::new("ghp_… or github_pat_…", &Theme::dark(), cx)
                .role(crate::text_input::InputRole::GithubToken)
                .masked()
        })
    }

    /// Opens the settings overlay and moves focus into it.
    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.settings_view.is_none() {
            self.settings_view = Some(crate::settings_ui::SettingsSection::default());
        }
        self.settings_focus.focus(window);
        cx.notify();
    }

    /// Closes the settings overlay, returning focus to the history.
    pub(crate) fn close_settings(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.settings_view = None;
        self.focus.focus(window);
        cx.notify();
    }

    /// Switches the visible settings section.
    pub(crate) fn set_settings_section(
        &mut self,
        section: crate::settings_ui::SettingsSection,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.settings_view.is_some() {
            self.settings_view = Some(section);
            cx.notify();
        }
    }

    /// Applies one settings mutation, persists it, and lets exactly the
    /// sections that changed react. Sections derive `PartialEq`, so "what
    /// changed" needs no bookkeeping beyond the comparison.
    pub(crate) fn update_settings(
        &mut self,
        cx: &mut gpui::Context<Self>,
        apply: impl FnOnce(&mut crate::settings::AppSettings),
    ) {
        let before = self.settings.clone();
        apply(&mut self.settings);
        if self.settings == before {
            return; // Re-selecting the active choice is not a change.
        }
        // Demo interactions must never overwrite this user's real settings.
        if !self.demo {
            let settings = self.settings.clone();
            cx.background_executor()
                .spawn(async move { settings.save() })
                .detach();
        }
        // A changed toggle or auth method takes effect without a restart — and
        // a changed line height does not go asking GitHub anything.
        if self.settings.github != before.github {
            self.refresh_github(cx);
        }
        cx.notify();
    }

    /// The settings overlay, while open.
    fn settings_overlay(&self, cx: &mut gpui::Context<Self>) -> Option<impl IntoElement + use<>> {
        let section = self.settings_view?;
        Some(crate::settings_ui::overlay(
            &crate::settings_ui::SettingsView {
                settings: &self.settings,
                section,
                connection: &self.github_connection,
                token_input: &self.token_input,
                theme: &self.theme,
                focus: &self.settings_focus,
            },
            cx,
        ))
    }
}

impl SourcefourWindow {
    /// Registers every window-level action and drag handler on the root.
    #[expect(
        clippy::too_many_lines,
        reason = "all window action routing stays centralized and auditable"
    )]
    fn root_actions(root: Div, cx: &mut gpui::Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &SelectNextCommit, _, cx| {
            this.move_selection(1, cx);
        }))
        .on_action(cx.listener(|this, _: &SelectPreviousCommit, _, cx| {
            this.move_selection(-1, cx);
        }))
        .on_action(cx.listener(|this, _: &PageDown, _, cx| {
            this.move_selection(PAGE_ROWS, cx);
        }))
        .on_action(cx.listener(|this, _: &PageUp, _, cx| {
            this.move_selection(-PAGE_ROWS, cx);
        }))
        .on_action(cx.listener(|this, _: &SelectFirstCommit, _, cx| {
            this.select_row(0, cx);
        }))
        .on_action(cx.listener(|this, _: &SelectLastLoadedCommit, _, cx| {
            let last = this.history.visible_len().saturating_sub(1);
            this.select_row(last, cx);
        }))
        .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
            this.filter_input
                .read(cx)
                .focus_handle
                .clone()
                .focus(window);
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &FilterEscape, window, cx| {
            if this.diff_switcher_open {
                this.close_diff_file_switcher(window, cx);
            } else if this.branch_dialog.is_some() {
                this.branch_dialog = None;
                this.focus.focus(window);
            } else if this.history.filter.is_empty() {
                // First Escape clears the query; a second returns to history.
                this.focus.focus(window);
            } else {
                this.filter_input
                    .update(cx, |input, cx| input.set_text("", cx));
            }
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &FilterEnter, window, cx| {
            if this.diff_switcher_open {
                this.open_selected_diff_file(window, cx);
            } else if this.branch_dialog.is_some() {
                this.submit_branch_dialog(window, cx);
            } else {
                this.focus.focus(window);
            }
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &CloseDiff, window, cx| {
            // Escape gives back the preview's selection first; the overlay
            // closes on the next one.
            if this.clear_diff_selection() || this.clear_preview_selection() {
                cx.notify();
            } else {
                this.close_diff(window, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &CopyPreviewSelection, _, cx| {
            if !this.copy_diff_selection(cx) {
                this.copy_preview_selection(cx);
            }
        }))
        .on_action(cx.listener(|this, _: &NextDiffHunk, _, cx| {
            this.navigate_diff_hunk(1, cx);
        }))
        .on_action(cx.listener(|this, _: &PrevDiffHunk, _, cx| {
            this.navigate_diff_hunk(-1, cx);
        }))
        .on_action(cx.listener(|this, _: &NextDiffFile, window, cx| {
            this.navigate_diff_file(1, window, cx);
        }))
        .on_action(cx.listener(|this, _: &PrevDiffFile, window, cx| {
            this.navigate_diff_file(-1, window, cx);
        }))
        .on_action(cx.listener(|this, _: &ShowUnifiedDiff, _, cx| {
            this.set_diff_mode(DiffMode::Unified, cx);
        }))
        .on_action(cx.listener(|this, _: &ShowSplitDiff, _, cx| {
            this.set_diff_mode(DiffMode::Split, cx);
        }))
        .on_action(cx.listener(|this, _: &ToggleDiffWhitespace, _, cx| {
            let enabled = !this.settings.diff.show_whitespace;
            this.update_settings(cx, |settings| settings.diff.show_whitespace = enabled);
        }))
        .on_action(cx.listener(|this, _: &ToggleDiffWrap, _, cx| {
            let enabled = !this.settings.diff.wrap;
            this.update_settings(cx, |settings| settings.diff.wrap = enabled);
            this.reset_diff_wrap_list(cx);
        }))
        .on_action(cx.listener(|this, _: &OpenDiffFileSwitcher, window, cx| {
            this.open_diff_file_switcher(window, cx);
        }))
        .on_action(cx.listener(|this, _: &NextDiffSwitcherResult, _, cx| {
            if this.diff_switcher_open {
                this.move_diff_switcher_selection(1, cx);
            } else {
                this.move_selection(1, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &PrevDiffSwitcherResult, _, cx| {
            if this.diff_switcher_open {
                this.move_diff_switcher_selection(-1, cx);
            } else {
                this.move_selection(-1, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &ToggleDetails, _, cx| {
            this.details_collapsed = !this.details_collapsed;
            this.persist_ui_state(cx);
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &FocusDetails, window, cx| {
            if this.details_collapsed {
                this.details_collapsed = false;
                this.persist_ui_state(cx);
            }
            this.details_focus.focus(window);
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
            this.open_settings(window, cx);
        }))
        .on_action(cx.listener(|this, _: &CloseSettings, window, cx| {
            this.close_settings(window, cx);
        }))
        .on_action(cx.listener(|this, _: &CloseActionsRun, window, cx| {
            this.close_actions(window, cx);
        }))
        .on_action(cx.listener(|this, _: &NextActionsJob, _, cx| {
            this.select_actions_job(1, cx);
        }))
        .on_action(cx.listener(|this, _: &PrevActionsJob, _, cx| {
            this.select_actions_job(-1, cx);
        }))
        .on_action(cx.listener(|this, _: &NextActionsStep, _, cx| {
            this.select_actions_step(1, cx);
        }))
        .on_action(cx.listener(|this, _: &PrevActionsStep, _, cx| {
            this.select_actions_step(-1, cx);
        }))
        .map(|root| Self::root_drag_handlers(root, cx))
    }

    /// Window-wide drag handlers: splitter handles and scrub starts only arm
    /// state; the movement happens here, so a fast drag cannot escape a
    /// 3px handle.
    fn root_drag_handlers(root: Div, cx: &mut gpui::Context<Self>) -> Div {
        root.on_mouse_move(
            cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                this.drag_move(
                    event.position.x.0,
                    event.position.y.0,
                    event.pressed_button,
                    window,
                    cx,
                );
            }),
        )
        .on_mouse_up(
            gpui::MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.end_drag(cx);
            }),
        )
    }

    /// Routes a held drag to whatever armed it.
    pub(super) fn drag_move(
        &mut self,
        x: f32,
        y: f32,
        pressed: Option<gpui::MouseButton>,
        window: &Window,
        cx: &mut gpui::Context<Self>,
    ) {
        // A release the window never routed — outside its bounds, during a
        // focus switch — must not leave the drag armed and sticky: the move
        // event itself says whether the button is still held.
        if self.drag.is_some() && drag_lost_its_button(pressed) {
            self.end_drag(cx);
            return;
        }
        match self.drag {
            None => {}
            Some(Drag::Splitter(splitter)) => {
                self.panels
                    .drag(splitter, x, y, window.viewport_size().height.0);
                cx.notify();
            }
            Some(Drag::DiffBar) => {
                self.scrub_diff(y);
                cx.notify();
            }
            Some(Drag::ImageSlider) => {
                self.scrub_image(x);
                cx.notify();
            }
            Some(Drag::PreviewText) => {
                self.drag_preview_selection(gpui::point(px(x), px(y)), cx);
            }
        }
    }

    /// Ends whatever drag a released mouse button was holding; splitter
    /// positions persist on release.
    pub(super) fn end_drag(&mut self, cx: &mut gpui::Context<Self>) {
        match self.drag.take() {
            None => {}
            Some(Drag::Splitter(_)) => {
                self.persist_ui_state(cx);
                cx.notify();
            }
            // A text selection outlives the drag that drew it.
            Some(Drag::DiffBar | Drag::ImageSlider | Drag::PreviewText) => cx.notify(),
        }
    }
}

impl Render for SourcefourWindow {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        // §12.5 startup measurement: SOURCEFOUR_STARTUP_LOG=1 prints the
        // milliseconds from process start to the end of the first frame and
        // quits, so a script can sample it repeatedly.
        if !self.startup_reported {
            self.startup_reported = true;
            if std::env::var_os("SOURCEFOUR_STARTUP_LOG").is_some() {
                window.on_next_frame(|_, cx| {
                    println!("first-frame-ms {}", crate::since_process_start());
                    cx.quit();
                });
            }
        }
        let frame_started = std::time::Instant::now();
        self.apply_responsive_diff_mode(window.viewport_size().width.0, cx);
        let columns = ColumnVisibility::for_available_width(
            window.viewport_size().width.0 - self.panels.sidebar,
        );
        let root = div().key_context("History");
        let element = Self::root_actions(root, cx)
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(self.theme.bg_page)
            .text_color(self.theme.text_primary)
            .child(self.titlebar())
            .child(self.toolbar(window, cx))
            .child(
                div()
                    .flex_grow()
                    .flex()
                    .min_h(px(1.0))
                    .child(self.sidebar(cx))
                    .child(self.splitter(Splitter::Sidebar, cx))
                    .child(
                        div()
                            .flex_grow()
                            .flex()
                            .flex_col()
                            .min_w(px(1.0))
                            .bg(self.theme.bg_list)
                            .child(self.header(columns))
                            .child(self.history(columns, cx))
                            .children(
                                (!self.details_collapsed)
                                    .then(|| self.splitter(Splitter::Details, cx)),
                            )
                            .child(self.details(cx)),
                    ),
            )
            .child(self.status())
            .children(self.diff_overlay(cx))
            .children(self.branch_overlay(cx))
            .children(self.settings_overlay(cx))
            .children(self.actions_overlay(window.viewport_size().height.0, cx));
        // §12.5 frame instrumentation: element construction only — layout,
        // paint, and GPU time happen inside gpui after this returns.
        if std::env::var_os("SOURCEFOUR_FRAME_LOG").is_some() {
            let elapsed = frame_started.elapsed();
            if elapsed > std::time::Duration::from_micros(16_700) {
                tracing::warn!(?elapsed, "slow frame build");
            }
        }
        element
    }
}

/// A sink for operations whose progress no interface element shows yet.
struct DiscardSink;

impl sourcefour_git::OperationSink for DiscardSink {
    fn report(&self, _: sourcefour_model::OperationProgress) {}
}

/// Whether a move event proves the dragging button is no longer held.
fn drag_lost_its_button(pressed: Option<gpui::MouseButton>) -> bool {
    pressed != Some(gpui::MouseButton::Left)
}

/// True when a click stayed put — a slider or scrollbar drag released
/// over a backdrop is the end of a drag, not a request to close.
pub(crate) fn is_true_click(event: &gpui::ClickEvent) -> bool {
    let (down, up) = (event.down.position, event.up.position);
    (down.x.0 - up.x.0).abs() <= 3.0 && (down.y.0 - up.y.0).abs() <= 3.0
}

/// The shared modal shell (§4.6 modality): a scrim that occludes
/// everything behind it. Callers add key context, focus, layout, and a
/// close-on-click gated by [`is_true_click`].
pub(crate) fn modal_backdrop(id: &'static str, theme: &Theme) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .occlude()
        .absolute()
        .inset_0()
        .flex()
        .bg(theme.scrim())
}

/// The framed panel every modal floats: raised, bordered, and swallowing
/// clicks so they never reach the backdrop's close handler.
pub(crate) fn modal_panel(id: &'static str, theme: &Theme) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .rounded(px(10.0))
        .border_1()
        .border_color(theme.border_strong)
        .bg(theme.bg_panel)
        .shadow_lg()
        .on_click(|_, _, cx| cx.stop_propagation())
}

/// The window shown instead of the shell when discovery fails.
pub(crate) struct ErrorWindow {
    theme: Theme,
    title: String,
    message: String,
}

impl ErrorWindow {
    pub(crate) fn new(failure: &RepoFailure) -> Self {
        Self {
            theme: Theme::dark(),
            title: failure.user.title.clone(),
            message: failure.user.message.clone(),
        }
    }
}

impl Render for ErrorWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(8.0))
            .px(px(24.0))
            .pt(px(TITLEBAR_HEIGHT))
            .bg(self.theme.bg_page)
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(self.theme.text_primary)
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(self.theme.text_secondary)
                    .child(self.message.clone()),
            )
    }
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{
        Generation, RepoEnvelope, RepoFailure, RepoFailureKind, RepoSessionId, RequestId,
    };

    use super::{ColumnVisibility, ErrorWindow, belongs_to, counted, drag_lost_its_button};

    #[test]
    fn a_drag_survives_only_a_held_left_button() {
        use gpui::MouseButton;

        assert!(!drag_lost_its_button(Some(MouseButton::Left)));
        assert!(
            drag_lost_its_button(None),
            "a release the window never saw must still end the drag"
        );
        assert!(drag_lost_its_button(Some(MouseButton::Right)));
    }

    #[test]
    fn change_letters_follow_git_conventions() {
        use sourcefour_model::ChangeKind;

        use super::change_letter;

        assert_eq!(change_letter(ChangeKind::Added), "A");
        assert_eq!(change_letter(ChangeKind::Modified), "M");
        assert_eq!(change_letter(ChangeKind::Deleted), "D");
        assert_eq!(change_letter(ChangeKind::Renamed), "R");
        assert_eq!(change_letter(ChangeKind::Copied), "C");
        assert_eq!(change_letter(ChangeKind::Unknown), "?");
    }

    #[test]
    fn counts_are_pluralized() {
        assert_eq!(counted(1, "worktree"), "1 worktree");
        assert_eq!(counted(0, "worktree"), "0 worktrees");
        assert_eq!(counted(3, "worktree"), "3 worktrees");
        assert_eq!(counted(1, "branch"), "1 branch");
        assert_eq!(counted(2, "branch"), "2 branches");
        assert_eq!(counted(2, "remote branch"), "2 remote branches");
    }

    #[test]
    fn a_result_from_a_superseded_generation_is_rejected() {
        let session = RepoSessionId::new();
        let envelope = |session, generation| RepoEnvelope {
            session,
            generation,
            request: RequestId(0),
            payload: (),
        };

        assert!(belongs_to(
            session,
            Generation(1),
            &envelope(session, Generation(1))
        ));
        assert!(
            !belongs_to(session, Generation(2), &envelope(session, Generation(1))),
            "a result from an older generation must not be applied"
        );
        assert!(
            !belongs_to(
                session,
                Generation(1),
                &envelope(RepoSessionId::new(), Generation(1))
            ),
            "a result from another session must not be applied"
        );
    }

    #[test]
    fn column_visibility_hides_author_before_hash() {
        assert_eq!(
            ColumnVisibility::for_available_width(1044.0),
            ColumnVisibility {
                author: true,
                hash: true
            }
        );
        assert_eq!(
            ColumnVisibility::for_available_width(744.0),
            ColumnVisibility {
                author: false,
                hash: true
            }
        );
        assert_eq!(
            ColumnVisibility::for_available_width(684.0),
            ColumnVisibility {
                author: false,
                hash: false
            }
        );
    }

    #[test]
    fn the_error_window_shows_the_user_facing_failure_text() {
        let failure = RepoFailure::new(
            RepoFailureKind::NotARepository,
            "Not a Git repository",
            "No Git repository contains /opt.",
        )
        .with_details("internal diagnostics");

        let window = ErrorWindow::new(&failure);

        assert_eq!(window.title, "Not a Git repository");
        assert_eq!(window.message, "No Git repository contains /opt.");
    }
}
