//! The left sidebar: reorderable, collapsible sections of worktrees,
//! branches, remotes, and Actions runs.

use crate::context_menu::{ContextMenuExt as _, PrimaryClickExt as _};
use gpui::{
    Div, FontWeight, IntoElement, Render, StatefulInteractiveElement, Window, div, prelude::*, px,
    svg,
};
use sourcefour_model::{
    AheadBehindState, BranchSnapshot, HeadSnapshot, RepoSnapshot, WorktreeAccessibility,
};

use std::collections::BTreeMap;

use crate::{
    history::{is_scoped_to, toggled_scope},
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

#[derive(Debug, Default)]
struct BranchTree<'a> {
    branch: Option<&'a BranchSnapshot>,
    children: BTreeMap<&'a str, BranchTree<'a>>,
    path: String,
}

impl<'a> BranchTree<'a> {
    fn from_branches(branches: &'a [BranchSnapshot]) -> Self {
        let mut root = Self::default();
        for branch in branches {
            let mut node = &mut root;
            let mut path = String::new();
            for segment in branch.short_name.split('/') {
                if !path.is_empty() {
                    path.push('/');
                }
                path.push_str(segment);
                node = node.children.entry(segment).or_insert_with(|| Self {
                    path: path.clone(),
                    ..Self::default()
                });
            }
            node.branch = Some(branch);
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
        "icons/chevron-down.svg"
    } else {
        "icons/chevron-right.svg"
    };
    div()
        .size(px(12.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(svg().path(path).size(px(10.0)).text_color(theme.text_faint))
}

fn branch_marker(theme: &Theme) -> gpui::Svg {
    svg()
        .path("icons/git-branch.svg")
        .size(px(12.0))
        .flex_none()
        .text_color(theme.text_faint)
}

fn folder_marker(theme: &Theme) -> gpui::Svg {
    svg()
        .path("icons/folder.svg")
        .size(px(12.0))
        .flex_none()
        .text_color(theme.text_faint)
}

fn branch_indent(depth: usize) -> f32 {
    15.0 + f32::from(u16::try_from(depth).unwrap_or(u16::MAX)) * 16.0
}

fn remote_marker(theme: &Theme) -> gpui::Svg {
    svg()
        .path("icons/globe.svg")
        .size(px(12.0))
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
            .on_primary_click(cx.listener(move |this, _, _, cx| {
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
        let branch_tree = BranchTree::from_branches(&snapshot.local_branches);
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
                    .when(self.sections.expanded(section), |this| {
                        this.children(self.branch_tree_rows(&branch_tree, 0, cx))
                    }),
                // The prototype counts remotes here, not their branches.
                SidebarSection::Remotes => root
                    .child(self.section("REMOTES", snapshot.remotes.len().to_string(), section, cx))
                    .when(self.sections.expanded(section), |this| {
                        this.children(snapshot.remotes.iter().flat_map(|remote| {
                            let mut rows = vec![self.remote_row(remote, cx)];
                            rows.extend(
                                remote
                                    .branches
                                    .iter()
                                    .map(|branch| self.remote_branch_row(branch, cx)),
                            );
                            rows
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

    pub(super) fn worktree_row(&self, tree: &sourcefour_model::WorktreeSnapshot) -> Div {
        let marker = match (&tree.accessibility, tree.is_current) {
            (WorktreeAccessibility::Inaccessible { .. }, _) => self.theme.red,
            (WorktreeAccessibility::Prunable { .. }, _) => self.theme.orange,
            (WorktreeAccessibility::Accessible, true) => self.theme.green,
            (WorktreeAccessibility::Accessible, false) => self.theme.text_faint,
        };
        let host = self.menus.downgrade();
        let path = tree.path.clone();
        let accessible = matches!(tree.accessibility, WorktreeAccessibility::Accessible);
        div()
            .on_context_menu(move |event, window, cx| {
                crate::context_menu::show(
                    &host,
                    event.position,
                    super::menus::path_entries(&path, accessible),
                    window,
                    cx,
                );
            })
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
        let tip = branch.tip;
        let menu_name = branch.short_name.clone();
        let menu_full_name = branch.full_name.clone();
        div()
            .on_context_menu(
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    let entries = this.ref_entries(
                        &menu_name,
                        Some(&menu_full_name),
                        sourcefour_model::RefKind::LocalBranch,
                        tip,
                        cx,
                    );
                    this.show_menu(event.position, entries, window, cx);
                }),
            )
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
            .on_primary_click(cx.listener(move |this, _, _, cx| {
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
            .child(label.into())
            .children(self.pr_chip(&branch.short_name))
            .child(div().flex_grow_1())
            .children(ahead_behind_text(branch.ahead_behind).map(|text| {
                div()
                    .text_size(px(10.5))
                    .text_color(self.theme.accent)
                    .child(text)
            }))
    }

    fn branch_tree_rows(
        &self,
        tree: &BranchTree<'_>,
        depth: usize,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut rows = Vec::new();
        for (&segment, node) in &tree.children {
            if node.children.is_empty() {
                if let Some(branch) = node.branch {
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
            if let Some(branch) = node.branch {
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
            .child(label.to_owned())
            .on_primary_click(cx.listener(move |this, _, _, cx| {
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
    ) -> Div {
        let name = remote.name.clone();
        div()
            .on_context_menu(
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    let entries = this.remote_entries(&name, cx);
                    this.show_menu(event.position, entries, window, cx);
                }),
            )
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
            .child(remote.name.clone())
            .child(div().flex_grow_1())
            .children(remote.fetch_url.clone().map(|url| {
                div()
                    .text_size(px(10.0))
                    .text_color(self.theme.text_faint)
                    .child(url)
            }))
    }

    pub(super) fn remote_branch_row(
        &self,
        branch: &sourcefour_model::RemoteBranchSnapshot,
        cx: &mut gpui::Context<Self>,
    ) -> Div {
        let target = branch.clone();
        div()
            .on_context_menu(
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    let entries = this.ref_entries(
                        &target.short_name,
                        Some(&target.full_name),
                        sourcefour_model::RefKind::RemoteBranch,
                        target.tip,
                        cx,
                    );
                    this.show_menu(event.position, entries, window, cx);
                }),
            )
            .h(px(24.0))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(34.0))
            .text_size(px(12.0))
            .text_color(self.theme.purple)
            .child(branch.short_name.clone())
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

        let tree = BranchTree::from_branches(&branches);
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

        let tree = BranchTree::from_branches(&branches);
        let feature = &tree.children["feature"];

        assert_eq!(
            feature.branch.map(|branch| branch.short_name.as_str()),
            Some("feature")
        );
        assert_eq!(
            feature.children["login"]
                .branch
                .map(|branch| branch.short_name.as_str()),
            Some("feature/login")
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
