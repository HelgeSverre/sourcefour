use gpui::{
    Div, FocusHandle, FontWeight, IntoElement, Render, StatefulInteractiveElement,
    UniformListScrollHandle, Window, actions, div, prelude::*, px, svg, uniform_list,
};
use sourcefour_git::{GixHistoryCursor, HistoryCursor as _};
use sourcefour_model::{
    AheadBehindState, AheadBehindUpdate, Generation, HeadSnapshot, HistoryBatch, HistoryQuery,
    HistoryScope, LoadState, RepoEnvelope, RepoFailure, RepoLocation, RepoSessionId, RepoSnapshot,
    RequestId, WorktreeAccessibility,
};

use crate::{
    app::WindowLaunch,
    demo,
    graph_paint::{HALO_OPACITY, HALO_RADIUS, NODE_RADIUS, STROKE_WIDTH, Shape, row_shapes},
    history::{HistoryState, is_scoped_to, refreshed_scope, relative_date, toggled_scope},
    panels::{PanelSizes, Splitter},
    theme::{
        HEADER_HEIGHT, HISTORY_ROW_HEIGHT, SPLITTER_WIDTH, STATUS_HEIGHT, TITLEBAR_HEIGHT,
        TOOLBAR_HEIGHT, Theme,
    },
};

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
    list_scroll: UniformListScrollHandle,
    focus: FocusHandle,
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
    ]
);

/// Rows a page key moves, matching the baseline viewport's row count.
const PAGE_ROWS: isize = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SidebarSection {
    Worktrees,
    Branches,
    Remotes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SidebarSections {
    worktrees: bool,
    branches: bool,
    remotes: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ColumnVisibility {
    author: bool,
    hash: bool,
}

impl ColumnVisibility {
    fn for_window_width(width: f32) -> Self {
        Self {
            author: width >= 1000.0,
            hash: width >= 940.0,
        }
    }
}

impl Default for SidebarSections {
    fn default() -> Self {
        Self {
            worktrees: true,
            branches: true,
            remotes: true,
        }
    }
}

impl SidebarSections {
    fn expanded(self, section: SidebarSection) -> bool {
        match section {
            SidebarSection::Worktrees => self.worktrees,
            SidebarSection::Branches => self.branches,
            SidebarSection::Remotes => self.remotes,
        }
    }
    fn toggle(&mut self, section: SidebarSection) {
        match section {
            SidebarSection::Worktrees => self.worktrees = !self.worktrees,
            SidebarSection::Branches => self.branches = !self.branches,
            SidebarSection::Remotes => self.remotes = !self.remotes,
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
    let content = row_count_as_f32(history.len()) * HISTORY_ROW_HEIGHT;
    // Mirror the list's own clamp so rubber-band overscroll cannot shear the
    // graph away from the rows it annotates.
    let scroll_top =
        (-scroll.0.borrow().base_handle.offset().y.0).clamp(0.0, (content - viewport).max(0.0));
    let first = usize_from_f32((scroll_top / HISTORY_ROW_HEIGHT).floor());
    let last = history
        .len()
        .min(first + usize_from_f32((viewport / HISTORY_ROW_HEIGHT).ceil()) + 1);
    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
        for index in first..last {
            let Some(graph) = history.layout.get(index) else {
                break;
            };
            let is_head = history
                .rows
                .get(index)
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
            list_scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
        };
        if launch.demo {
            // The fixture is seeded as if a traversal had already completed, so
            // every capture goes through the real rendering path (§12.4).
            window.repo = LoadState::Ready(demo::snapshot());
            window.history.reset(HistoryScope::AllRefs);
            let (rows, layout) = demo::history();
            window.history.extend(rows, layout, false);
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
            cx.notify();
        }
    }

    /// Selects one loaded row by index and keeps it visible.
    fn select_row(&mut self, index: usize, cx: &mut gpui::Context<Self>) {
        if let Some(index) = self.history.select_index(index) {
            self.list_scroll
                .scroll_to_item(index, gpui::ScrollStrategy::Top);
            cx.notify();
        }
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

    fn toolbar(&self) -> impl IntoElement {
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
            .child(action("Fetch", "icons/cloud-download.svg", false))
            .child(action("Pull", "icons/arrow-down-to-line.svg", true))
            .child(action("Push", "icons/arrow-up-from-line.svg", true))
            .child(action("Commit", "icons/git-commit-horizontal.svg", true))
            .child(action("Branch", "icons/git-branch.svg", false))
            .child(action("Merge", "icons/git-merge.svg", true))
            .child(action("Stash", "icons/archive.svg", true))
            .child(div().flex_grow())
            .child(
                div()
                    .w(px(260.0))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .px(px(10.0))
                    .gap(px(7.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(self.theme.border_strong)
                    .bg(self.theme.bg_list)
                    .text_size(px(12.0))
                    .text_color(self.theme.text_faint)
                    .child(filter_icon(&self.theme))
                    .child("Filter commits")
                    .child(div().flex_grow())
                    .child("Cmd+F"),
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
            .when(section != SidebarSection::Worktrees, |this| {
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
                cx.notify();
            }))
    }

    fn sidebar(&self, cx: &mut gpui::Context<Self>) -> Div {
        match self.snapshot() {
            Some(snapshot) => self.repository_sidebar(snapshot, cx),
            None => self.loading_sidebar(cx),
        }
    }

    /// The sidebar built from real repository metadata.
    fn repository_sidebar(&self, snapshot: &RepoSnapshot, cx: &mut gpui::Context<Self>) -> Div {
        div()
            .w(px(self.panels.sidebar))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(self.theme.bg_panel)
            .child(self.section(
                "WORKTREES",
                snapshot.worktrees.len().to_string(),
                SidebarSection::Worktrees,
                cx,
            ))
            .when(self.sections.worktrees, |this| {
                this.children(
                    snapshot
                        .worktrees
                        .iter()
                        .map(|tree| self.worktree_row(tree)),
                )
            })
            .child(self.section(
                "BRANCHES",
                snapshot.local_branches.len().to_string(),
                SidebarSection::Branches,
                cx,
            ))
            .when(self.sections.branches, |this| {
                this.children(
                    snapshot
                        .local_branches
                        .iter()
                        .map(|branch| self.branch_row(branch, cx)),
                )
            })
            // The prototype counts remotes here, not their branches.
            .child(self.section(
                "REMOTES",
                snapshot.remotes.len().to_string(),
                SidebarSection::Remotes,
                cx,
            ))
            .when(self.sections.remotes, |this| {
                this.children(snapshot.remotes.iter().flat_map(|remote| {
                    std::iter::once(self.remote_row(remote)).chain(
                        remote
                            .branches
                            .iter()
                            .map(|branch| self.remote_branch_row(&branch.short_name)),
                    )
                }))
            })
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
                    .child(if tree.is_current { "CURRENT" } else { "" }),
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
        div()
            .w(px(self.panels.sidebar))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(self.theme.bg_panel)
            .child(self.section("WORKTREES", "", SidebarSection::Worktrees, cx))
            .when(self.sections.worktrees, |this| this.child(loading()))
            .child(self.section("BRANCHES", "", SidebarSection::Branches, cx))
            .when(self.sections.branches, |this| this.child(loading()))
            .child(self.section("REMOTES", "", SidebarSection::Remotes, cx))
            .when(self.sections.remotes, |this| this.child(loading()))
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

    /// A draggable divider; the window's mouse handlers do the actual moving.
    fn splitter(&self, splitter: Splitter, cx: &mut gpui::Context<Self>) -> Div {
        let vertical = matches!(splitter, Splitter::Sidebar | Splitter::Graph);
        let dragging = self.dragging == Some(splitter);
        let base = div()
            .flex_none()
            // The graph divider floats over content, so it only shows itself
            // when interacted with; the panel dividers read as borders.
            .bg(if dragging {
                self.theme.accent
            } else if matches!(splitter, Splitter::Graph) {
                gpui::transparent_black()
            } else {
                self.theme.border
            })
            .hover(|style| style.bg(self.theme.accent))
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
        if self.history.len() == 0 {
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
        if self.history.len() == 0 {
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
        // Demo captures anchor "now" so relative dates never drift between runs.
        let now = if self.demo {
            demo::NOW_SECONDS
        } else {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| {
                    i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
                })
        };
        div().size_full().bg(self.theme.bg_list).child(
            uniform_list(
                cx.entity(),
                "history",
                self.history.len(),
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
        let Some(row) = self.history.rows.get(index) else {
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
                    // Without clipping, a long subject runs straight through the
                    // author column instead of stopping at it.
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .pl(px(10.0))
                    .pr(px(10.0))
                    .text_size(px(12.5))
                    .text_color(if row.flags.is_merge {
                        self.theme.text_secondary
                    } else {
                        self.theme.text_primary
                    })
                    .child(row.summary.clone()),
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
                cx.notify();
            }))
    }

    fn details(&self) -> impl IntoElement {
        // §6.10: the header is populated from the already-loaded row while
        // the exact metadata, files, and diff arrive in M4.
        let selected = self
            .history
            .selected_index()
            .and_then(|index| self.history.rows.get(index));
        let (hash, subject, author) = selected.map_or_else(
            || {
                (
                    String::new(),
                    String::from("No commit selected"),
                    String::new(),
                )
            },
            |row| {
                (
                    row.oid.abbreviated(9),
                    row.summary.clone(),
                    row.author_name.clone(),
                )
            },
        );
        div()
            .h(px(self.panels.details))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(14.0))
            .pt(px(10.0))
            .bg(self.theme.bg_panel)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(self.theme.accent)
                    .child(hash),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(self.theme.text_primary)
                    .child(subject),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(self.theme.text_secondary)
                    .child(author),
            )
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
            .child(div().flex_grow())
            .child(self.snapshot().map_or_else(
                || String::from("Loading repository metadata..."),
                status_summary,
            ))
    }
}

impl Render for SourcefourWindow {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let columns = ColumnVisibility::for_window_width(window.viewport_size().width.0);
        div()
            .key_context("History")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectNextCommit, _, cx| {
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
                let last = this.history.len().saturating_sub(1);
                this.select_row(last, cx);
            }))
            // Splitter handles arm `dragging`; the window-wide handlers below
            // do the moving, so a fast drag cannot escape a 3px handle.
            .on_mouse_move(
                cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                    if let Some(splitter) = this.dragging {
                        this.panels.drag(
                            splitter,
                            event.position.x.0,
                            event.position.y.0,
                            window.viewport_size().height.0,
                        );
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.dragging.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(self.theme.bg_page)
            .text_color(self.theme.text_primary)
            .child(self.titlebar())
            .child(self.toolbar())
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
                            .child(self.splitter(Splitter::Details, cx))
                            .child(self.details()),
                    ),
            )
            .child(self.status())
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
    fn column_visibility_hides_author_before_hash() {
        assert_eq!(
            ColumnVisibility::for_window_width(1280.0),
            ColumnVisibility {
                author: true,
                hash: true
            }
        );
        assert_eq!(
            ColumnVisibility::for_window_width(980.0),
            ColumnVisibility {
                author: false,
                hash: true
            }
        );
        assert_eq!(
            ColumnVisibility::for_window_width(920.0),
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
