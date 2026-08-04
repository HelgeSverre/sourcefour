use gpui::{
    Div, FontWeight, IntoElement, Render, ScrollHandle, StatefulInteractiveElement, Window, div,
    point, prelude::*, px, svg,
};
use sourcefour_model::RepoFailure;

use crate::{
    demo::COMMITS,
    theme::{
        DETAILS_HEIGHT, GRAPH_WIDTH, HEADER_HEIGHT, HISTORY_ROW_HEIGHT, SIDEBAR_WIDTH,
        STATUS_HEIGHT, TITLEBAR_HEIGHT, TOOLBAR_HEIGHT, Theme,
    },
};

pub(crate) struct SourcefourWindow {
    /// Repository name, stable across the repository's worktrees.
    name: String,
    /// Display-friendly active worktree path.
    path: String,
    demo: bool,
    theme: Theme,
    sections: SidebarSections,
    history_scroll: ScrollHandle,
}

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

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollbarMetrics {
    thumb_height: f32,
    thumb_top: f32,
    maximum_offset: f32,
}

fn scrollbar_metrics(
    content_height: f32,
    viewport_height: f32,
    offset: f32,
) -> Option<ScrollbarMetrics> {
    if content_height <= viewport_height || viewport_height <= 0.0 {
        return None;
    }
    let thumb_height = (viewport_height * viewport_height / content_height)
        .max(24.0)
        .min(viewport_height);
    let maximum_offset = content_height - viewport_height;
    let offset = offset.clamp(0.0, maximum_offset);
    let thumb_top =
        3.0 + (offset / maximum_offset) * (viewport_height - thumb_height - 6.0).max(0.0);
    Some(ScrollbarMetrics {
        thumb_height,
        thumb_top,
        maximum_offset,
    })
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

fn toolbar_icon(theme: &Theme, index: u8) -> Div {
    let color = if index == 0 || index == 4 {
        theme.accent
    } else {
        theme.text_faint
    };
    match index {
        0..=2 => div()
            .size(px(16.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .child(div().w(px(1.0)).h(px(6.0)).bg(color))
            .child(div().w(px(7.0)).h(px(1.0)).bg(color))
            .child(div().w(px(3.0)).h(px(1.0)).bg(color)),
        3 => div()
            .size(px(12.0))
            .rounded_full()
            .border_1()
            .border_color(color)
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(3.0)).rounded_full().bg(color)),
        4 => branch_marker(theme),
        5 => div()
            .size(px(16.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(2.0))
            .child(div().w(px(12.0)).h(px(1.0)).bg(color))
            .child(div().w(px(7.0)).h(px(1.0)).bg(color)),
        _ => div()
            .size(px(16.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(2.0))
            .child(div().w(px(12.0)).h(px(1.0)).bg(color))
            .child(div().w(px(9.0)).h(px(1.0)).bg(color))
            .child(div().w(px(6.0)).h(px(1.0)).bg(color)),
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

fn filter_icon(theme: &Theme) -> Div {
    div()
        .w(px(13.0))
        .h(px(13.0))
        .flex()
        .items_end()
        .justify_end()
        .child(
            div()
                .size(px(9.0))
                .rounded_full()
                .border_1()
                .border_color(theme.text_faint),
        )
        .child(div().w(px(5.0)).h(px(1.0)).bg(theme.text_faint))
}

fn branch_marker(theme: &Theme) -> Div {
    div()
        .w(px(13.0))
        .h(px(13.0))
        .flex()
        .items_center()
        .gap(px(2.0))
        .child(
            div()
                .size(px(4.0))
                .rounded_full()
                .border_1()
                .border_color(theme.text_faint),
        )
        .child(div().w(px(6.0)).h(px(1.0)).bg(theme.text_faint))
}

fn remote_marker(theme: &Theme) -> Div {
    div()
        .size(px(12.0))
        .rounded_full()
        .border_1()
        .border_color(theme.orange)
}

fn lane_marker(theme: &Theme, lane: usize, merge: bool) -> Div {
    let color = theme.graph_lanes[lane];
    div()
        .w(px(GRAPH_WIDTH))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(2.0))
        .child(div().w(px(1.0)).h_full().bg(color))
        .child(
            div()
                .size(px(if merge { 9.0 } else { 7.0 }))
                .rounded_full()
                .bg(color),
        )
}

impl SourcefourWindow {
    pub(crate) fn new(demo: bool, name: String, path: String) -> Self {
        Self {
            name,
            path,
            demo,
            theme: Theme::dark(),
            sections: SidebarSections::default(),
            history_scroll: ScrollHandle::new(),
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

    fn toolbar(&self) -> impl IntoElement {
        let action = |index: u8, name: &'static str, planned: bool| {
            div()
                .h_full()
                .flex()
                .flex_col()
                .justify_center()
                .items_center()
                .w(px(58.0))
                .text_size(px(10.0))
                .text_color(if planned {
                    self.theme.text_faint
                } else {
                    self.theme.text_secondary
                })
                .hover(|this| this.bg(self.theme.bg_hover))
                .child(toolbar_icon(&self.theme, index))
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
            .child(action(0, "Fetch", false))
            .child(action(1, "Pull", true))
            .child(action(2, "Push", true))
            .child(action(3, "Commit", true))
            .child(action(4, "Branch", false))
            .child(action(5, "Merge", true))
            .child(action(6, "Stash", true))
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
        count: &'static str,
        section: SidebarSection,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let expanded = self.sections.expanded(section);
        div()
            .id(title)
            .h(px(27.0))
            .flex()
            .items_end()
            .justify_between()
            .px(px(14.0))
            .pb(px(4.0))
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

    #[expect(
        clippy::too_many_lines,
        reason = "the fixed M0 sidebar keeps its visual contract together"
    )]
    fn sidebar(&self, cx: &mut gpui::Context<Self>) -> Div {
        if !self.demo {
            return self.loading_sidebar(cx);
        }
        let worktree = |name: &'static str, path: &'static str, current: bool| {
            div()
                .h(px(47.0))
                .flex()
                .flex_col()
                .justify_center()
                .px(px(14.0))
                .bg(if current {
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
                                .child(div().size(px(7.0)).rounded_full().bg(if current {
                                    self.theme.green
                                } else {
                                    self.theme.text_faint
                                }))
                                .child(name),
                        )
                        .child(if current { "CURRENT" } else { "" }),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(self.theme.text_faint)
                        .child(path),
                )
        };
        let branch = |name: &'static str, selected: bool| {
            div()
                .h(px(26.0))
                .flex()
                .items_center()
                .px(px(15.0))
                .gap(px(7.0))
                .bg(if selected {
                    self.theme.bg_selected
                } else {
                    self.theme.bg_panel
                })
                .text_size(px(12.0))
                .text_color(if selected {
                    self.theme.text_primary
                } else {
                    self.theme.text_secondary
                })
                .child(branch_marker(&self.theme))
                .child(name)
        };
        div()
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .bg(self.theme.bg_panel)
            .border_r_1()
            .border_color(self.theme.border)
            .child(self.section("WORKTREES", "3", SidebarSection::Worktrees, cx))
            .when(self.sections.worktrees, |this| {
                this.child(worktree("sourcefour", "~/code/sourcefour", true))
                    .child(worktree(
                        "sourcefour-history",
                        "~/code/sourcefour-history",
                        false,
                    ))
                    .child(worktree("scratch", "~/code/scratch", false))
            })
            .child(self.section("BRANCHES", "5", SidebarSection::Branches, cx))
            .when(self.sections.branches, |this| {
                this.child(branch("main   +2", true))
                    .child(branch("feature/worktrees", false))
                    .child(branch("feature/history", false))
                    .child(branch("release/v1", false))
                    .child(branch("prototype", false))
            })
            .child(self.section("REMOTES", "1", SidebarSection::Remotes, cx))
            .when(self.sections.remotes, |this| {
                this.child(
                    div()
                        .h(px(29.0))
                        .flex()
                        .items_center()
                        .px(px(14.0))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(self.theme.orange)
                        .child(remote_marker(&self.theme))
                        .child("origin")
                        .child(div().flex_grow())
                        .child("git@github.com:helge/sourcefour"),
                )
                .child(
                    div()
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .pl(px(34.0))
                        .text_size(px(12.0))
                        .text_color(self.theme.purple)
                        .child("main"),
                )
                .child(
                    div()
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .pl(px(34.0))
                        .text_size(px(12.0))
                        .text_color(self.theme.cyan)
                        .child("feature/worktrees"),
                )
            })
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
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .bg(self.theme.bg_panel)
            .border_r_1()
            .border_color(self.theme.border)
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
            .child(div().w(px(GRAPH_WIDTH)))
            .child(div().flex_grow().pl(px(10.0)).child("DESCRIPTION"))
            .when(columns.author, |this| {
                this.child(div().w(px(148.0)).child("AUTHOR"))
            })
            .child(div().w(px(96.0)).child("DATE"))
            .when(columns.hash, |this| {
                this.child(div().w(px(74.0)).child("HASH"))
            })
    }

    fn history(&self, columns: ColumnVisibility, cx: &gpui::Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .flex_grow()
            .min_h(px(1.0))
            .child(self.history_rows(columns))
            .child(self.history_scrollbar(cx))
    }

    fn history_rows(&self, columns: ColumnVisibility) -> impl IntoElement {
        if !self.demo {
            return div()
                .id("history-scroll")
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(self.theme.bg_list)
                .text_size(px(12.0))
                .text_color(self.theme.text_faint)
                .child(format!("Loading history from {}...", self.path));
        }
        let rows = &COMMITS[..];
        div()
            .id("history-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.history_scroll)
            .bg(self.theme.bg_list)
            .children(rows.iter().enumerate().map(|(index, commit)| {
                let selected = index == 0;
                div()
                    .h(px(HISTORY_ROW_HEIGHT))
                    .flex_none()
                    .flex()
                    .items_center()
                    .bg(if selected {
                        self.theme.bg_selected
                    } else {
                        self.theme.bg_list
                    })
                    .text_color(self.theme.text_primary)
                    .child(lane_marker(&self.theme, commit.lane, commit.merge))
                    .child(
                        div()
                            .flex_grow()
                            .min_w(px(1.0))
                            .pl(px(10.0))
                            .text_size(px(12.5))
                            .text_color(if commit.merge {
                                self.theme.text_secondary
                            } else {
                                self.theme.text_primary
                            })
                            .child(commit.subject),
                    )
                    .when(columns.author, |this| {
                        this.child(
                            div()
                                .w(px(148.0))
                                .text_size(px(11.5))
                                .text_color(self.theme.text_secondary)
                                .child(commit.author),
                        )
                    })
                    .child(
                        div()
                            .w(px(96.0))
                            .text_size(px(11.5))
                            .text_color(self.theme.text_secondary)
                            .child(commit.date),
                    )
                    .when(columns.hash, |this| {
                        this.child(
                            div()
                                .w(px(74.0))
                                .text_size(px(11.0))
                                .text_color(if selected {
                                    self.theme.accent
                                } else {
                                    self.theme.text_faint
                                })
                                .child(commit.hash),
                        )
                    })
            }))
    }

    fn history_scrollbar(&self, cx: &gpui::Context<Self>) -> impl IntoElement {
        if !self.demo {
            return div().id("history-scrollbar");
        }
        let content_height = COMMITS
            .iter()
            .fold(0.0, |height, _| height + HISTORY_ROW_HEIGHT);
        let measured_height = self.history_scroll.bounds().size.height.0;
        let viewport_height = if measured_height > 0.0 {
            measured_height
        } else {
            390.0
        };
        if content_height <= viewport_height {
            return div().id("history-scrollbar");
        }

        let metrics = scrollbar_metrics(
            content_height,
            viewport_height,
            -self.history_scroll.offset().y.0,
        );
        let Some(metrics) = metrics else {
            return div().id("history-scrollbar");
        };
        let scroll = self.history_scroll.clone();
        let entity = cx.entity();

        div()
            .id("history-scrollbar")
            .absolute()
            .top(px(0.0))
            .right(px(0.0))
            .h_full()
            .w(px(10.0))
            .bg(self.theme.bg_hover)
            .cursor_pointer()
            .on_click(move |event, _, cx| {
                let bounds = scroll.bounds();
                let viewport = bounds.size.height.0;
                if viewport > 0.0 {
                    let percentage =
                        ((event.up.position.y - bounds.origin.y).0 / viewport).clamp(0.0, 1.0);
                    scroll.set_offset(point(px(0.0), px(-metrics.maximum_offset * percentage)));
                    cx.notify(entity.entity_id());
                }
            })
            .child(
                div()
                    .absolute()
                    .top(px(metrics.thumb_top))
                    .right(px(2.0))
                    .w(px(6.0))
                    .h(px(metrics.thumb_height))
                    .rounded_full()
                    .bg(self.theme.text_secondary),
            )
    }

    fn details(&self) -> impl IntoElement {
        if !self.demo {
            return div()
                .h(px(DETAILS_HEIGHT))
                .flex_none()
                .flex()
                .items_center()
                .px(px(14.0))
                .border_t_1()
                .border_color(self.theme.border)
                .bg(self.theme.bg_panel)
                .text_size(px(12.0))
                .text_color(self.theme.text_faint)
                .child(format!("Loading repository metadata for {}...", self.path));
        }
        let hash = COMMITS[0].hash;
        let title = COMMITS[0].subject;
        div().h(px(DETAILS_HEIGHT)).flex_none().flex().flex_col().border_t_1().border_color(self.theme.border).bg(self.theme.bg_panel).px(px(14.0)).pt(px(10.0)).text_color(self.theme.text_primary).child(div().flex().gap(px(10.0)).items_center().text_size(px(12.0)).text_color(self.theme.accent).child(hash).child(div().rounded(px(5.0)).border_1().border_color(self.theme.border).px(px(8.0)).text_size(px(10.5)).text_color(self.theme.text_secondary).child("Copy"))).child(div().pt(px(8.0)).text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).child(title)).child(div().pt(px(4.0)).text_size(px(11.5)).text_color(self.theme.text_secondary).child("Brings linked-worktree enumeration and the metadata sidebar into the shell.")).child(div().mt(px(10.0)).pt(px(8.0)).border_t_1().border_color(self.theme.border).text_size(px(10.0)).font_weight(FontWeight::BOLD).text_color(self.theme.text_faint).child("CHANGED FILES")).child(div().h(px(24.0)).flex().items_center().gap(px(8.0)).text_size(px(11.0)).text_color(self.theme.text_secondary).child("M   apps/sourcefour/src/views.rs").child(div().text_color(self.theme.red).child("-22"))).child(div().h(px(24.0)).flex().items_center().text_size(px(11.0)).text_color(self.theme.text_secondary).child("A   crates/sourcefour-git/src/worktree.rs"))
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
            .child(if self.demo {
                "main +2 / 3 worktrees / 5 branches / 4 remote / 18 commits / demo"
            } else {
                "Loading repository metadata..."
            })
    }
}

impl Render for SourcefourWindow {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let columns = ColumnVisibility::for_window_width(window.viewport_size().width.0);
        div()
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
                    .child(
                        div()
                            .flex_grow()
                            .flex()
                            .flex_col()
                            .min_w(px(1.0))
                            .bg(self.theme.bg_list)
                            .child(self.header(columns))
                            .child(self.history(columns, cx))
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
    use sourcefour_model::{RepoFailure, RepoFailureKind};

    use super::{
        ColumnVisibility, ErrorWindow, SidebarSection, SidebarSections, scrollbar_metrics,
    };

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
    fn scrollbar_metrics_map_offsets_to_the_track() {
        let top = scrollbar_metrics(540.0, 390.0, 0.0).expect("scrollable content");
        let bottom = scrollbar_metrics(540.0, 390.0, 150.0).expect("scrollable content");
        assert!((top.maximum_offset - 150.0).abs() < f32::EPSILON);
        assert!(bottom.thumb_top > top.thumb_top);
        assert_eq!(scrollbar_metrics(390.0, 390.0, 0.0), None);
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
