//! The full-window diff overlay (§6.11): text unified/split layouts and the
//! image before/after views with the juxtapose slider.

use std::sync::Arc;

use gpui::{
    Div, FontWeight, IntoElement, StatefulInteractiveElement, UniformListScrollHandle, Window, div,
    prelude::*, px, uniform_list,
};
use sourcefour_model::{
    ChangeKind, ChangedFile, DiffContent, DiffLine, DiffLineKind, FileDiffRequest,
};

use crate::theme::MONO_FONT;

use super::{Drag, SourcefourWindow, change_color, change_letter, counted, row_count_as_f32};

/// How the diff overlay lays out its lines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DiffMode {
    Unified,
    Split,
}

impl DiffMode {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Unified => "unified",
            Self::Split => "split",
        }
    }

    pub(super) fn from_name(name: Option<&str>) -> Self {
        match name {
            Some("split") => Self::Split,
            _ => Self::Unified,
        }
    }
}

/// One open file diff: header info plus content once loaded (§6.11).
pub(super) struct DiffView {
    pub(super) title: String,
    pub(super) status: ChangeKind,
    /// `None` while the read is in flight.
    pub(super) content: Option<DiffContent>,
    pub(super) mode: DiffMode,
    /// Side-by-side rows, computed once per content when split is shown.
    pub(super) split: Option<Vec<crate::diff_split::SplitRow>>,
    /// Renderable old-side image, wrapped once per content.
    pub(super) before_image: Option<Arc<gpui::Image>>,
    /// Renderable new-side image, wrapped once per content.
    pub(super) after_image: Option<Arc<gpui::Image>>,
    /// Juxtapose divider position as a fraction of the width.
    pub(super) slider: f32,
}

impl DiffView {
    /// Builds the split pairing when it is needed and not yet cached.
    pub(super) fn ensure_split(&mut self) {
        if self.split.is_some() || self.mode != DiffMode::Split {
            return;
        }
        if let Some(DiffContent::Text { lines }) = &self.content {
            self.split = Some(crate::diff_split::split_rows(lines));
        }
    }

    /// Wraps image bytes for gpui once per loaded content; wrapping per frame
    /// would defeat the renderer's id-keyed image cache.
    pub(super) fn ensure_images(&mut self) {
        if self.before_image.is_some() || self.after_image.is_some() {
            return;
        }
        if let Some(DiffContent::Image {
            before,
            after,
            format,
        }) = &self.content
        {
            self.before_image = render_image(before.as_deref(), format);
            self.after_image = render_image(after.as_deref(), format);
        }
    }

    fn is_image(&self) -> bool {
        matches!(self.content, Some(DiffContent::Image { .. }))
    }
}

/// Wraps encoded image bytes as a gpui image with a fresh cache id.
fn render_image(bytes: Option<&[u8]>, format: &str) -> Option<Arc<gpui::Image>> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);
    let format = match format {
        "png" => gpui::ImageFormat::Png,
        "jpeg" => gpui::ImageFormat::Jpeg,
        "gif" => gpui::ImageFormat::Gif,
        "webp" => gpui::ImageFormat::Webp,
        "bmp" => gpui::ImageFormat::Bmp,
        "tiff" => gpui::ImageFormat::Tiff,
        _ => return None,
    };
    bytes.map(|bytes| {
        Arc::new(gpui::Image {
            format,
            bytes: bytes.to_vec(),
            id: NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed),
        })
    })
}

/// Height of one rendered diff line in either layout.
const DIFF_ROW_HEIGHT: f32 = 20.0;

impl SourcefourWindow {
    /// Closes the diff overlay, returning focus to the history.
    pub(crate) fn close_diff(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.diff_view = None;
        self.focus.focus(window);
        cx.notify();
    }

    /// Opens the diff overlay for one changed file of the selection (§6.11).
    pub(super) fn open_diff(
        &mut self,
        file: &ChangedFile,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(oid) = self.history.selected else {
            return;
        };
        let Some(location) = self.location.clone() else {
            return;
        };
        let path = file
            .new_path
            .as_ref()
            .or(file.old_path.as_ref())
            .cloned()
            .unwrap_or_else(|| sourcefour_model::RepoPath(Vec::new()));
        let request = FileDiffRequest {
            oid,
            parent: self.compare_parent,
            path,
        };
        self.diff_view = Some(DiffView {
            title: request.path.display_lossy(),
            status: file.status,
            content: None,
            mode: self.preferred_diff_mode,
            split: None,
            before_image: None,
            after_image: None,
            slider: 0.5,
        });
        self.diff_request += 1;
        let token = self.diff_request;
        self.diff_scroll = UniformListScrollHandle::new();
        self.diff_focus.focus(window);
        cx.spawn(async move |this, cx| {
            let diff = cx
                .background_executor()
                .spawn(async move { sourcefour_git::file_diff(&location, &request) })
                .await;
            this.update(cx, |this, cx| {
                if this.diff_request != token {
                    return;
                }
                if let Some(view) = &mut this.diff_view {
                    view.content = Some(match diff {
                        Ok(diff) => diff.content,
                        Err(failure) => DiffContent::Unavailable {
                            message: failure.user.message,
                        },
                    });
                    view.split = None;
                    view.ensure_split();
                    view.before_image = None;
                    view.after_image = None;
                    view.ensure_images();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The full-window diff overlay (§6.11), closed by Escape, ✕, or a click
    /// outside the panel.
    pub(super) fn diff_overlay(
        &self,
        cx: &mut gpui::Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let view = self.diff_view.as_ref()?;
        let line_count = match &view.content {
            Some(DiffContent::Text { lines }) => lines.len(),
            _ => 0,
        };
        let body = self.diff_body(view, line_count, cx);
        Some(
            super::modal_backdrop("diff-overlay", &self.theme)
                .key_context("Diff")
                .track_focus(&self.diff_focus)
                // Occlusion also stops the root's handlers, so scrub drags
                // route here.
                .on_mouse_move(
                    cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                        this.drag_move(event.position.x.0, event.position.y.0, window, cx);
                    }),
                )
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.end_drag(cx);
                    }),
                )
                .p(px(26.0))
                .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                    if super::is_true_click(event) {
                        this.close_diff(window, cx);
                    }
                }))
                .child(
                    super::modal_panel("diff-panel", &self.theme)
                        .flex_1()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        // Clicks inside the panel must not fall through to the
                        // backdrop's close handler.
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(self.diff_header(view, line_count, cx))
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_h(px(1.0))
                                .bg(self.theme.bg_list)
                                .child(body)
                                .children(self.diff_scrollbar(cx)),
                        ),
                ),
        )
    }

    /// The overlay's content area for the current mode and content kind.
    pub(super) fn diff_body(
        &self,
        view: &DiffView,
        line_count: usize,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        match &view.content {
            None => self.diff_notice("Computing diff…").into_any_element(),
            Some(DiffContent::Text { lines }) if lines.is_empty() => {
                self.diff_notice("No textual changes.").into_any_element()
            }
            Some(DiffContent::Text { .. }) if view.mode == DiffMode::Split => uniform_list(
                cx.entity(),
                "diff-split-rows",
                view.split.as_ref().map_or(0, Vec::len),
                move |this, range, _window, _cx| {
                    let Some(rows) = this.diff_view.as_ref().and_then(|view| view.split.as_ref())
                    else {
                        return Vec::new();
                    };
                    range
                        .filter_map(|index| rows.get(index).cloned())
                        .map(|row| this.split_row_view(&row))
                        .collect()
                },
            )
            .track_scroll(self.diff_scroll.clone())
            .size_full()
            .into_any_element(),
            Some(DiffContent::Text { .. }) => uniform_list(
                cx.entity(),
                "diff-lines",
                line_count,
                move |this, range, _window, _cx| {
                    let Some(DiffContent::Text { lines }) = this
                        .diff_view
                        .as_ref()
                        .and_then(|view| view.content.as_ref())
                    else {
                        return Vec::new();
                    };
                    range
                        .filter_map(|index| lines.get(index).cloned())
                        .map(|line| this.diff_line_row(&line))
                        .collect()
                },
            )
            .track_scroll(self.diff_scroll.clone())
            .size_full()
            .into_any_element(),
            Some(DiffContent::Image { .. }) if view.mode == DiffMode::Split => {
                self.image_split_view(view).into_any_element()
            }
            Some(DiffContent::Image { .. }) => self.image_slider_view(view, cx).into_any_element(),
            Some(DiffContent::Binary { message } | DiffContent::Unavailable { message }) => {
                self.diff_notice(message.clone()).into_any_element()
            }
            Some(DiffContent::TooLarge { lines, bytes, .. }) => self
                .diff_notice(format!(
                    "Diff too large to render safely ({lines} lines, {bytes} bytes) — \
                     open it with an external tool."
                ))
                .into_any_element(),
        }
    }

    /// The diff overlay's title bar: status, path, counts, and close.
    pub(super) fn diff_header(
        &self,
        view: &DiffView,
        line_count: usize,
        cx: &mut gpui::Context<Self>,
    ) -> Div {
        let status_color = change_color(&self.theme, view.status);
        div()
            .h(px(40.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(14.0))
            .border_b_1()
            .border_color(self.theme.border)
            .bg(self.theme.bg_chrome)
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(12.0))
                    .text_color(status_color)
                    .child(change_letter(view.status)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(1.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(12.5))
                    .text_color(self.theme.text_primary)
                    .child(view.title.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(self.theme.border_strong)
                    .overflow_hidden()
                    .child(self.diff_mode_button(
                        if view.is_image() { "Slider" } else { "Unified" },
                        DiffMode::Unified,
                        view.mode,
                        cx,
                    ))
                    .child(self.diff_mode_button(
                        if view.is_image() {
                            "Side by side"
                        } else {
                            "Split"
                        },
                        DiffMode::Split,
                        view.mode,
                        cx,
                    )),
            )
            .children((line_count > 0).then(|| {
                div()
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(self.theme.text_faint)
                    .child(counted(line_count, "line"))
            }))
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(self.theme.text_faint)
                    .child("Esc"),
            )
            .child(
                div()
                    .id("diff-close")
                    .flex_none()
                    .px(px(8.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .text_size(px(13.0))
                    .text_color(self.theme.text_secondary)
                    .hover(|style| style.bg(self.theme.bg_hover))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_diff(window, cx);
                    }))
                    .child("✕"),
            )
    }

    /// One segment of the unified/split toggle.
    pub(super) fn diff_mode_button(
        &self,
        label: &'static str,
        mode: DiffMode,
        active: DiffMode,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        let selected = mode == active;
        div()
            .id(label)
            .px(px(9.0))
            .py(px(2.0))
            .cursor_pointer()
            .text_size(px(10.5))
            .bg(if selected {
                self.theme.bg_selected
            } else {
                self.theme.bg_list
            })
            .text_color(if selected {
                self.theme.text_primary
            } else {
                self.theme.text_faint
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(view) = &mut this.diff_view {
                    view.mode = mode;
                    view.ensure_split();
                }
                this.preferred_diff_mode = mode;
                this.persist_ui_state(cx);
                cx.notify();
            }))
            .child(label)
    }

    /// Side-by-side before/after panes for an image comparison (§6.11).
    pub(super) fn image_split_view(&self, view: &DiffView) -> Div {
        div()
            .size_full()
            .flex()
            .child(self.image_pane("Before", view.before_image.clone(), "Added — no before"))
            .child(div().w(px(1.0)).flex_none().h_full().bg(self.theme.border))
            .child(self.image_pane("After", view.after_image.clone(), "Deleted — no after"))
    }

    /// One labeled half of the side-by-side image view.
    pub(super) fn image_pane(
        &self,
        label: &'static str,
        image: Option<Arc<gpui::Image>>,
        missing: &'static str,
    ) -> Div {
        div()
            .flex_1()
            .min_w(px(1.0))
            .h_full()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .p(px(28.0))
            .child(match image {
                Some(image) => gpui::img(image)
                    .size_full()
                    .object_fit(gpui::ObjectFit::Contain)
                    .into_any_element(),
                None => div()
                    .text_size(px(12.0))
                    .text_color(self.theme.text_faint)
                    .child(missing)
                    .into_any_element(),
            })
            .child(self.image_side_chips(&[label]))
    }

    /// A small chip naming an image side.
    pub(super) fn image_side_chip(&self, label: &'static str) -> Div {
        div()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(4.0))
            .bg(self.theme.hud())
            .text_size(px(9.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(self.theme.text_faint)
            .child(label)
    }

    /// The chip layer over an image view.
    ///
    /// Chips are placed by this row rather than by their own insets: an
    /// absolute element inherits a zero inset on every side it does not set,
    /// so a right-anchored chip would stretch the full width and draw its
    /// text back on the left.
    pub(super) fn image_side_chips(&self, labels: &[&'static str]) -> Div {
        div()
            .absolute()
            .top(px(8.0))
            .left(px(8.0))
            .right(px(8.0))
            .flex()
            .justify_between()
            .children(labels.iter().map(|label| self.image_side_chip(label)))
    }

    /// The juxtapose view: after fills the area, before overlays it clipped to
    /// the slider fraction, and dragging anywhere moves the divider.
    pub(super) fn image_slider_view(
        &self,
        view: &DiffView,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        let fraction = view.slider;
        let bounds_cell = self.juxtapose_bounds.clone();
        let divider = if self.drag == Some(Drag::ImageSlider) {
            self.theme.text_secondary
        } else {
            self.theme.border_strong
        };
        div()
            .id("image-juxtapose")
            .size_full()
            .relative()
            .overflow_hidden()
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    this.drag = Some(Drag::ImageSlider);
                    this.scrub_image(event.position.x.0);
                    cx.notify();
                }),
            )
            .on_click(cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                if event.down.click_count == 2
                    && let Some(view) = &mut this.diff_view
                {
                    view.slider = 0.5;
                    cx.notify();
                }
            }))
            .child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, (), _, _| bounds_cell.set(bounds),
                )
                .absolute()
                .inset_0(),
            )
            .child(Self::image_layer(view.after_image.clone()).inset_0())
            .child(
                // The before side, clipped at the divider. The inner layer is
                // re-widened by the reciprocal so the image keeps the full
                // area's geometry and only the reveal changes.
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(gpui::relative(fraction))
                    .overflow_hidden()
                    .child(
                        Self::image_layer(view.before_image.clone())
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(gpui::relative(1.0 / fraction))
                            .bg(self.theme.bg_list),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(gpui::relative(fraction))
                    .ml(px(-1.0))
                    .w(px(2.0))
                    .bg(divider),
            )
            .child(
                div()
                    .absolute()
                    .top(gpui::relative(0.5))
                    .left(gpui::relative(fraction))
                    .ml(px(-9.0))
                    .mt(px(-9.0))
                    .size(px(18.0))
                    .rounded_full()
                    .border_1()
                    .border_color(divider)
                    .bg(self.theme.bg_chrome)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(8.0))
                    .text_color(self.theme.text_secondary)
                    .child("↔"),
            )
            .child(self.image_side_chips(&["Before", "After"]))
    }

    /// An absolutely positioned layer holding one centered, contained image.
    pub(super) fn image_layer(image: Option<Arc<gpui::Image>>) -> Div {
        div()
            .absolute()
            .flex()
            .items_center()
            .justify_center()
            .p(px(28.0))
            .children(image.map(|image| {
                gpui::img(image)
                    .size_full()
                    .object_fit(gpui::ObjectFit::Contain)
            }))
    }

    /// Maps a window-space X onto the juxtapose slider fraction.
    pub(super) fn scrub_image(&mut self, x: f32) {
        let bounds = self.juxtapose_bounds.get();
        let width = bounds.size.width.0;
        if width <= 0.0 {
            return;
        }
        let fraction = ((x - bounds.origin.x.0) / width).clamp(0.02, 0.98);
        if let Some(view) = &mut self.diff_view {
            view.slider = fraction;
        }
    }

    /// One visual row of the side-by-side view.
    pub(super) fn split_row_view(&self, row: &crate::diff_split::SplitRow) -> Div {
        if let Some(hunk) = &row.hunk {
            return div()
                .h(px(20.0))
                .w_full()
                .flex()
                .items_center()
                .px(px(10.0))
                .bg(self.theme.bg_hover)
                .font_family(MONO_FONT)
                .text_size(px(11.0))
                .text_color(self.theme.accent)
                .child(hunk.clone());
        }
        div()
            .h(px(20.0))
            .w_full()
            .flex()
            .child(self.split_half(row.left.as_ref(), false))
            .child(div().w(px(1.0)).flex_none().h_full().bg(self.theme.border))
            .child(self.split_half(row.right.as_ref(), true))
    }

    /// One half of a split row: number, marker tint, and content.
    pub(super) fn split_half(
        &self,
        side: Option<&crate::diff_split::SplitSide>,
        right: bool,
    ) -> Div {
        let base = div().flex_1().min_w(px(1.0)).h_full().flex().items_center();
        let rule = || {
            div()
                .w(px(1.0))
                .flex_none()
                .h_full()
                .bg(self.theme.gutter_rule())
        };
        let Some(side) = side else {
            return base
                .bg(self.theme.bg_panel.opacity(0.4))
                .child(div().w(px(44.0)).flex_none().h_full())
                .child(rule());
        };
        let (text_color, background) = match side.kind {
            DiffLineKind::Addition if right => (self.theme.green, Some(self.theme.tint_added())),
            DiffLineKind::Deletion if !right => (self.theme.red, Some(self.theme.tint_removed())),
            _ => (self.theme.text_secondary, None),
        };
        base.when_some(background, gpui::Styled::bg)
            .child(
                div()
                    .w(px(44.0))
                    .flex_none()
                    .pr(px(6.0))
                    .font_family(MONO_FONT)
                    .text_size(px(10.5))
                    .text_color(self.theme.text_faint)
                    .child(
                        side.number
                            .map_or_else(String::new, |number| number.to_string()),
                    ),
            )
            .child(rule())
            .child(
                div()
                    .flex_1()
                    .min_w(px(1.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_family(MONO_FONT)
                    .text_size(px(11.0))
                    .text_color(text_color)
                    .child(side.text.clone()),
            )
    }

    /// Rows currently shown by the diff overlay's list, either layout.
    pub(super) fn diff_rows_len(&self) -> usize {
        let Some(view) = &self.diff_view else {
            return 0;
        };
        match (&view.content, view.mode) {
            (Some(DiffContent::Text { .. }), DiffMode::Split) => {
                view.split.as_ref().map_or(0, Vec::len)
            }
            (Some(DiffContent::Text { lines }), DiffMode::Unified) => lines.len(),
            _ => 0,
        }
    }

    /// A draggable scrollbar for scrubbing through large diffs.
    pub(super) fn diff_scrollbar(
        &self,
        cx: &mut gpui::Context<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        let rows = self.diff_rows_len();
        let handle = self.diff_scroll.0.borrow();
        let bounds = handle.base_handle.bounds();
        let viewport = bounds.size.height.0;
        let content = row_count_as_f32(rows) * DIFF_ROW_HEIGHT;
        if viewport <= 0.0 || content <= viewport {
            return None;
        }
        let offset = (-handle.base_handle.offset().y.0).clamp(0.0, content - viewport);
        drop(handle);
        let thumb = (viewport * viewport / content).clamp(30.0, viewport);
        let top = offset / (content - viewport) * (viewport - thumb);
        Some(
            div()
                .id("diff-scrollbar")
                .absolute()
                .top_0()
                .right_0()
                .h_full()
                .w(px(12.0))
                .cursor_pointer()
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                        this.drag = Some(Drag::DiffBar);
                        this.scrub_diff(event.position.y.0);
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(top))
                        .right(px(2.0))
                        .w(px(8.0))
                        .h(px(thumb))
                        .rounded_full()
                        .bg(if self.drag == Some(Drag::DiffBar) {
                            self.theme.accent
                        } else {
                            self.theme.border_strong
                        }),
                ),
        )
    }

    /// Maps a window-space Y onto the diff list's scroll offset.
    pub(super) fn scrub_diff(&mut self, y: f32) {
        let rows = self.diff_rows_len();
        let handle = self.diff_scroll.0.borrow();
        let bounds = handle.base_handle.bounds();
        let viewport = bounds.size.height.0;
        let content = row_count_as_f32(rows) * DIFF_ROW_HEIGHT;
        if viewport <= 0.0 || content <= viewport {
            return;
        }
        let fraction = ((y - bounds.origin.y.0) / viewport).clamp(0.0, 1.0);
        let offset = fraction * (content - viewport);
        handle
            .base_handle
            .set_offset(gpui::point(px(0.0), px(-offset)));
    }

    /// A centered message replacing diff lines when there are none to show.
    pub(super) fn diff_notice(&self, message: impl Into<gpui::SharedString>) -> Div {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .px(px(30.0))
            .text_size(px(12.0))
            .text_color(self.theme.text_faint)
            .child(message.into())
    }

    /// One rendered diff line: numbers, marker, and tinted content.
    pub(super) fn diff_line_row(&self, line: &DiffLine) -> Div {
        let (marker, text_color, background) = match line.kind {
            DiffLineKind::Addition => ("+", self.theme.green, Some(self.theme.tint_added())),
            DiffLineKind::Deletion => ("-", self.theme.red, Some(self.theme.tint_removed())),
            DiffLineKind::Hunk => ("", self.theme.accent, Some(self.theme.bg_hover)),
            DiffLineKind::Meta | DiffLineKind::Marker => ("", self.theme.text_faint, None),
            DiffLineKind::Context => (" ", self.theme.text_secondary, None),
        };
        let number = |value: Option<u32>| {
            div()
                .w(px(44.0))
                .flex_none()
                .pr(px(6.0))
                .font_family(MONO_FONT)
                .text_size(px(10.5))
                .text_color(self.theme.text_faint)
                .child(value.map_or_else(String::new, |value| value.to_string()))
        };
        // The gutter rule pauses on hunk headers, which read as full-width
        // banners rather than numbered lines.
        let rule = if matches!(line.kind, DiffLineKind::Hunk) {
            gpui::transparent_black()
        } else {
            self.theme.gutter_rule()
        };
        div()
            .h(px(20.0))
            .w_full()
            .flex()
            .items_center()
            .when_some(background, gpui::Styled::bg)
            .child(number(line.old_line))
            .child(number(line.new_line))
            .child(div().w(px(1.0)).flex_none().h_full().bg(rule))
            .child(
                div()
                    .w(px(14.0))
                    .flex_none()
                    .font_family(MONO_FONT)
                    .text_size(px(11.0))
                    .text_color(text_color)
                    .child(marker),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(1.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_family(MONO_FONT)
                    .text_size(px(11.0))
                    .text_color(text_color)
                    .child(line.text.clone()),
            )
    }
}
