//! The window chrome: titlebar, toolbar, filter field, status bar, and the
//! panel splitters, plus the §6.12 fetch that the toolbar launches.

use std::sync::{Arc, Mutex, atomic::AtomicBool};

use gpui::{Div, IntoElement, Window, div, prelude::*, px, svg};
use sourcefour_git::OperationSink;
use sourcefour_model::{FetchRequest, OperationOutcome, OperationProgress, RepoSnapshot};

use crate::{
    icons::Icon,
    panels::Splitter,
    theme::{SPLITTER_WIDTH, STATUS_HEIGHT, TITLEBAR_HEIGHT, TOOLBAR_HEIGHT, Theme},
};

use super::{SourcefourWindow, counted, head_label};

pub(super) enum NetworkOperationState {
    Idle {
        generation: u64,
    },
    Running {
        generation: u64,
        op: NetworkOp,
        progress: Arc<Mutex<Option<OperationProgress>>>,
        cancel: Arc<AtomicBool>,
    },
}

impl Default for NetworkOperationState {
    fn default() -> Self {
        Self::Idle { generation: 0 }
    }
}

impl NetworkOperationState {
    pub(super) fn cancel_and_retire(&mut self) {
        if let Self::Running { cancel, .. } = self {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        *self = Self::Idle {
            generation: self.generation().wrapping_add(1),
        };
    }

    fn running_op(&self) -> Option<NetworkOp> {
        match self {
            Self::Idle { .. } => None,
            Self::Running { op, .. } => Some(*op),
        }
    }

    fn progress(&self) -> Option<&Arc<Mutex<Option<OperationProgress>>>> {
        match self {
            Self::Idle { .. } => None,
            Self::Running { progress, .. } => Some(progress),
        }
    }

    fn generation(&self) -> u64 {
        match self {
            Self::Idle { generation } | Self::Running { generation, .. } => *generation,
        }
    }

    fn running_generation(&self) -> Option<u64> {
        match self {
            Self::Idle { .. } => None,
            Self::Running { generation, .. } => Some(*generation),
        }
    }

    fn finish(&mut self, generation: u64) -> bool {
        if self.running_generation() != Some(generation) {
            return false;
        }
        *self = Self::Idle { generation };
        true
    }
}

/// The three network operations the toolbar can launch (§6.12).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NetworkOp {
    Fetch,
    Push,
    Pull,
}

impl NetworkOp {
    fn label(self) -> &'static str {
        match self {
            Self::Fetch => "Fetch",
            Self::Push => "Push",
            Self::Pull => "Pull",
        }
    }

    fn running_label(self) -> &'static str {
        match self {
            Self::Fetch => "Fetching",
            Self::Push => "Pushing",
            Self::Pull => "Pulling",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Fetch => Icon::CloudDownload,
            Self::Push => Icon::ArrowUpFromLine,
            Self::Pull => Icon::ArrowDownToLine,
        }
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

fn filter_icon(theme: &Theme) -> gpui::Svg {
    svg()
        .path(Icon::Search.path())
        .size(px(13.0))
        .text_color(theme.text_faint)
}

impl SourcefourWindow {
    pub(super) fn titlebar(&self) -> impl IntoElement {
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

    pub(super) fn toolbar(
        &self,
        window: &Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let action = |name, icon| {
            self.toolbar_column(name, icon, false)
                .hover(|this| this.bg(self.theme.bg_hover))
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
            .child(self.operation_button(NetworkOp::Fetch, cx))
            .child(self.operation_button(NetworkOp::Pull, cx))
            .child(self.operation_button(NetworkOp::Push, cx))
            .child(action("Commit", Icon::GitCommitHorizontal))
            .child(
                self.toolbar_column("Branch", Icon::GitBranch, true)
                    .id("branch-action")
                    .cursor_pointer()
                    .hover(|style| style.bg(self.theme.bg_hover))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_branch_dialog(window, cx);
                    })),
            )
            .child(action("Merge", Icon::GitMerge))
            .child(action("Stash", Icon::Archive))
            .child(div().flex_grow())
            .child(self.filter_box(window, cx))
            .child(crate::settings_ui::toolbar_button(&self.theme, cx))
    }

    /// One toolbar column: icon above label, lit when active. Callers add
    /// identity and click behavior; planned actions stay inert and faint.
    fn toolbar_column(&self, label: &'static str, icon: Icon, active: bool) -> Div {
        div()
            .h_full()
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .gap(px(3.0))
            .w(px(58.0))
            .text_size(px(10.0))
            .text_color(if active {
                self.theme.text_secondary
            } else {
                self.theme.text_faint
            })
            .child(
                svg()
                    .path(icon.path())
                    .size(px(15.0))
                    .text_color(if active {
                        self.theme.accent
                    } else {
                        self.theme.text_faint
                    }),
            )
            .child(label)
    }

    /// One live operation button: disabled while any operation runs, and
    /// wearing the running label while its own does (§6.12).
    fn operation_button(&self, op: NetworkOp, cx: &mut gpui::Context<Self>) -> gpui::Stateful<Div> {
        let running_op = self.network_operation.running_op();
        let busy = running_op.is_some();
        let mine = running_op == Some(op);
        self.toolbar_column(
            if mine { op.running_label() } else { op.label() },
            op.icon(),
            !busy,
        )
        .id(op.label())
        .when(!busy, |this| {
            this.cursor_pointer()
                .hover(|style| style.bg(self.theme.bg_hover))
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.start_operation(op, cx);
        }))
    }

    /// The §4.7 filter field, wrapping the real text input.
    pub(super) fn filter_box(
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

    /// Starts one network operation through the user's own Git (§6.12).
    ///
    /// A second operation while one runs is a no-op: the buttons disable,
    /// and this guard holds even if a keybinding races the render.
    pub(super) fn start_operation(&mut self, op: NetworkOp, cx: &mut gpui::Context<Self>) {
        self.start_operation_with_remote(op, None, cx);
    }

    /// Fetches one named remote from its sidebar header.
    pub(super) fn fetch_remote(&mut self, remote: String, cx: &mut gpui::Context<Self>) {
        self.start_operation_with_remote(NetworkOp::Fetch, Some(remote), cx);
    }

    fn start_operation_with_remote(
        &mut self,
        op: NetworkOp,
        remote: Option<String>,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.network_operation.running_op().is_some() {
            return;
        }
        let Some(location) = self.location.clone() else {
            return;
        };
        let request = FetchRequest {
            worktree: self.active_worktree_id(),
            remote,
            prune: self.settings.git.fetch_prune,
        };
        let latest = Arc::new(Mutex::new(None));
        let cancel = Arc::new(AtomicBool::new(false));
        let generation = self.network_operation.generation().wrapping_add(1);
        self.network_operation = NetworkOperationState::Running {
            generation,
            op,
            progress: Arc::clone(&latest),
            cancel: Arc::clone(&cancel),
        };
        self.op_status = None;
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let sink = LatestSink(latest);
                    match op {
                        NetworkOp::Fetch => {
                            sourcefour_git::fetch(&location, &request, &sink, &cancel)
                        }
                        NetworkOp::Push => sourcefour_git::push(&location, &sink, &cancel),
                        NetworkOp::Pull => sourcefour_git::pull(&location, &sink, &cancel),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if !this.network_operation.finish(generation) {
                    return;
                }
                match outcome {
                    Ok(OperationOutcome::Succeeded { summary, .. }) => {
                        this.op_status = Some((true, summary));
                        // Refresh immediately rather than waiting for the
                        // watcher's next poll (§6.12).
                        this.begin_reload(cx);
                    }
                    Ok(OperationOutcome::Cancelled { .. }) => {
                        this.op_status = Some((true, format!("{} cancelled", op.label())));
                    }
                    Ok(OperationOutcome::Failed { error, .. }) => {
                        this.op_status = Some((false, error.user.message));
                    }
                    Err(failure) => {
                        this.op_status = Some((false, failure.user.message));
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
                        if this.network_operation.running_generation() == Some(generation) {
                            cx.notify();
                            false
                        } else {
                            true
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

    /// A draggable divider; the window's mouse handlers do the actual moving.
    pub(super) fn splitter(&self, splitter: Splitter, cx: &mut gpui::Context<Self>) -> Div {
        let vertical = matches!(splitter, Splitter::Sidebar | Splitter::Graph);
        let dragging = self.drag == Some(super::Drag::Splitter(splitter));
        let base = div()
            .flex_none()
            // The graph divider floats over content, so it only shows itself
            // when interacted with; the panel dividers read as borders.
            .bg(if dragging {
                self.theme.grab_active()
            } else if matches!(splitter, Splitter::Graph) {
                gpui::transparent_black()
            } else {
                self.theme.border
            })
            .hover(|style| style.bg(self.theme.grab_hover()))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.drag = Some(super::Drag::Splitter(splitter));
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

    pub(super) fn status(&self) -> impl IntoElement {
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
            .children(self.network_operation.progress().map(|latest| {
                let message = latest
                    .lock()
                    .ok()
                    .and_then(|progress| progress.clone())
                    .map_or_else(
                        || {
                            format!(
                                "{}…",
                                self.network_operation
                                    .running_op()
                                    .map_or("Working", NetworkOp::running_label)
                            )
                        },
                        |progress| progress.message,
                    );
                div().text_color(self.theme.accent).child(message)
            }))
            .children(self.op_status.clone().map(|(ok, message)| {
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, atomic::AtomicBool};

    use super::{NetworkOp, NetworkOperationState};

    fn running(generation: u64, op: NetworkOp) -> NetworkOperationState {
        NetworkOperationState::Running {
            generation,
            op,
            progress: Arc::new(Mutex::new(None)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn only_the_matching_network_operation_can_finish() {
        let mut state = running(4, NetworkOp::Fetch);

        assert!(!state.finish(3));
        assert_eq!(state.running_op(), Some(NetworkOp::Fetch));
        assert!(state.finish(4));
        assert_eq!(state.running_op(), None);
    }

    #[test]
    fn idle_state_retains_the_last_generation() {
        let mut state = running(7, NetworkOp::Pull);

        assert!(state.finish(7));
        assert_eq!(state.generation(), 7);
        assert_eq!(state.running_generation(), None);
    }
}
