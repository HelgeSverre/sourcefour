use gpui::{
    Div, FocusHandle, FontWeight, IntoElement, Render, StatefulInteractiveElement,
    UniformListScrollHandle, Window, actions, div, prelude::*, px, svg, uniform_list,
};
use std::sync::{Arc, Mutex, atomic::AtomicBool};

use sourcefour_git::{GixHistoryCursor, HistoryCursor as _, OperationSink};
use sourcefour_model::{
    AheadBehindState, AheadBehindUpdate, ChangeKind, ChangedFile, CommitFiles, DiffParent,
    FetchRequest, Generation, HeadSnapshot, HistoryBatch, HistoryQuery, HistoryScope, LoadState,
    OperationOutcome, OperationProgress, RepoEnvelope, RepoFailure, RepoLocation, RepoSessionId,
    RepoSnapshot, RequestId, WorktreeAccessibility,
};

use crate::{
    app::WindowLaunch,
    demo,
    graph_paint::{HALO_OPACITY, HALO_RADIUS, NODE_RADIUS, STROKE_WIDTH, Shape, row_shapes},
    history::{HistoryState, is_scoped_to, refreshed_scope, relative_date, toggled_scope},
    panels::{PanelSizes, Splitter},
    theme::{
        HEADER_HEIGHT, HISTORY_ROW_HEIGHT, MONO_FONT, SPLITTER_WIDTH, STATUS_HEIGHT,
        TITLEBAR_HEIGHT, TOOLBAR_HEIGHT, Theme,
    },
};

mod branch_dialog;
mod diff;
mod github;

use branch_dialog::BranchDialog;
use diff::{DiffMode, DiffView, Scrub};
use github::{Cached, GithubChecks};

pub(crate) struct SourcefourWindow {
    /// Repository name, stable across the repository's worktrees.
    name: String,
    /// Display-friendly active worktree path.
    path: String,
    demo: bool,
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
    panels: PanelSizes,
    /// The splitter a mouse drag is currently moving.
    dragging: Option<Splitter>,
    /// Full metadata for the selected commit, when loaded (§6.10).
    detail: Option<sourcefour_model::CommitDetail>,
    /// Changed files for the selected commit, when loaded (§6.10).
    files: Option<CommitFiles>,
    /// The commit the current detail/files load belongs to, requested or done.
    files_for: Option<sourcefour_model::Oid>,
    /// Token for the newest detail/files request; stale completions bail out.
    files_request: u64,
    /// The open diff overlay, if any (§6.11).
    diff_view: Option<DiffView>,
    /// Token for the newest diff request.
    diff_request: u64,
    /// Focus target while the diff overlay is open, so Escape closes it.
    diff_focus: FocusHandle,
    /// Scroll position of the diff overlay's line list.
    diff_scroll: UniformListScrollHandle,
    /// Which diff-overlay control a held mouse button is dragging.
    scrubbing: Scrub,
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
    /// Recent Actions workflow runs with their fetch time.
    github_runs: Option<Cached<Vec<sourcefour_model::WorkflowRun>>>,
    /// Token for the newest workflow-run load, so stale results drop.
    github_runs_request: u64,
    /// Juxtapose area bounds captured at paint, for mapping mouse X.
    juxtapose_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    /// The last chosen diff layout, persisted across launches.
    preferred_diff_mode: DiffMode,
    /// Which parent the selection's files and diffs compare against (§6.10).
    compare_parent: DiffParent,
    /// §4.6: Space toggles the details pane collapsed.
    details_collapsed: bool,
    /// Latest fetch progress while one runs; `None` when idle (§6.12).
    fetching: Option<Arc<Mutex<Option<OperationProgress>>>>,
    /// Cooperative cancellation flag of the running fetch.
    fetch_cancel: Option<Arc<AtomicBool>>,
    /// The last operation outcome: success flag and message.
    fetch_status: Option<(bool, String)>,
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
        ToggleDetails,
        FocusDetails,
        OpenSettings,
        CloseSettings,
    ]
);

/// Rows a page key moves, matching the baseline viewport's row count.
const PAGE_ROWS: isize = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SidebarSection {
    Worktrees,
    Branches,
    Remotes,
    /// GitHub Actions runs; renders only when a GitHub remote resolves.
    Actions,
}

impl SidebarSection {
    /// Stable name used in the persisted state file.
    fn name(self) -> &'static str {
        match self {
            Self::Worktrees => "worktrees",
            Self::Branches => "branches",
            Self::Remotes => "remotes",
            Self::Actions => "actions",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "worktrees" => Some(Self::Worktrees),
            "branches" => Some(Self::Branches),
            "remotes" => Some(Self::Remotes),
            "actions" => Some(Self::Actions),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one collapse flag per section, mirroring the section enum"
)]
struct SidebarSections {
    worktrees: bool,
    branches: bool,
    remotes: bool,
    actions: bool,
    /// Render order, user-adjustable by dragging section headers.
    order: [SidebarSection; 4],
}

/// The payload carried while a section header is dragged.
#[derive(Clone)]
struct SectionDrag(SidebarSection);

/// The floating chip shown under the pointer while dragging a section.
struct SectionDragPreview {
    title: &'static str,
    theme: Theme,
}

impl Render for SectionDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .px(px(10.0))
            .py(px(4.0))
            .rounded(px(5.0))
            .border_1()
            .border_color(self.theme.accent)
            .bg(self.theme.bg_selected)
            .text_size(px(10.0))
            .font_weight(FontWeight::BOLD)
            .text_color(self.theme.text_primary)
            .child(self.title)
    }
}

/// Forwards the newest fetch progress into shared state; latest wins.
struct LatestSink(Arc<Mutex<Option<OperationProgress>>>);

impl OperationSink for LatestSink {
    fn report(&self, progress: OperationProgress) {
        if let Ok(mut latest) = self.0.lock() {
            *latest = Some(progress);
        }
    }
}

/// Assembled text for the details header (§6.10).
struct DetailLines {
    hash: String,
    subject: String,
    author: String,
    date: Option<String>,
    committer: Option<String>,
    /// Abbreviated hash and comparison choice per parent, commit order.
    parent_choices: Vec<(String, DiffParent)>,
    body: Option<String>,
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

impl Default for SidebarSections {
    fn default() -> Self {
        Self {
            worktrees: true,
            branches: true,
            remotes: true,
            actions: true,
            order: [
                SidebarSection::Worktrees,
                SidebarSection::Branches,
                SidebarSection::Remotes,
                SidebarSection::Actions,
            ],
        }
    }
}

impl SidebarSections {
    fn expanded(self, section: SidebarSection) -> bool {
        match section {
            SidebarSection::Worktrees => self.worktrees,
            SidebarSection::Branches => self.branches,
            SidebarSection::Remotes => self.remotes,
            SidebarSection::Actions => self.actions,
        }
    }
    fn toggle(&mut self, section: SidebarSection) {
        match section {
            SidebarSection::Worktrees => self.worktrees = !self.worktrees,
            SidebarSection::Branches => self.branches = !self.branches,
            SidebarSection::Remotes => self.remotes = !self.remotes,
            SidebarSection::Actions => self.actions = !self.actions,
        }
    }
    /// The sections in their current display order.
    fn ordered(self) -> [SidebarSection; 4] {
        self.order
    }
    /// Applies persisted order and collapse state, ignoring anything that
    /// no longer names a section. Sections this build knows but the file
    /// predates keep their default position at the end.
    fn apply(&mut self, state: &crate::ui_state::UiState) {
        if let Some(saved) = &state.section_order {
            let mut mapped: Vec<SidebarSection> = Vec::new();
            for name in saved {
                if let Some(section) = SidebarSection::from_name(name)
                    && !mapped.contains(&section)
                {
                    mapped.push(section);
                }
            }
            for section in self.order {
                if !mapped.contains(&section) {
                    mapped.push(section);
                }
            }
            if let Ok(order) = <[SidebarSection; 4]>::try_from(mapped) {
                self.order = order;
            }
        }
        for name in &state.collapsed_sections {
            match SidebarSection::from_name(name) {
                Some(SidebarSection::Worktrees) => self.worktrees = false,
                Some(SidebarSection::Branches) => self.branches = false,
                Some(SidebarSection::Remotes) => self.remotes = false,
                Some(SidebarSection::Actions) => self.actions = false,
                None => {}
            }
        }
    }

    /// The names persisted for the collapse state.
    fn collapsed_names(self) -> Vec<String> {
        self.order
            .iter()
            .filter(|&&section| !self.expanded(section))
            .map(|section| section.name().to_owned())
            .collect()
    }

    /// Moves `moved` to `target`'s position, shifting the rest along.
    fn reorder(&mut self, moved: SidebarSection, target: SidebarSection) {
        if moved == target {
            return;
        }
        let mut order: Vec<SidebarSection> =
            self.order.iter().copied().filter(|&s| s != moved).collect();
        let Some(position) = order.iter().position(|&s| s == target) else {
            return;
        };
        // Dropping below the removal point reads as "after the target".
        let from = self
            .order
            .iter()
            .position(|&s| s == moved)
            .unwrap_or_default();
        let to = self
            .order
            .iter()
            .position(|&s| s == target)
            .unwrap_or_default();
        order.insert(if from < to { position + 1 } else { position }, moved);
        if let Ok(order) = <[SidebarSection; 4]>::try_from(order) {
            self.order = order;
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

/// Divergence text for a branch, or `None` when there is nothing to claim.
///
/// A branch with no upstream shows nothing rather than a zero, and unrelated
/// histories show a marker rather than a fabricated count (§6.7).
fn ahead_behind_text(state: AheadBehindState) -> Option<String> {
    match state {
        AheadBehindState::Known {
            ahead: 0,
            behind: 0,
        }
        | AheadBehindState::Unavailable
        | AheadBehindState::Pending
        | AheadBehindState::Failed => None,
        AheadBehindState::Known { ahead, behind } => Some(match (ahead, behind) {
            (0, behind) => format!("-{behind}"),
            (ahead, 0) => format!("+{ahead}"),
            (ahead, behind) => format!("+{ahead} -{behind}"),
        }),
        AheadBehindState::Unrelated => Some(String::from("↕")),
    }
}

/// Short description of a worktree's HEAD for the sidebar's second line.
fn head_label(head: &HeadSnapshot) -> String {
    match head {
        HeadSnapshot::Branch { short_name, .. } => short_name.clone(),
        HeadSnapshot::Detached { oid } => oid.abbreviated(7),
        HeadSnapshot::Unborn { .. } => String::from("unborn"),
        HeadSnapshot::Missing => String::from("no HEAD"),
    }
}

/// The status bar's right-hand summary of what the snapshot contains.
fn status_summary(snapshot: &RepoSnapshot) -> String {
    let remote_branches: usize = snapshot
        .remotes
        .iter()
        .map(|remote| remote.branches.len())
        .sum();
    format!(
        "{} / {} / {} / {} / {}",
        head_label(&snapshot.head),
        counted(snapshot.worktrees.len(), "worktree"),
        counted(snapshot.local_branches.len(), "branch"),
        counted(remote_branches, "remote branch"),
        counted(snapshot.tags_count, "tag")
    )
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

fn disclosure(theme: &Theme, expanded: bool) -> Div {
    let path = if expanded {
        "icons/chevron-down.svg"
    } else {
        "icons/chevron-right.svg"
    };
    div()
        .size(px(12.0))
        .flex()
        .items_center()
        .justify_center()
        .child(svg().path(path).size(px(10.0)).text_color(theme.text_faint))
}

fn filter_icon(theme: &Theme) -> gpui::Svg {
    svg()
        .path("icons/search.svg")
        .size(px(13.0))
        .text_color(theme.text_faint)
}

fn branch_marker(theme: &Theme) -> gpui::Svg {
    svg()
        .path("icons/git-branch.svg")
        .size(px(12.0))
        .text_color(theme.text_faint)
}

fn remote_marker(theme: &Theme) -> gpui::Svg {
    svg()
        .path("icons/globe.svg")
        .size(px(12.0))
        .text_color(theme.orange)
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

/// Color of a graph line, wrapping when lanes exceed the palette.
fn lane_color(theme: &Theme, color: u8) -> gpui::Hsla {
    theme.graph_lanes[usize::from(color) % theme.graph_lanes.len()]
}

/// Paints every visible row's graph into one element per frame (§7.4).
///
/// The overlay shares the list's scroll offset, so the same frame that moves
/// the rows moves their lines.
fn paint_graph(
    bounds: gpui::Bounds<gpui::Pixels>,
    history: &HistoryState,
    scroll: &UniformListScrollHandle,
    theme: &Theme,
    window: &mut Window,
) {
    let viewport = bounds.size.height.0;
    let content = row_count_as_f32(history.visible_len()) * HISTORY_ROW_HEIGHT;
    // Mirror the list's own clamp so rubber-band overscroll cannot shear the
    // graph away from the rows it annotates.
    let scroll_top =
        (-scroll.0.borrow().base_handle.offset().y.0).clamp(0.0, (content - viewport).max(0.0));
    let first = usize_from_f32((scroll_top / HISTORY_ROW_HEIGHT).floor());
    let last = history
        .visible_len()
        .min(first + usize_from_f32((viewport / HISTORY_ROW_HEIGHT).ceil()) + 1);
    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
        for index in first..last {
            let Some(graph) = history.layout_at(index) else {
                break;
            };
            let is_head = history
                .row_at(index)
                .is_some_and(|row| row.labels.iter().any(|label| label.is_head));
            let row_top = row_count_as_f32(index) * HISTORY_ROW_HEIGHT - scroll_top;
            for shape in row_shapes(row_top, HISTORY_ROW_HEIGHT, graph, is_head) {
                paint_shape(bounds.origin, shape, theme, window);
            }
        }
    });
}

fn paint_shape(
    origin: gpui::Point<gpui::Pixels>,
    shape: Shape,
    theme: &Theme,
    window: &mut Window,
) {
    match shape {
        Shape::Line {
            x,
            top,
            bottom,
            color,
        } => window.paint_quad(gpui::fill(
            gpui::Bounds::new(
                gpui::point(origin.x + px(x - STROKE_WIDTH / 2.0), origin.y + px(top)),
                gpui::size(px(STROKE_WIDTH), px(bottom - top)),
            ),
            lane_color(theme, color),
        )),
        Shape::Curve { from, to, color } => {
            let start = gpui::point(origin.x + px(from.0), origin.y + px(from.1));
            let end = gpui::point(origin.x + px(to.0), origin.y + px(to.1));
            // Control points directly below/above the endpoints give the §7.4
            // vertical tangents.
            let middle = origin.y + px(f32::midpoint(from.1, to.1));
            let mut path = gpui::PathBuilder::stroke(px(STROKE_WIDTH));
            path.move_to(start);
            path.cubic_bezier_to(
                end,
                gpui::point(start.x, middle),
                gpui::point(end.x, middle),
            );
            if let Ok(path) = path.build() {
                window.paint_path(path, lane_color(theme, color));
            }
        }
        Shape::Node { x, y, color, halo } => {
            let color = lane_color(theme, color);
            window.paint_quad(
                gpui::fill(circle(origin, x, y, NODE_RADIUS), color).corner_radii(px(NODE_RADIUS)),
            );
            if halo {
                window.paint_quad(
                    gpui::outline(
                        circle(origin, x, y, HALO_RADIUS),
                        color.opacity(HALO_OPACITY),
                    )
                    .corner_radii(px(HALO_RADIUS)),
                );
            }
        }
    }
}

/// Square bounds of radius `radius` centered on a graph-column point.
fn circle(
    origin: gpui::Point<gpui::Pixels>,
    x: f32,
    y: f32,
    radius: f32,
) -> gpui::Bounds<gpui::Pixels> {
    gpui::Bounds::new(
        gpui::point(origin.x + px(x - radius), origin.y + px(y - radius)),
        gpui::size(px(radius * 2.0), px(radius * 2.0)),
    )
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
    pub(crate) fn new(launch: WindowLaunch, cx: &mut gpui::Context<Self>) -> Self {
        let session = RepoSessionId::new();
        let generation = Generation(0);
        let mut window = Self {
            name: launch.name,
            path: launch.path,
            demo: launch.demo,
            session,
            generation,
            location: None,
            repo: LoadState::Idle,
            history: HistoryState::default(),
            cursor: None,
            theme: Theme::dark(),
            sections: SidebarSections::default(),
            panels: PanelSizes::default(),
            dragging: None,
            detail: None,
            files: None,
            files_for: None,
            files_request: 0,
            diff_view: None,
            diff_request: 0,
            diff_focus: cx.focus_handle(),
            diff_scroll: UniformListScrollHandle::new(),
            scrubbing: Scrub::None,
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
            github_runs: None,
            github_runs_request: 0,
            token_input: Self::masked_token_input(cx),
            juxtapose_bounds: std::rc::Rc::default(),
            preferred_diff_mode: DiffMode::Unified,
            compare_parent: DiffParent::FirstParent,
            details_collapsed: false,
            details_focus: cx.focus_handle(),
            fetching: None,
            fetch_cancel: None,
            fetch_status: None,
            branch_dialog: None,
            branch_input: cx
                .new(|cx| crate::text_input::TextInput::new("new-branch-name", &Theme::dark(), cx)),
            list_scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            filter_input: cx
                .new(|cx| crate::text_input::TextInput::new("Filter commits", &Theme::dark(), cx)),
        };
        // The input owns the text; the window derives the filtered view.
        cx.observe(&window.filter_input, |this, input, cx| {
            let text = input.read(cx).content.to_string();
            if this.history.filter != text {
                this.history.set_filter(&text);
                cx.notify();
            }
        })
        .detach();
        let state = crate::ui_state::UiState::load();
        window.sections.apply(&state);
        window.panels.apply(&state);
        window.preferred_diff_mode = DiffMode::from_name(state.diff_mode.as_deref());
        window.details_collapsed = state.details_collapsed;
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
            // The fixture is seeded as if a traversal had already completed, so
            // every capture goes through the real rendering path (§12.4).
            window.repo = LoadState::Ready(demo::snapshot());
            window.history.reset(HistoryScope::AllRefs);
            let (rows, layout) = demo::history();
            window.history.extend(rows, layout, false);
            window.detail = Some(demo::detail());
            window.files = Some(demo::files());
            window.files_for = window.history.selected;
            window.seed_scene(launch.scene);
        } else if let Some(location) = launch.location {
            window.repo = LoadState::Loading {
                started_at: std::time::Instant::now(),
            };
            window.location = Some(location.clone());
            window.load_metadata(location.clone(), cx);
            Self::watch_metadata(location, cx);
        }
        window
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
                let reload = this.update(cx, |this, cx| {
                    // A new generation retires every result still in flight.
                    this.generation = Generation(this.generation.0 + 1);
                    if let Some(current) = this.repo.value().cloned() {
                        this.repo = LoadState::Refreshing {
                            current,
                            started_at: std::time::Instant::now(),
                        };
                    }
                    cx.notify();
                    this.location.clone()
                });
                match reload {
                    Ok(Some(location)) => {
                        this.update(cx, |this, cx| this.load_metadata(location, cx))
                            .ok();
                    }
                    Ok(None) => {}
                    // The window is gone, so the watcher has nothing to serve.
                    Err(_) => return,
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
        if let Some(index) = self.history.move_selection(delta) {
            self.list_scroll
                .scroll_to_item(index, gpui::ScrollStrategy::Top);
            self.load_selected_files(cx);
            cx.notify();
        }
    }

    /// Selects one loaded row by index and keeps it visible.
    fn select_row(&mut self, index: usize, cx: &mut gpui::Context<Self>) {
        if let Some(index) = self.history.select_index(index) {
            self.list_scroll
                .scroll_to_item(index, gpui::ScrollStrategy::Top);
            self.load_selected_files(cx);
            cx.notify();
        }
    }

    /// Loads the selected commit's changed files after a short debounce.
    ///
    /// §6.10: rapid arrow-key travel must not decode every passed commit, so
    /// the read starts only if the selection still stands after ~50ms. A
    /// result is dropped when the selection or generation moved on.
    fn load_selected_files(&mut self, cx: &mut gpui::Context<Self>) {
        // Checks follow the selection with their own cache and TTL.
        self.load_selected_checks(false, cx);
        let Some(oid) = self.history.selected else {
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

    fn titlebar(&self) -> impl IntoElement {
        let repository = &self.name;
        let path = &self.path;
        div()
            .h(px(TITLEBAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .border_b_1()
            .border_color(self.theme.border)
            .bg(self.theme.bg_chrome)
            .text_size(px(12.5))
            .text_color(self.theme.text_secondary)
            .child(format!("sourcefour - {repository} - {path}"))
    }

    fn toolbar(&self, window: &Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let action = |name: &'static str, icon: &'static str, planned: bool| {
            div()
                .h_full()
                .flex()
                .flex_col()
                .justify_center()
                .items_center()
                .gap(px(3.0))
                .w(px(58.0))
                .text_size(px(10.0))
                .text_color(if planned {
                    self.theme.text_faint
                } else {
                    self.theme.text_secondary
                })
                .hover(|this| this.bg(self.theme.bg_hover))
                .child(svg().path(icon).size(px(15.0)).text_color(if planned {
                    self.theme.text_faint
                } else {
                    self.theme.accent
                }))
                .child(name)
        };
        div()
            .h(px(TOOLBAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(10.0))
            .border_b_1()
            .border_color(self.theme.border)
            .bg(self.theme.bg_chrome)
            .child(self.fetch_button(cx))
            .child(action("Pull", "icons/arrow-down-to-line.svg", true))
            .child(action("Push", "icons/arrow-up-from-line.svg", true))
            .child(action("Commit", "icons/git-commit-horizontal.svg", true))
            .child(
                div()
                    .id("branch-action")
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .items_center()
                    .gap(px(3.0))
                    .w(px(58.0))
                    .text_size(px(10.0))
                    .text_color(self.theme.text_secondary)
                    .cursor_pointer()
                    .hover(|style| style.bg(self.theme.bg_hover))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_branch_dialog(window, cx);
                    }))
                    .child(
                        svg()
                            .path("icons/git-branch.svg")
                            .size(px(15.0))
                            .text_color(self.theme.accent),
                    )
                    .child("Branch"),
            )
            .child(action("Merge", "icons/git-merge.svg", true))
            .child(action("Stash", "icons/archive.svg", true))
            .child(div().flex_grow())
            .child(self.filter_box(window, cx))
            .child(crate::settings_ui::toolbar_button(&self.theme, cx))
    }

    /// The toolbar's Fetch action: live, and disabled while one runs (§6.12).
    fn fetch_button(&self, cx: &mut gpui::Context<Self>) -> gpui::Stateful<Div> {
        let running = self.fetching.is_some();
        div()
            .id("fetch")
            .h_full()
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .gap(px(3.0))
            .w(px(58.0))
            .text_size(px(10.0))
            .text_color(if running {
                self.theme.text_faint
            } else {
                self.theme.text_secondary
            })
            .when(!running, |this| {
                this.cursor_pointer()
                    .hover(|style| style.bg(self.theme.bg_hover))
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.start_fetch(cx);
            }))
            .child(
                svg()
                    .path("icons/cloud-download.svg")
                    .size(px(15.0))
                    .text_color(if running {
                        self.theme.text_faint
                    } else {
                        self.theme.accent
                    }),
            )
            .child(if running { "Fetching" } else { "Fetch" })
    }

    /// The §4.7 filter field, wrapping the real text input.
    fn filter_box(
        &self,
        window: &Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let focused = self.filter_input.read(cx).focus_handle.is_focused(window);
        let matches = self
            .history
            .is_filtering()
            .then(|| self.history.visible_len().to_string());
        div()
            .id("filter")
            .on_click(cx.listener(|this, _, window, cx| {
                this.filter_input
                    .read(cx)
                    .focus_handle
                    .clone()
                    .focus(window);
                cx.notify();
            }))
            .w(px(260.0))
            .h(px(28.0))
            .flex()
            .items_center()
            .px(px(10.0))
            .gap(px(7.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(if focused {
                self.theme.accent
            } else {
                self.theme.border_strong
            })
            .bg(self.theme.bg_list)
            .text_size(px(12.0))
            .text_color(self.theme.text_primary)
            .child(filter_icon(&self.theme))
            .child(self.filter_input.clone())
            .children(matches.map(|matches| {
                div()
                    .flex_none()
                    .text_color(self.theme.accent)
                    .child(matches)
            }))
            .child(
                div()
                    .flex_none()
                    .text_size(px(9.0))
                    .text_color(self.theme.text_faint.opacity(0.7))
                    .child(if focused { "Esc" } else { "Cmd+F" }),
            )
    }

    fn section(
        &self,
        title: &'static str,
        count: impl Into<gpui::SharedString>,
        section: SidebarSection,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let count = count.into();
        let expanded = self.sections.expanded(section);
        let theme = self.theme;
        let accent = self.theme.accent;
        div()
            .id(title)
            .h(px(27.0))
            .flex()
            .items_end()
            .justify_between()
            .px(px(14.0))
            .pb(px(4.0))
            // A subtle divider between the sidebar's sections; the first sits
            // under the toolbar's own border and needs none.
            .when(self.sections.ordered()[0] != section, |this| {
                this.border_t_1().border_color(self.theme.border)
            })
            .text_size(px(10.0))
            .font_weight(FontWeight::BOLD)
            .text_color(self.theme.text_faint)
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .child(disclosure(&self.theme, expanded))
                    .child(title),
            )
            .child(count)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.sections.toggle(section);
                this.persist_ui_state(cx);
                cx.notify();
            }))
            // Sections reorder by dragging a header onto another header.
            .on_drag(SectionDrag(section), move |_, _, _, cx| {
                cx.new(|_| SectionDragPreview { title, theme })
            })
            .drag_over::<SectionDrag>(move |style, _, _, _| style.border_t_2().border_color(accent))
            .on_drop(cx.listener(move |this, dragged: &SectionDrag, _, cx| {
                this.sections.reorder(dragged.0, section);
                this.persist_ui_state(cx);
                cx.notify();
            }))
    }

    fn sidebar(&self, cx: &mut gpui::Context<Self>) -> Div {
        match self.snapshot() {
            Some(snapshot) => self.repository_sidebar(snapshot, cx),
            None => self.loading_sidebar(cx),
        }
    }

    /// The sidebar built from real repository metadata, sections in the
    /// user's order.
    fn repository_sidebar(&self, snapshot: &RepoSnapshot, cx: &mut gpui::Context<Self>) -> Div {
        let mut root = div()
            .w(px(self.panels.sidebar))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(self.theme.bg_panel);
        for section in self.sections.ordered() {
            root = match section {
                SidebarSection::Worktrees => root
                    .child(self.section(
                        "WORKTREES",
                        snapshot.worktrees.len().to_string(),
                        section,
                        cx,
                    ))
                    .when(self.sections.worktrees, |this| {
                        this.children(
                            snapshot
                                .worktrees
                                .iter()
                                .map(|tree| self.worktree_row(tree)),
                        )
                    }),
                SidebarSection::Branches => root
                    .child(self.section(
                        "BRANCHES",
                        snapshot.local_branches.len().to_string(),
                        section,
                        cx,
                    ))
                    .when(self.sections.branches, |this| {
                        this.children(
                            snapshot
                                .local_branches
                                .iter()
                                .map(|branch| self.branch_row(branch, cx)),
                        )
                    }),
                // The prototype counts remotes here, not their branches.
                SidebarSection::Remotes => root
                    .child(self.section("REMOTES", snapshot.remotes.len().to_string(), section, cx))
                    .when(self.sections.remotes, |this| {
                        this.children(snapshot.remotes.iter().flat_map(|remote| {
                            std::iter::once(self.remote_row(remote)).chain(
                                remote
                                    .branches
                                    .iter()
                                    .map(|branch| self.remote_branch_row(&branch.short_name)),
                            )
                        }))
                    }),
                // Only a GitHub repository has Actions to show.
                SidebarSection::Actions if self.github_remote.is_none() => root,
                SidebarSection::Actions => {
                    let runs = self
                        .github_runs
                        .as_ref()
                        .map_or(&[][..], |cache| cache.value.as_slice());
                    root.child(self.section("ACTIONS", runs.len().to_string(), section, cx))
                        .when(self.sections.actions, |this| {
                            this.children(
                                runs.iter()
                                    .enumerate()
                                    .map(|(index, run)| self.workflow_run_row(index, run)),
                            )
                        })
                }
            };
        }
        root
    }

    fn worktree_row(&self, tree: &sourcefour_model::WorktreeSnapshot) -> Div {
        let marker = match (&tree.accessibility, tree.is_current) {
            (WorktreeAccessibility::Inaccessible { .. }, _) => self.theme.red,
            (WorktreeAccessibility::Prunable { .. }, _) => self.theme.orange,
            (WorktreeAccessibility::Accessible, true) => self.theme.green,
            (WorktreeAccessibility::Accessible, false) => self.theme.text_faint,
        };
        div()
            .h(px(47.0))
            .flex()
            .flex_col()
            .justify_center()
            .px(px(14.0))
            .bg(if tree.is_current {
                self.theme.bg_selected
            } else {
                self.theme.bg_panel
            })
            .text_color(self.theme.text_primary)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(div().size(px(7.0)).rounded_full().bg(marker))
                            .child(tree.display_name.clone())
                            .when(tree.is_locked, |this| {
                                this.child(
                                    div()
                                        .px(px(3.0))
                                        .rounded(px(2.0))
                                        .border_1()
                                        .border_color(self.theme.text_faint)
                                        .text_size(px(8.0))
                                        .text_color(self.theme.text_faint)
                                        .child("LOCKED"),
                                )
                            }),
                    )
                    .children(tree.is_current.then(|| {
                        div()
                            .h(px(15.0))
                            .px(px(4.0))
                            .flex()
                            .items_center()
                            .rounded(px(3.0))
                            .border_1()
                            .border_color(self.theme.accent.opacity(0.45))
                            .bg(self.theme.accent.opacity(0.12))
                            .text_size(px(8.0))
                            .line_height(px(8.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(self.theme.accent)
                            .child("CURRENT")
                    })),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_size(px(10.0))
                    .text_color(self.theme.text_faint)
                    .child(crate::app::display_path(&tree.path))
                    .child(head_label(&tree.head)),
            )
    }

    fn branch_row(
        &self,
        branch: &sourcefour_model::BranchSnapshot,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        let scoped = is_scoped_to(self.history.scope.as_ref(), &branch.full_name);
        let full_name = branch.full_name.clone();
        let tip = branch.tip;
        div()
            .id(gpui::SharedString::from(branch.full_name.clone()))
            .h(px(26.0))
            .flex()
            .items_center()
            .px(px(15.0))
            .gap(px(7.0))
            .cursor_pointer()
            .hover(|style| style.bg(self.theme.bg_hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                let scope = toggled_scope(this.history.scope.as_ref(), &full_name, tip);
                this.start_history(scope, cx);
                cx.notify();
            }))
            .bg(if scoped || branch.is_current {
                self.theme.bg_selected
            } else {
                self.theme.bg_panel
            })
            .text_size(px(12.0))
            .text_color(if scoped || branch.is_current {
                self.theme.text_primary
            } else {
                self.theme.text_secondary
            })
            .child(branch_marker(&self.theme))
            .child(branch.short_name.clone())
            .children(self.pr_chip(&branch.short_name))
            .child(div().flex_grow())
            .children(ahead_behind_text(branch.ahead_behind).map(|text| {
                div()
                    .text_size(px(10.5))
                    .text_color(self.theme.accent)
                    .child(text)
            }))
    }

    fn remote_row(&self, remote: &sourcefour_model::RemoteSnapshot) -> Div {
        div()
            .h(px(29.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(14.0))
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(self.theme.orange)
            .child(remote_marker(&self.theme))
            .child(remote.name.clone())
            .child(div().flex_grow())
            .children(remote.fetch_url.clone().map(|url| {
                div()
                    .text_size(px(10.0))
                    .text_color(self.theme.text_faint)
                    .child(url)
            }))
    }

    fn remote_branch_row(&self, short_name: &str) -> Div {
        div()
            .h(px(24.0))
            .flex()
            .items_center()
            .pl(px(34.0))
            .text_size(px(12.0))
            .text_color(self.theme.purple)
            .child(short_name.to_owned())
    }

    fn loading_sidebar(&self, cx: &mut gpui::Context<Self>) -> Div {
        let loading = || {
            div()
                .h(px(32.0))
                .flex()
                .items_center()
                .px(px(14.0))
                .text_size(px(11.0))
                .text_color(self.theme.text_faint)
                .child("Loading repository metadata...")
        };
        let mut root = div()
            .w(px(self.panels.sidebar))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(self.theme.bg_panel);
        for section in self.sections.ordered() {
            let title = match section {
                SidebarSection::Worktrees => "WORKTREES",
                SidebarSection::Branches => "BRANCHES",
                SidebarSection::Remotes => "REMOTES",
                // Nothing to promise while metadata loads: Actions appears
                // only once a GitHub remote resolves.
                SidebarSection::Actions => continue,
            };
            root = root
                .child(self.section(title, "", section, cx))
                .when(self.sections.expanded(section), |this| {
                    this.child(loading())
                });
        }
        root
    }

    fn header(&self, columns: ColumnVisibility) -> impl IntoElement {
        div()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(self.theme.border)
            .bg(self.theme.bg_panel)
            .text_size(px(10.0))
            .font_weight(FontWeight::BOLD)
            .text_color(self.theme.text_faint)
            .child(div().w(px(self.panels.graph)).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w(px(1.0))
                    .pl(px(10.0))
                    .child("DESCRIPTION"),
            )
            .when(columns.author, |this| {
                this.child(div().w(px(148.0)).flex_none().child("AUTHOR"))
            })
            .child(div().w(px(96.0)).flex_none().child("DATE"))
            .when(columns.hash, |this| {
                this.child(div().w(px(74.0)).flex_none().child("HASH"))
            })
    }

    fn history(&self, columns: ColumnVisibility, cx: &mut gpui::Context<Self>) -> Div {
        div()
            .relative()
            .flex_grow()
            .min_h(px(1.0))
            .child(self.history_list(columns, cx))
            .children(self.graph_overlay(cx))
            // The graph divider floats over the list so it costs no layout.
            .child(
                self.splitter(Splitter::Graph, cx)
                    .absolute()
                    .top_0()
                    .left(px(self.panels.graph - SPLITTER_WIDTH)),
            )
    }

    /// Starts a fetch of every remote through the user's own Git (§6.12).
    ///
    /// A second fetch while one runs is a no-op: the button disables, and
    /// this guard holds even if a keybinding races the render.
    fn start_fetch(&mut self, cx: &mut gpui::Context<Self>) {
        if self.fetching.is_some() {
            return;
        }
        let Some(location) = self.location.clone() else {
            return;
        };
        let request = FetchRequest {
            worktree: self
                .snapshot()
                .and_then(|snapshot| snapshot.active_worktree.clone())
                .unwrap_or_else(|| sourcefour_model::WorktreeId(String::from("active"))),
            remote: None,
            prune: false,
        };
        let latest = Arc::new(Mutex::new(None));
        let cancel = Arc::new(AtomicBool::new(false));
        self.fetching = Some(Arc::clone(&latest));
        self.fetch_cancel = Some(Arc::clone(&cancel));
        self.fetch_status = None;
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    sourcefour_git::fetch(&location, &request, &LatestSink(latest), &cancel)
                })
                .await;
            this.update(cx, |this, cx| {
                this.fetching = None;
                this.fetch_cancel = None;
                match outcome {
                    Ok(OperationOutcome::Succeeded { summary, .. }) => {
                        this.fetch_status = Some((true, summary));
                        // Refresh immediately rather than waiting for the
                        // watcher's next poll (§6.12).
                        this.generation = Generation(this.generation.0 + 1);
                        if let Some(current) = this.repo.value().cloned() {
                            this.repo = LoadState::Refreshing {
                                current,
                                started_at: std::time::Instant::now(),
                            };
                        }
                        if let Some(location) = this.location.clone() {
                            this.load_metadata(location, cx);
                        }
                    }
                    Ok(OperationOutcome::Cancelled { .. }) => {
                        this.fetch_status = Some((true, String::from("Fetch cancelled")));
                    }
                    Ok(OperationOutcome::Failed { error, .. }) => {
                        this.fetch_status = Some((false, error.user.message));
                    }
                    Err(failure) => {
                        this.fetch_status = Some((false, failure.user.message));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        // Repaint on a short tick while the fetch runs so progress shows.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                let done = this
                    .update(cx, |this, cx| {
                        if this.fetching.is_none() {
                            true
                        } else {
                            cx.notify();
                            false
                        }
                    })
                    .unwrap_or(true);
                if done {
                    return;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Saves panel sizes, section order, and collapse state off-thread.
    fn persist_ui_state(&self, cx: &gpui::Context<Self>) {
        let state = crate::ui_state::UiState {
            sidebar_width: Some(self.panels.sidebar),
            graph_width: Some(self.panels.graph),
            details_height: Some(self.panels.details),
            section_order: Some(
                self.sections
                    .ordered()
                    .iter()
                    .map(|section| section.name().to_owned())
                    .collect(),
            ),
            collapsed_sections: self.sections.collapsed_names(),
            diff_mode: Some(self.preferred_diff_mode.name().to_owned()),
            details_collapsed: self.details_collapsed,
        };
        cx.background_executor()
            .spawn(async move { state.save() })
            .detach();
    }

    /// A draggable divider; the window's mouse handlers do the actual moving.
    fn splitter(&self, splitter: Splitter, cx: &mut gpui::Context<Self>) -> Div {
        let vertical = matches!(splitter, Splitter::Sidebar | Splitter::Graph);
        let dragging = self.dragging == Some(splitter);
        let base = div()
            .flex_none()
            // The graph divider floats over content, so it only shows itself
            // when interacted with; the panel dividers read as borders.
            .bg(if dragging {
                self.theme.accent.opacity(0.55)
            } else if matches!(splitter, Splitter::Graph) {
                gpui::transparent_black()
            } else {
                self.theme.border
            })
            .hover(|style| style.bg(self.theme.accent.opacity(0.35)))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.dragging = Some(splitter);
                    cx.notify();
                }),
            );
        if vertical {
            base.w(px(SPLITTER_WIDTH))
                .h_full()
                .cursor(gpui::CursorStyle::ResizeLeftRight)
        } else {
            base.h(px(SPLITTER_WIDTH))
                .w_full()
                .cursor(gpui::CursorStyle::ResizeUpDown)
        }
    }

    /// The single absolute canvas painting all visible graph rows (§8.4).
    ///
    /// It carries no listeners, so clicks fall through to the rows beneath it.
    fn graph_overlay(&self, cx: &gpui::Context<Self>) -> Option<impl IntoElement + use<>> {
        if self.history.visible_len() == 0 {
            return None;
        }
        let entity = cx.entity();
        let scroll = self.list_scroll.clone();
        let theme = self.theme;
        Some(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, (), window, cx| {
                    paint_graph(bounds, &entity.read(cx).history, &scroll, &theme, window);
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .w(px(self.panels.graph))
            .h_full(),
        )
    }

    /// The virtualized commit list: one element per visible row only (§8.3).
    fn history_list(&self, columns: ColumnVisibility, cx: &mut gpui::Context<Self>) -> Div {
        if self.history.visible_len() == 0 {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(self.theme.bg_list)
                .text_size(px(12.0))
                .text_color(self.theme.text_faint)
                .child(if self.history.has_more {
                    format!("Loading history from {}...", self.path)
                } else {
                    String::from("No commits yet")
                });
        }
        let now = self.now_seconds();
        div().size_full().bg(self.theme.bg_list).child(
            uniform_list(
                cx.entity(),
                "history",
                self.history.visible_len(),
                move |this, visible, _window, cx| {
                    // Scrolling near the tail is what asks for the next batch.
                    if this.history.wants_more(visible.end) {
                        this.request_batch(cx);
                    }
                    visible
                        .clone()
                        .map(|index| this.commit_row(index, columns, now, cx))
                        .collect()
                },
            )
            .track_scroll(self.list_scroll.clone())
            .size_full(),
        )
    }

    /// One history row: graph cell, subject, author, date, hash.
    fn commit_row(
        &self,
        index: usize,
        columns: ColumnVisibility,
        now: i64,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        let Some(row) = self.history.row_at(index) else {
            return div()
                .id(("commit-missing", index))
                .h(px(HISTORY_ROW_HEIGHT));
        };
        let selected = self.history.selected == Some(row.oid);
        let oid = row.oid;
        div()
            .id(("commit", index))
            .h(px(HISTORY_ROW_HEIGHT))
            // Without a full-width row the subject sizes to its text and every
            // later column drifts, so the header no longer lines up with it.
            .w_full()
            .flex()
            .items_center()
            .bg(if selected {
                self.theme.bg_selected
            } else {
                self.theme.bg_list
            })
            .hover(|style| style.bg(self.theme.bg_hover))
            .text_color(self.theme.text_primary)
            // The graph column is reserved per row but painted by the overlay.
            .child(div().w(px(self.panels.graph)).h_full().flex_none())
            .child(
                div()
                    // flex-basis 0: a long subject must never widen this cell
                    // and push the fixed columns out of the header's alignment.
                    .flex_1()
                    .min_w(px(1.0))
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .pl(px(10.0))
                    .pr(px(10.0))
                    // Ref labels for this commit, capped so a tag pile-up
                    // cannot push the subject out of the row (§6.6).
                    .children(
                        row.labels
                            .iter()
                            .take(3)
                            .map(|label| self.label_chip(label)),
                    )
                    .children((row.labels.len() > 3).then(|| {
                        div()
                            .flex_none()
                            .text_size(px(9.5))
                            .text_color(self.theme.text_faint)
                            .child(format!("+{}", row.labels.len() - 3))
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(1.0))
                            // Without clipping, a long subject runs straight
                            // through the author column instead of stopping.
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(12.5))
                            .text_color(if row.flags.is_merge {
                                self.theme.text_secondary
                            } else {
                                self.theme.text_primary
                            })
                            .child(row.summary.clone()),
                    ),
            )
            .when(columns.author, |this| {
                this.child(
                    div()
                        .w(px(148.0))
                        .flex_none()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(11.5))
                        .text_color(self.theme.text_secondary)
                        .child(row.author_name.clone()),
                )
            })
            .child(
                div()
                    .w(px(96.0))
                    .flex_none()
                    .text_size(px(11.5))
                    .text_color(self.theme.text_secondary)
                    .child(relative_date(now, row.commit_time)),
            )
            .when(columns.hash, |this| {
                this.child(
                    div()
                        .w(px(74.0))
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(if selected {
                            self.theme.accent
                        } else {
                            self.theme.text_faint
                        })
                        .child(oid.abbreviated(7)),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.history.selected = Some(oid);
                this.load_selected_files(cx);
                cx.notify();
            }))
    }

    /// One ref label chip: HEAD, branch, remote branch, or tag (§6.6).
    fn label_chip(&self, label: &sourcefour_model::RefLabel) -> Div {
        let color = if label.is_head {
            self.theme.accent
        } else {
            match label.kind {
                sourcefour_model::RefKind::Head => self.theme.accent,
                sourcefour_model::RefKind::LocalBranch => self.theme.green,
                sourcefour_model::RefKind::RemoteBranch => self.theme.orange,
                sourcefour_model::RefKind::Tag => self.theme.purple,
                sourcefour_model::RefKind::Other => self.theme.text_faint,
            }
        };
        div()
            .flex_none()
            .px(px(5.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(color.opacity(0.4))
            .bg(color.opacity(0.1))
            .text_size(px(9.5))
            .text_color(color)
            .child(label.name.clone())
    }

    /// The details header text, from the exact metadata when it has arrived
    /// and from the already-loaded row until then (§6.10).
    fn detail_lines(&self) -> DetailLines {
        let now = self.now_seconds();
        let row = self
            .history
            .selected_index()
            .and_then(|index| self.history.rows.get(index));
        let detail = self.detail.as_ref();
        DetailLines {
            hash: row.map_or_else(String::new, |row| row.oid.abbreviated(9)),
            subject: detail.map_or_else(
                || {
                    row.map_or_else(
                        || String::from("No commit selected"),
                        |row| row.summary.clone(),
                    )
                },
                |detail| detail.subject.clone(),
            ),
            author: detail.map_or_else(
                || row.map_or_else(String::new, |row| row.author_name.clone()),
                |detail| format!("{} <{}>", detail.author.name, detail.author.email),
            ),
            date: detail
                .map(|detail| detail.author.time)
                .or_else(|| row.map(|row| row.commit_time))
                .map(|time| relative_date(now, time)),
            committer: detail
                .filter(|detail| {
                    detail.committer.name != detail.author.name
                        || detail.committer.email != detail.author.email
                })
                .map(|detail| {
                    format!(
                        "committed by {} {}",
                        detail.committer.name,
                        relative_date(now, detail.committer.time)
                    )
                }),
            parent_choices: detail.map_or_else(Vec::new, |detail| {
                detail
                    .parents
                    .iter()
                    .enumerate()
                    .map(|(index, parent)| {
                        let choice = if index == 0 {
                            DiffParent::FirstParent
                        } else {
                            DiffParent::Parent(*parent)
                        };
                        (parent.abbreviated(7), choice)
                    })
                    .collect()
            }),
            body: detail
                .map(|detail| detail.body.clone())
                .filter(|body| !body.is_empty()),
        }
    }

    /// The details pane's first row: hash, and parent hashes — clickable
    /// comparison choices when the commit is a merge (§6.10).
    fn details_hash_row(
        &self,
        hash: &str,
        parent_choices: &[(String, DiffParent)],
        cx: &mut gpui::Context<Self>,
    ) -> Div {
        let comparing = self.compare_parent;
        let hash = hash.to_owned();

        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .text_size(px(12.0))
            .font_family(MONO_FONT)
            .text_color(self.theme.accent)
            .child(hash)
            .children((parent_choices.len() == 1).then(|| {
                div()
                    .text_size(px(10.5))
                    .text_color(self.theme.text_faint)
                    .child(format!("Parent  {}", parent_choices[0].0))
            }))
            .children((parent_choices.len() > 1).then(|| {
                // A merge: pick which parent to compare against.
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(px(10.5))
                    .text_color(self.theme.text_faint)
                    .child("vs")
                    .children(
                        parent_choices
                            .iter()
                            .enumerate()
                            .map(|(index, (hash, choice))| {
                                let choice = *choice;
                                let selected = comparing == choice;
                                div()
                                    .id(("parent-choice", index))
                                    .px(px(6.0))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .cursor_pointer()
                                    .border_color(if selected {
                                        self.theme.accent
                                    } else {
                                        self.theme.border_strong
                                    })
                                    .text_color(if selected {
                                        self.theme.accent
                                    } else {
                                        self.theme.text_secondary
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_compare_parent(choice, cx);
                                    }))
                                    .child(hash.clone())
                            }),
                    )
            }))
    }

    /// §4.6: the collapsed details strip; Space or a click expands it.
    fn collapsed_details(
        &self,
        hash: &str,
        subject: &str,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let hash = hash.to_owned();
        let subject = subject.to_owned();
        div()
            .id("details-collapsed")
            .h(px(30.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(14.0))
            .border_t_1()
            .border_color(self.theme.border)
            .bg(self.theme.bg_panel)
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.details_collapsed = false;
                this.persist_ui_state(cx);
                cx.notify();
            }))
            .child(
                div()
                    .flex_none()
                    .font_family(MONO_FONT)
                    .text_size(px(11.0))
                    .text_color(self.theme.accent)
                    .child(hash),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(1.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(11.5))
                    .text_color(self.theme.text_secondary)
                    .child(subject),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.0))
                    .text_color(self.theme.text_faint)
                    .child("Space to expand"),
            )
            .into_any_element()
    }

    fn details(&self, cx: &mut gpui::Context<Self>) -> gpui::AnyElement {
        let lines = self.detail_lines();
        if self.details_collapsed {
            return self.collapsed_details(&lines.hash, &lines.subject, cx);
        }
        div()
            .h(px(self.panels.details))
            .flex_none()
            .flex()
            .bg(self.theme.bg_panel)
            .child(self.details_message_column(lines, cx))
            .child(div().w(px(1.0)).flex_none().bg(self.theme.border))
            .child(self.details_files_column(cx))
            .into_any_element()
    }

    /// The left details column: hash row, subject, author line, and the
    /// scrollable commit body.
    fn details_message_column(&self, lines: DetailLines, cx: &mut gpui::Context<Self>) -> Div {
        let DetailLines {
            hash,
            subject,
            author,
            date,
            committer,
            parent_choices,
            body,
        } = lines;
        div()
            .flex_1()
            .min_w(px(1.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(14.0))
            .pt(px(10.0))
            .child(self.details_hash_row(&hash, &parent_choices, cx))
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(self.theme.text_primary)
                    .child(subject),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(11.5))
                    .text_color(self.theme.text_secondary)
                    .child(author)
                    .children(date.map(|date| div().text_color(self.theme.text_faint).child(date)))
                    .children(
                        committer.map(|committer| {
                            div().text_color(self.theme.text_faint).child(committer)
                        }),
                    ),
            )
            .child(
                div()
                    .id("details-scroll")
                    .track_focus(&self.details_focus)
                    .flex_grow()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .children(body.map(|body| {
                        div()
                            .pt(px(4.0))
                            .pb(px(4.0))
                            .text_size(px(11.5))
                            .text_color(self.theme.text_secondary)
                            .child(body)
                    }))
                    .children(self.checks_block(cx)),
            )
    }

    /// The right details column: the changed-files header and scrollable list.
    fn details_files_column(&self, cx: &mut gpui::Context<Self>) -> Div {
        div()
            .flex_1()
            .min_w(px(1.0))
            .flex()
            .flex_col()
            .px(px(14.0))
            .pt(px(10.0))
            .child(
                div()
                    .pb(px(6.0))
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(self.theme.text_faint)
                    .child(self.files.as_ref().map_or_else(
                        || String::from("CHANGED FILES"),
                        |files| format!("CHANGED FILES · {}", counted(files.files.len(), "file")),
                    )),
            )
            .child(
                div()
                    .id("details-files-scroll")
                    .flex_grow()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .pb(px(6.0))
                    .children(
                        self.files
                            .clone()
                            .iter()
                            .flat_map(|files| files.files.clone())
                            .enumerate()
                            .map(|(index, file)| self.file_row(index, &file, cx)),
                    ),
            )
    }

    /// Switches the comparison parent and reloads files for the selection.
    fn set_compare_parent(&mut self, parent: DiffParent, cx: &mut gpui::Context<Self>) {
        if self.compare_parent == parent {
            return;
        }
        self.compare_parent = parent;
        self.files_for = None;
        self.load_selected_files(cx);
        cx.notify();
    }

    /// Opens the overlay a capture scene asks for, with its content already
    /// loaded (§12.4). Demo mode has no repository to read a diff from, so the
    /// content comes from the fixture — through the shipping formatter.
    fn seed_scene(&mut self, scene: demo::Scene) {
        let mode = match scene {
            demo::Scene::Overview => return,
            demo::Scene::Settings => {
                self.settings_view = Some(crate::settings_ui::SettingsSection::default());
                return;
            }
            demo::Scene::Split => DiffMode::Split,
            _ => DiffMode::Unified,
        };
        let mut view = DiffView {
            title: scene.file().to_owned(),
            status: ChangeKind::Modified,
            content: Some(scene.content()),
            mode,
            split: None,
            before_image: None,
            after_image: None,
            slider: 0.5,
        };
        view.ensure_split();
        view.ensure_images();
        self.diff_view = Some(view);
    }

    /// The GitHub section's token field: like every input, but masked.
    fn masked_token_input(
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Entity<crate::text_input::TextInput> {
        cx.new(|cx| {
            let mut input =
                crate::text_input::TextInput::new("ghp_… or github_pat_…", &Theme::dark(), cx);
            input.masked = true;
            input
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

    /// Applies one settings mutation and writes the file immediately, the
    /// same contract as Zed's settings controls.
    pub(crate) fn update_settings(
        &mut self,
        cx: &mut gpui::Context<Self>,
        apply: impl FnOnce(&mut crate::settings::AppSettings),
    ) {
        apply(&mut self.settings);
        self.settings.save();
        // A changed toggle or auth method takes effect without a restart.
        self.refresh_github(cx);
        cx.notify();
    }

    /// The settings overlay, while open.
    fn settings_overlay(&self, cx: &mut gpui::Context<Self>) -> Option<impl IntoElement + use<>> {
        let section = self.settings_view?;
        Some(crate::settings_ui::overlay(
            &self.settings,
            section,
            &self.github_connection,
            &self.token_input,
            &self.theme,
            &self.settings_focus,
            cx,
        ))
    }

    /// One changed file: status letter, path, and line counts when known.
    fn file_row(
        &self,
        index: usize,
        file: &ChangedFile,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        let color = change_color(&self.theme, file.status);
        let path = file
            .new_path
            .as_ref()
            .or(file.old_path.as_ref())
            .map_or_else(String::new, sourcefour_model::RepoPath::display_lossy);
        let clicked = file.clone();
        div()
            .id(("changed-file", index))
            .h(px(22.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_size(px(11.0))
            .cursor_pointer()
            .hover(|style| style.bg(self.theme.bg_hover))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_diff(&clicked, window, cx);
            }))
            .child(
                div()
                    .w(px(12.0))
                    .flex_none()
                    .font_weight(FontWeight::BOLD)
                    .text_color(color)
                    .child(change_letter(file.status)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(1.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(self.theme.text_secondary)
                    .child(path),
            )
            .children(file.additions.map(|added| {
                div()
                    .text_color(self.theme.green)
                    .child(format!("+{added}"))
            }))
            .children(file.deletions.map(|removed| {
                div()
                    .text_color(self.theme.red)
                    .child(format!("-{removed}"))
            }))
    }

    fn status(&self) -> impl IntoElement {
        let path = self.path.clone();
        div()
            .h(px(STATUS_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(14.0))
            .px(px(12.0))
            .border_t_1()
            .border_color(self.theme.border)
            .bg(self.theme.bg_chrome)
            .text_size(px(10.0))
            .text_color(self.theme.text_faint)
            .child(path)
            .children(self.fetching.as_ref().map(|latest| {
                let message = latest
                    .lock()
                    .ok()
                    .and_then(|progress| progress.clone())
                    .map_or_else(|| String::from("Fetching…"), |progress| progress.message);
                div().text_color(self.theme.accent).child(message)
            }))
            .children(self.fetch_status.clone().map(|(ok, message)| {
                div()
                    .text_color(if ok { self.theme.green } else { self.theme.red })
                    .child(message)
            }))
            .children(self.history.is_filtering().then(|| {
                div().text_color(self.theme.accent).child(format!(
                    "Searching loaded history — {} of {} match",
                    self.history.visible_len(),
                    self.history.rows.len()
                ))
            }))
            .child(div().flex_grow())
            .child(self.snapshot().map_or_else(
                || String::from("Loading repository metadata..."),
                status_summary,
            ))
    }
}

impl SourcefourWindow {
    /// Registers every window-level action and drag handler on the root.
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
            if this.branch_dialog.is_some() {
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
            if this.branch_dialog.is_some() {
                this.submit_branch_dialog(window, cx);
            } else {
                this.focus.focus(window);
            }
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &CloseDiff, window, cx| {
            this.close_diff(window, cx);
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
        .map(|root| Self::root_drag_handlers(root, cx))
    }

    /// Window-wide drag handlers: splitter handles and scrub starts only arm
    /// state; the movement happens here, so a fast drag cannot escape a
    /// 3px handle.
    fn root_drag_handlers(root: Div, cx: &mut gpui::Context<Self>) -> Div {
        root.on_mouse_move(
            cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                if let Some(splitter) = this.dragging {
                    this.panels.drag(
                        splitter,
                        event.position.x.0,
                        event.position.y.0,
                        window.viewport_size().height.0,
                    );
                    cx.notify();
                } else {
                    this.scrub_move(event.position.x.0, event.position.y.0, cx);
                }
            }),
        )
        .on_mouse_up(
            gpui::MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                if this.dragging.take().is_some() {
                    this.persist_ui_state(cx);
                    cx.notify();
                }
                this.end_scrub(cx);
            }),
        )
    }
}

impl Render for SourcefourWindow {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let columns = ColumnVisibility::for_available_width(
            window.viewport_size().width.0 - self.panels.sidebar,
        );
        let root = div().key_context("History");
        Self::root_actions(root, cx)
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
    }
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
        AheadBehindState, Generation, HeadSnapshot, Oid, RepoEnvelope, RepoFailure,
        RepoFailureKind, RepoSessionId, RequestId,
    };

    use super::{
        ColumnVisibility, ErrorWindow, SidebarSection, SidebarSections, ahead_behind_text,
        belongs_to, counted, head_label,
    };

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
    fn ahead_behind_shows_a_count_only_once_one_is_known() {
        assert_eq!(ahead_behind_text(AheadBehindState::Unavailable), None);
        assert_eq!(ahead_behind_text(AheadBehindState::Pending), None);
        assert_eq!(ahead_behind_text(AheadBehindState::Failed), None);
        assert_eq!(
            ahead_behind_text(AheadBehindState::Known {
                ahead: 0,
                behind: 0
            }),
            None,
            "a branch level with its upstream needs no indicator"
        );
        assert_eq!(
            ahead_behind_text(AheadBehindState::Known {
                ahead: 2,
                behind: 0
            })
            .as_deref(),
            Some("+2")
        );
        assert_eq!(
            ahead_behind_text(AheadBehindState::Known {
                ahead: 0,
                behind: 3
            })
            .as_deref(),
            Some("-3")
        );
        assert_eq!(
            ahead_behind_text(AheadBehindState::Known {
                ahead: 2,
                behind: 3
            })
            .as_deref(),
            Some("+2 -3")
        );
        assert_eq!(
            ahead_behind_text(AheadBehindState::Unrelated).as_deref(),
            Some("↕"),
            "unrelated histories must not show a fabricated count"
        );
    }

    #[test]
    fn head_label_describes_every_head_state() {
        assert_eq!(
            head_label(&HeadSnapshot::Branch {
                full_name: String::from("refs/heads/main"),
                short_name: String::from("main"),
                oid: Oid::sha1([0; 20]),
            }),
            "main"
        );
        assert_eq!(
            head_label(&HeadSnapshot::Detached {
                oid: Oid::sha1([0xab; 20])
            }),
            "abababa",
            "a detached HEAD shows an abbreviated hash"
        );
        assert_eq!(
            head_label(&HeadSnapshot::Unborn {
                intended_branch: Some(String::from("main"))
            }),
            "unborn"
        );
        assert_eq!(head_label(&HeadSnapshot::Missing), "no HEAD");
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
    fn sidebar_sections_start_expanded_and_toggle_independently() {
        let mut sections = SidebarSections::default();
        assert!(sections.expanded(SidebarSection::Worktrees));
        assert!(sections.expanded(SidebarSection::Branches));
        assert!(sections.expanded(SidebarSection::Remotes));
        sections.toggle(SidebarSection::Branches);
        assert!(sections.expanded(SidebarSection::Worktrees));
        assert!(!sections.expanded(SidebarSection::Branches));
        assert!(sections.expanded(SidebarSection::Remotes));
    }

    #[test]
    fn persisted_section_state_round_trips() {
        use SidebarSection::{Branches, Remotes, Worktrees};
        let mut sections = SidebarSections::default();
        sections.reorder(Remotes, Worktrees);
        sections.toggle(Branches);

        let state = crate::ui_state::UiState {
            section_order: Some(
                sections
                    .ordered()
                    .iter()
                    .map(|section| section.name().to_owned())
                    .collect(),
            ),
            collapsed_sections: sections.collapsed_names(),
            ..Default::default()
        };
        let mut restored = SidebarSections::default();
        restored.apply(&state);

        assert_eq!(restored, sections);

        // Garbage in the state file must not corrupt the layout.
        let mut untouched = SidebarSections::default();
        untouched.apply(&crate::ui_state::UiState {
            section_order: Some(vec![String::from("worktrees"), String::from("worktrees")]),
            collapsed_sections: vec![String::from("hologram")],
            ..Default::default()
        });
        assert_eq!(untouched, SidebarSections::default());
    }

    #[test]
    fn sidebar_sections_reorder_by_dropping_onto_a_target() {
        use SidebarSection::{Actions, Branches, Remotes, Worktrees};
        let mut sections = SidebarSections::default();
        assert_eq!(sections.ordered(), [Worktrees, Branches, Remotes, Actions]);

        // Dragging a section onto another takes that section's position.
        sections.reorder(Remotes, Worktrees);
        assert_eq!(sections.ordered(), [Remotes, Worktrees, Branches, Actions]);

        sections.reorder(Remotes, Branches);
        assert_eq!(sections.ordered(), [Worktrees, Branches, Remotes, Actions]);

        // Dropping a section onto itself changes nothing.
        sections.reorder(Branches, Branches);
        assert_eq!(sections.ordered(), [Worktrees, Branches, Remotes, Actions]);
    }

    #[test]
    fn a_section_order_predating_actions_keeps_it_at_the_end() {
        use SidebarSection::{Actions, Branches, Remotes, Worktrees};
        let mut sections = SidebarSections::default();

        sections.apply(&crate::ui_state::UiState {
            section_order: Some(vec![
                String::from("remotes"),
                String::from("branches"),
                String::from("worktrees"),
            ]),
            ..Default::default()
        });

        assert_eq!(sections.ordered(), [Remotes, Branches, Worktrees, Actions]);
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
