//! The left sidebar: reorderable, collapsible sections of worktrees,
//! branches, remotes, and Actions runs.

use gpui::{
    AnyView, App, Div, FontWeight, Hsla, IntoElement, Render, SharedString,
    StatefulInteractiveElement, Window, div, prelude::*, px, svg,
};
use sourcefour_model::{
    AheadBehindState, BranchSnapshot, HeadSnapshot, RemoteBranchSnapshot, RepoSnapshot,
    WorktreeAccessibility,
};

use std::collections::BTreeMap;

use crate::{
    history::{is_scoped_to, toggled_scope},
    icons::Icon,
    theme::Theme,
};

use super::SourcefourWindow;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SidebarSection {
    Worktrees,
    Branches,
    Remotes,
    /// GitHub Actions runs; renders only when a GitHub remote resolves.
    Actions,
}

impl SidebarSection {
    /// Stable name used in the persisted state file.
    pub(super) fn name(self) -> &'static str {
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
pub(super) struct SidebarSections {
    /// Expansion per section, indexed by declaration order.
    expanded: [bool; 4],
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

struct SidebarTooltip {
    text: SharedString,
    palette: TooltipPalette,
}

#[derive(Clone, Copy)]
struct TooltipPalette {
    background: Hsla,
    border: Hsla,
    text: Hsla,
}

impl From<&Theme> for TooltipPalette {
    fn from(theme: &Theme) -> Self {
        Self {
            background: theme.bg_chrome,
            border: theme.border_strong,
            text: theme.text_primary,
        }
    }
}

impl Render for SidebarTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(5.0))
            .border_1()
            .border_color(self.palette.border)
            .bg(self.palette.background)
            .text_size(px(11.0))
            .text_color(self.palette.text)
            .shadow_md()
            .child(self.text.clone())
    }
}

fn sidebar_tooltip(
    text: impl Into<SharedString>,
    palette: TooltipPalette,
    cx: &mut App,
) -> AnyView {
    let text = text.into();
    cx.new(|_| SidebarTooltip { text, palette }).into()
}

#[derive(Debug)]
struct RefTree<'a, T> {
    item: Option<&'a T>,
    children: BTreeMap<&'a str, RefTree<'a, T>>,
    path: String,
}

impl<T> Default for RefTree<'_, T> {
    fn default() -> Self {
        Self {
            item: None,
            children: BTreeMap::new(),
            path: String::new(),
        }
    }
}

impl<'a, T> RefTree<'a, T> {
    fn insert(&mut self, name: &'a str, item: &'a T) {
        let mut node = self;
        let mut path = String::new();
        for segment in name.split('/') {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(segment);
            node = node.children.entry(segment).or_insert_with(|| Self {
                path: path.clone(),
                ..Self::default()
            });
        }
        node.item = Some(item);
    }
}

impl<'a> RefTree<'a, BranchSnapshot> {
    fn from_branches(branches: &'a [BranchSnapshot]) -> Self {
        let mut root = Self::default();
        for branch in branches {
            root.insert(&branch.short_name, branch);
        }
        root
    }
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

impl Default for SidebarSections {
    fn default() -> Self {
        Self {
            expanded: [true; 4],
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
        self.expanded[section as usize]
    }
    fn toggle(&mut self, section: SidebarSection) {
        self.expanded[section as usize] = !self.expanded[section as usize];
    }
    /// The sections in their current display order.
    pub(super) fn ordered(self) -> [SidebarSection; 4] {
        self.order
    }
    /// Applies persisted order and collapse state, ignoring anything that
    /// no longer names a section. Sections this build knows but the file
    /// predates keep their default position at the end.
    pub(super) fn apply(&mut self, state: &crate::ui_state::UiState) {
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
            if let Some(section) = SidebarSection::from_name(name) {
                self.expanded[section as usize] = false;
            }
        }
    }

    /// The names persisted for the collapse state.
    pub(super) fn collapsed_names(self) -> Vec<String> {
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
pub(super) fn head_label(head: &HeadSnapshot) -> String {
    match head {
        HeadSnapshot::Branch { short_name, .. } => short_name.clone(),
        HeadSnapshot::Detached { oid } => oid.abbreviated(7),
        HeadSnapshot::Unborn { .. } => String::from("unborn"),
        HeadSnapshot::Missing => String::from("no HEAD"),
    }
}

fn disclosure(theme: &Theme, expanded: bool) -> Div {
    let path = if expanded {
        Icon::ChevronDown
    } else {
        Icon::ChevronRight
    };
    div()
        .size(px(12.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(path.path())
                .size(px(10.0))
                .text_color(theme.text_faint),
        )
}

fn branch_marker(theme: &Theme) -> gpui::Svg {
    svg()
        .path(Icon::GitBranch.path())
        .size(px(12.0))
        .flex_none()
        .text_color(theme.text_faint)
}

fn folder_marker(theme: &Theme) -> gpui::Svg {
    svg()
        .path(Icon::Folder.path())
        .size(px(12.0))
        .flex_none()
        .text_color(theme.text_faint)
}

fn branch_indent(depth: usize) -> f32 {
    15.0 + f32::from(u16::try_from(depth).unwrap_or(u16::MAX)) * 16.0
}

fn truncating_label(label: impl Into<SharedString>) -> Div {
    div()
        .flex_1()
        .min_w(px(1.0))
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .child(label.into())
}

fn remote_marker(theme: &Theme) -> gpui::Svg {
    svg()
        .path(Icon::Globe.path())
        .size(px(12.0))
        .flex_none()
        .text_color(theme.orange)
}

impl SourcefourWindow {
    pub(super) fn section(
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
            .flex_none()
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

    pub(super) fn sidebar(&self, cx: &mut gpui::Context<Self>) -> gpui::AnyElement {
        match self.snapshot() {
            Some(snapshot) => self.repository_sidebar(snapshot, cx).into_any_element(),
            None => self.loading_sidebar(cx).into_any_element(),
        }
    }

    /// The sidebar built from real repository metadata, sections in the
    /// user's order.
    pub(super) fn repository_sidebar(
        &self,
        snapshot: &RepoSnapshot,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        let branch_tree = RefTree::from_branches(&snapshot.local_branches);
        let mut root = div()
            .w(px(self.panels.sidebar))
            .flex_none()
            .flex()
            .flex_col()
            .id("sidebar-scroll")
            .overflow_y_scroll()
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
                    .when(self.sections.expanded(section), |this| {
                        this.children(
                            snapshot
                                .worktrees
                                .iter()
                                .map(|tree| self.worktree_row(tree, cx)),
                        )
                    }),
                SidebarSection::Branches => root
                    .child(self.section(
                        "BRANCHES",
                        snapshot.local_branches.len().to_string(),
                        section,
                        cx,
                    ))
                    .when(self.sections.expanded(section), |this| {
                        this.children(self.branch_tree_rows(&branch_tree, 0, cx))
                    }),
                SidebarSection::Remotes => root
                    .child(
                        self.section(
                            "REMOTES",
                            snapshot
                                .remotes
                                .iter()
                                .map(|remote| remote.branches.len())
                                .sum::<usize>()
                                .to_string(),
                            section,
                            cx,
                        ),
                    )
                    .when(self.sections.expanded(section), |this| {
                        this.children(snapshot.remotes.iter().flat_map(|remote| {
                            let mut tree = RefTree::default();
                            for branch in &remote.branches {
                                let relative = branch
                                    .short_name
                                    .strip_prefix(&format!("{}/", remote.name))
                                    .unwrap_or(&branch.short_name);
                                tree.insert(relative, branch);
                            }
                            std::iter::once(self.remote_row(remote, cx).into_any_element())
                                .chain(self.remote_tree_rows(&tree, &remote.name, 0, cx))
                                .collect::<Vec<_>>()
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
                        .when(self.sections.expanded(section), |this| {
                            this.children(
                                runs.iter()
                                    .enumerate()
                                    .map(|(index, run)| self.workflow_run_row(index, run, cx)),
                            )
                        })
                }
            };
        }
        root
    }

    pub(super) fn worktree_row(
        &self,
        tree: &sourcefour_model::WorktreeSnapshot,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        let marker = match (&tree.accessibility, tree.is_current) {
            (WorktreeAccessibility::Inaccessible { .. }, _) => self.theme.red,
            (WorktreeAccessibility::Prunable { .. }, _) => self.theme.orange,
            (WorktreeAccessibility::Accessible, true) => self.theme.green,
            (WorktreeAccessibility::Accessible, false) => self.theme.text_faint,
        };
        let path = tree.path.clone();
        let activatable =
            matches!(tree.accessibility, WorktreeAccessibility::Accessible) && !tree.is_current;
        div()
            .id(gpui::SharedString::from(format!("worktree:{}", tree.id.0)))
            .h(px(47.0))
            .flex_none()
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
            .when(activatable, |row| {
                row.cursor_pointer()
                    .hover(|style| style.bg(self.theme.bg_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_worktree_path(path.clone(), cx);
                    }))
            })
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
                            .child(div().size(px(7.0)).flex_none().rounded_full().bg(marker))
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
                            .border_color(self.theme.chip_border(self.theme.accent))
                            .bg(self.theme.chip_fill(self.theme.accent))
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

    pub(super) fn branch_row(
        &self,
        branch: &BranchSnapshot,
        label: impl Into<gpui::SharedString>,
        depth: usize,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        let scoped = is_scoped_to(self.history.scope.as_ref(), &branch.full_name);
        let full_name = branch.full_name.clone();
        let worktree_full_name = branch.full_name.clone();
        let worktree_short_name = branch.short_name.clone();
        let worktree_tooltip = format!("Create worktree from {}", branch.short_name);
        let tooltip_palette = TooltipPalette::from(&self.theme);
        let label = label.into();
        let tip = branch.tip;
        div()
            .id(gpui::SharedString::from(branch.full_name.clone()))
            .h(px(26.0))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(branch_indent(depth)))
            .pr(px(15.0))
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
            .child(truncating_label(label))
            .children(self.pr_chip(&branch.short_name))
            .children(ahead_behind_text(branch.ahead_behind).map(|text| {
                div()
                    .flex_none()
                    .text_size(px(10.5))
                    .text_color(self.theme.accent)
                    .child(text)
            }))
            .child(
                div()
                    .id(gpui::SharedString::from(format!(
                        "add-worktree:{}",
                        branch.full_name
                    )))
                    .size(px(20.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(self.theme.bg_hover))
                    .tooltip(move |_, cx| {
                        sidebar_tooltip(worktree_tooltip.clone(), tooltip_palette, cx)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_worktree_dialog(
                            super::worktree_dialog::WorktreeDialogSource::Local {
                                full_name: worktree_full_name.clone(),
                                short_name: worktree_short_name.clone(),
                            },
                            window,
                            cx,
                        );
                    }))
                    .child(
                        svg()
                            .path(Icon::FolderPlus.path())
                            .size(px(12.0))
                            .text_color(self.theme.text_faint),
                    ),
            )
    }

    fn branch_tree_rows(
        &self,
        tree: &RefTree<'_, BranchSnapshot>,
        depth: usize,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut rows = Vec::new();
        for (&segment, node) in &tree.children {
            if node.children.is_empty() {
                if let Some(branch) = node.item {
                    rows.push(
                        self.branch_row(branch, segment.to_owned(), depth, cx)
                            .into_any_element(),
                    );
                }
                continue;
            }

            let expanded = !self.collapsed_branch_folders.contains(&node.path);
            rows.push(
                self.branch_folder_row(segment, &node.path, depth, expanded, cx)
                    .into_any_element(),
            );
            if !expanded {
                continue;
            }
            if let Some(branch) = node.item {
                rows.push(
                    self.branch_row(branch, branch.short_name.clone(), depth + 1, cx)
                        .into_any_element(),
                );
            }
            rows.extend(self.branch_tree_rows(node, depth + 1, cx));
        }
        rows
    }

    fn branch_folder_row(
        &self,
        label: &str,
        path: &str,
        depth: usize,
        expanded: bool,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        let path = path.to_owned();
        div()
            .id(gpui::SharedString::from(format!("branch-folder:{path}")))
            .h(px(26.0))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(branch_indent(depth)))
            .pr(px(15.0))
            .gap(px(7.0))
            .cursor_pointer()
            .hover(|style| style.bg(self.theme.bg_hover))
            .text_size(px(12.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(self.theme.text_secondary)
            .child(disclosure(&self.theme, expanded))
            .child(folder_marker(&self.theme))
            .child(truncating_label(label.to_owned()))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.collapsed_branch_folders.remove(&path) {
                    this.collapsed_branch_folders.insert(path.clone());
                }
                this.persist_ui_state(cx);
                cx.notify();
            }))
    }

    pub(super) fn remote_row(
        &self,
        remote: &sourcefour_model::RemoteSnapshot,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        let remote_name = remote.name.clone();
        let tooltip = format!("Fetch {}", remote.name);
        let tooltip_palette = TooltipPalette::from(&self.theme);
        div()
            .id(gpui::SharedString::from(format!("remote:{}", remote.name)))
            .h(px(29.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(14.0))
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(self.theme.orange)
            .child(remote_marker(&self.theme))
            .child(truncating_label(remote.name.clone()).flex_initial())
            .children(remote.fetch_url.clone().map(|url| {
                truncating_label(url)
                    .text_size(px(10.0))
                    .text_color(self.theme.text_faint)
            }))
            .child(
                div()
                    .id(gpui::SharedString::from(format!("fetch:{}", remote.name)))
                    .size(px(20.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(self.theme.bg_hover))
                    .tooltip(move |_, cx| sidebar_tooltip(tooltip.clone(), tooltip_palette, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.fetch_remote(remote_name.clone(), cx);
                    }))
                    .child(
                        svg()
                            .path(Icon::CloudDownload.path())
                            .size(px(12.0))
                            .text_color(self.theme.orange),
                    ),
            )
    }

    fn remote_tree_rows(
        &self,
        tree: &RefTree<'_, RemoteBranchSnapshot>,
        remote: &str,
        depth: usize,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut rows = Vec::new();
        for (&segment, node) in &tree.children {
            if node.children.is_empty() {
                if let Some(branch) = node.item {
                    rows.push(
                        self.remote_branch_row(branch, segment, depth, cx)
                            .into_any_element(),
                    );
                }
                continue;
            }
            let key = format!("remote:{remote}:{}", node.path);
            let expanded = !self.collapsed_branch_folders.contains(&key);
            rows.push(
                self.branch_folder_row(segment, &key, depth + 1, expanded, cx)
                    .into_any_element(),
            );
            if !expanded {
                continue;
            }
            if let Some(branch) = node.item {
                rows.push(
                    self.remote_branch_row(branch, &node.path, depth + 1, cx)
                        .into_any_element(),
                );
            }
            rows.extend(self.remote_tree_rows(node, remote, depth + 1, cx));
        }
        rows
    }

    pub(super) fn remote_branch_row(
        &self,
        branch: &RemoteBranchSnapshot,
        label: &str,
        depth: usize,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        let scoped = is_scoped_to(self.history.scope.as_ref(), &branch.full_name);
        let full_name = branch.full_name.clone();
        let action_full_name = branch.full_name.clone();
        let action_short_name = branch.short_name.clone();
        let worktree_full_name = branch.full_name.clone();
        let worktree_short_name = branch.short_name.clone();
        let tracking_tip = format!("Use {} locally", branch.full_name);
        let worktree_tip = format!("Create worktree from {}", branch.full_name);
        let palette = TooltipPalette::from(&self.theme);
        let tip = branch.tip;
        div()
            .id(gpui::SharedString::from(branch.full_name.clone()))
            .h(px(24.0))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(branch_indent(depth + 1)))
            .pr(px(15.0))
            .gap(px(7.0))
            .cursor_pointer()
            .hover(|style| style.bg(self.theme.bg_hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                let scope = toggled_scope(this.history.scope.as_ref(), &full_name, tip);
                this.start_history(scope, cx);
                cx.notify();
            }))
            .bg(if scoped {
                self.theme.bg_selected
            } else {
                self.theme.bg_panel
            })
            .text_size(px(12.0))
            .text_color(self.theme.purple)
            .child(branch_marker(&self.theme))
            .child(truncating_label(label.to_owned()))
            .child(
                div()
                    .id(gpui::SharedString::from(format!(
                        "track:{}",
                        branch.full_name
                    )))
                    .size(px(20.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(self.theme.bg_hover))
                    .tooltip(move |_, cx| sidebar_tooltip(tracking_tip.clone(), palette, cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.use_remote_branch(
                            action_full_name.clone(),
                            action_short_name.clone(),
                            window,
                            cx,
                        );
                    }))
                    .child(
                        svg()
                            .path(Icon::CloudDownload.path())
                            .size(px(12.0))
                            .text_color(self.theme.purple),
                    ),
            )
            .child(
                div()
                    .id(gpui::SharedString::from(format!(
                        "remote-worktree:{}",
                        branch.full_name
                    )))
                    .size(px(20.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(self.theme.bg_hover))
                    .tooltip(move |_, cx| sidebar_tooltip(worktree_tip.clone(), palette, cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_worktree_dialog(
                            super::worktree_dialog::WorktreeDialogSource::Remote {
                                full_name: worktree_full_name.clone(),
                                short_name: worktree_short_name.clone(),
                            },
                            window,
                            cx,
                        );
                    }))
                    .child(
                        svg()
                            .path(Icon::FolderPlus.path())
                            .size(px(12.0))
                            .text_color(self.theme.text_faint),
                    ),
            )
    }

    pub(super) fn loading_sidebar(&self, cx: &mut gpui::Context<Self>) -> Div {
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
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{AheadBehindState, Oid};

    use super::*;

    fn branch(short_name: &str) -> BranchSnapshot {
        BranchSnapshot {
            full_name: format!("refs/heads/{short_name}"),
            short_name: short_name.to_owned(),
            tip: Oid::sha1([0; 20]),
            is_current: false,
            upstream: None,
            ahead_behind: AheadBehindState::Unavailable,
            checked_out_in: None,
        }
    }

    #[test]
    fn branches_form_a_recursive_alphabetical_tree() {
        let branches = [
            branch("main"),
            branch("feature/zebra"),
            branch("feature/auth/login"),
            branch("fix/crash"),
        ];

        let tree = RefTree::from_branches(&branches);
        assert_eq!(
            tree.children.keys().copied().collect::<Vec<_>>(),
            ["feature", "fix", "main"]
        );
        let feature = &tree.children["feature"];
        assert_eq!(feature.path, "feature");
        assert_eq!(
            feature.children.keys().copied().collect::<Vec<_>>(),
            ["auth", "zebra"]
        );
        assert_eq!(
            feature.children["auth"].children["login"].path,
            "feature/auth/login"
        );
    }

    #[test]
    fn a_branch_can_also_be_a_folder_prefix() {
        let branches = [branch("feature"), branch("feature/login")];

        let tree = RefTree::from_branches(&branches);
        let feature = &tree.children["feature"];

        assert_eq!(
            feature.item.map(|branch| branch.short_name.as_str()),
            Some("feature")
        );
        assert_eq!(
            feature.children["login"]
                .item
                .map(|branch| branch.short_name.as_str()),
            Some("feature/login")
        );
    }

    #[test]
    fn remote_names_form_the_same_recursive_tree_without_the_remote_prefix() {
        let branches = [
            sourcefour_model::RemoteBranchSnapshot {
                full_name: String::from("refs/remotes/origin/feature/auth/login"),
                short_name: String::from("origin/feature/auth/login"),
                tip: Oid::sha1([0; 20]),
            },
            sourcefour_model::RemoteBranchSnapshot {
                full_name: String::from("refs/remotes/origin/fix/crash"),
                short_name: String::from("origin/fix/crash"),
                tip: Oid::sha1([1; 20]),
            },
        ];
        let mut tree = RefTree::default();
        for branch in &branches {
            tree.insert(branch.short_name.strip_prefix("origin/").unwrap(), branch);
        }
        assert_eq!(
            tree.children.keys().copied().collect::<Vec<_>>(),
            ["feature", "fix"]
        );
        assert_eq!(
            tree.children["feature"].children["auth"].children["login"]
                .item
                .unwrap()
                .full_name,
            "refs/remotes/origin/feature/auth/login"
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
}
