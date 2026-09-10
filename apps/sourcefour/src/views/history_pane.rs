//! The commit list (§4.4, §8.4): virtualized rows, the header, and the
//! graph canvas painted over the lane column.

use crate::context_menu::{ContextMenuExt as _, PrimaryClickExt as _};
use gpui::{
    Div, FontWeight, IntoElement, UniformListScrollHandle, Window, div, prelude::*, px,
    uniform_list,
};

use crate::{
    graph_paint::{HALO_OPACITY, HALO_RADIUS, NODE_RADIUS, STROKE_WIDTH, Shape, row_shapes},
    history::{HistoryState, display_date},
    panels::Splitter,
    theme::{HEADER_HEIGHT, SPLITTER_WIDTH, Theme},
};

use super::{ColumnVisibility, SourcefourWindow, row_count_as_f32, usize_from_f32};

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
    row_height: f32,
    theme: &Theme,
    window: &mut Window,
) {
    let viewport = bounds.size.height.as_f32();
    let content = row_count_as_f32(history.visible_len()) * row_height;
    // Mirror the list's own clamp so rubber-band overscroll cannot shear the
    // graph away from the rows it annotates.
    let scroll_top = (-scroll.0.borrow().base_handle.offset().y.as_f32())
        .clamp(0.0, (content - viewport).max(0.0));
    let first = usize_from_f32((scroll_top / row_height).floor());
    let last = history
        .visible_len()
        .min(first + usize_from_f32((viewport / row_height).ceil()) + 1);
    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
        for index in first..last {
            let Some(graph) = history.layout_at(index) else {
                break;
            };
            let is_head = history
                .row_at(index)
                .is_some_and(|row| row.labels.iter().any(|label| label.is_head));
            let row_top = row_count_as_f32(index) * row_height - scroll_top;
            for shape in row_shapes(row_top, row_height, graph, is_head) {
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
                        gpui::BorderStyle::default(),
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

impl SourcefourWindow {
    pub(super) fn header(&self, columns: ColumnVisibility) -> impl IntoElement {
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

    pub(super) fn history(&self, columns: ColumnVisibility, cx: &mut gpui::Context<Self>) -> Div {
        div()
            .relative()
            .flex_grow_1()
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

    /// The single absolute canvas painting all visible graph rows (§8.4).
    ///
    /// It carries no listeners, so clicks fall through to the rows beneath it.
    pub(super) fn graph_overlay(
        &self,
        cx: &gpui::Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if self.history.visible_len() == 0 {
            return None;
        }
        let entity = cx.entity();
        let scroll = self.list_scroll.clone();
        let theme = self.theme;
        let row_height = self.settings.history.row_height();
        Some(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, (), window, cx| {
                    paint_graph(
                        bounds,
                        &entity.read(cx).history,
                        &scroll,
                        row_height,
                        &theme,
                        window,
                    );
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
    pub(super) fn history_list(
        &self,
        columns: ColumnVisibility,
        cx: &mut gpui::Context<Self>,
    ) -> Div {
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
                "history",
                self.history.visible_len(),
                cx.processor(move |this, visible: std::ops::Range<usize>, _window, cx| {
                    // Scrolling near the tail is what asks for the next batch.
                    if this.history.wants_more(visible.end) {
                        this.request_batch(cx);
                    }
                    visible
                        .clone()
                        .map(|index| this.commit_row(index, columns, now, cx))
                        .collect()
                }),
            )
            .track_scroll(&self.list_scroll)
            .size_full(),
        )
    }

    /// The row's flexible middle: capped ref labels, the clipped subject,
    /// and the CI dot when one is known.
    fn commit_description_cell(
        &self,
        row: &sourcefour_model::CommitRow,
        cx: &mut gpui::Context<Self>,
    ) -> Div {
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
                    .map(|label| self.label_chip(label, row.oid, cx)),
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
            )
            .children(self.commit_state_dot(row.oid))
    }

    /// One history row: graph cell, subject, author, date, hash.
    pub(super) fn commit_row(
        &self,
        index: usize,
        columns: ColumnVisibility,
        now: i64,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        if index == 0 && self.history.working_tree_row_visible() {
            return self.working_tree_row(cx);
        }
        let Some(row) = self.history.row_at(index) else {
            return div()
                .id(("commit-missing", index))
                .h(px(self.settings.history.row_height()));
        };
        let menus = self.menus.read(cx);
        let selected = self.history.selected_commit() == Some(row.oid)
            || (menus.is_open() && menus.targets(&row.oid.to_hex()));
        let oid = row.oid;
        div()
            .id(("commit", index))
            .debug_selector(|| format!("commit-row-{index}"))
            .on_context_menu(
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    this.show_commit_menu(oid, event.position, window, cx);
                }),
            )
            .h(px(self.settings.history.row_height()))
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
            .child(self.commit_description_cell(row, cx))
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
                    .child(display_date(
                        self.settings.appearance.date_display,
                        now,
                        row.commit_time,
                    )),
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
            .on_primary_click(cx.listener(move |this, _, _, cx| {
                this.initial_selection_pending = false;
                this.history.selected = Some(crate::history::Selection::Commit(oid));
                this.load_selected_files(cx);
                cx.notify();
            }))
    }

    /// The pinned working-tree row: dirty counts, one click from committing.
    fn working_tree_row(&self, cx: &mut gpui::Context<Self>) -> gpui::Stateful<Div> {
        let selected = self.history.selected == Some(crate::history::Selection::WorkingTree);
        let summary = self
            .history
            .working_tree
            .unwrap_or(sourcefour_model::WorkingTreeSummary {
                staged: 0,
                unstaged: 0,
            });
        div()
            .id("working-tree-row")
            .h(px(self.settings.history.row_height()))
            .w_full()
            .flex()
            .items_center()
            .bg(if selected {
                self.theme.bg_selected
            } else {
                self.theme.bg_list
            })
            .hover(|style| style.bg(self.theme.bg_hover))
            .on_primary_click(cx.listener(|this, _, _, cx| {
                this.select_working_tree(cx);
            }))
            .child(div().w(px(self.panels.graph)).h_full().flex_none())
            .child(
                div()
                    .flex_none()
                    .mr(px(8.0))
                    .size(px(7.0))
                    .rounded_full()
                    .bg(self.theme.orange),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(1.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(self.theme.text_primary)
                    .child("Uncommitted changes"),
            )
            .child(
                div()
                    .flex_none()
                    .pr(px(12.0))
                    .text_size(px(11.0))
                    .text_color(self.theme.text_secondary)
                    .child(format!(
                        "{} staged · {} unstaged",
                        summary.staged, summary.unstaged
                    )),
            )
    }

    /// One ref label chip: HEAD, branch, remote branch, or tag (§6.6).
    pub(super) fn label_chip(
        &self,
        label: &sourcefour_model::RefLabel,
        oid: sourcefour_model::Oid,
        cx: &mut gpui::Context<Self>,
    ) -> Div {
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
        let target = label.clone();
        div()
            .debug_selector(|| format!("ref-chip-{}", label.name))
            .on_context_menu(
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    let entries = this.ref_entries(
                        &target.name,
                        target.full_name.as_deref(),
                        target.kind,
                        oid,
                        cx,
                    );
                    this.show_menu(event.position, entries, window, cx);
                }),
            )
            .flex_none()
            .px(px(5.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(self.theme.chip_border(color))
            .bg(self.theme.chip_fill(color))
            .text_size(px(9.5))
            .text_color(color)
            .child(label.name.clone())
    }
}
