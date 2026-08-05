//! The diff overlay's rendered-document pane: Markdown, not diff lines.
//!
//! A Markdown file is two things at once — a patch and a document — and the
//! header's Source/Preview toggle picks which one is on screen. The preview
//! reads the same side the diff is showing, parses it off the main thread
//! through `sourcefour_doc`, and resolves every image reference against the
//! repository, so a README renders with the pictures the commit shipped.
//!
//! Nothing here persists: the toggle is per-open-file, and closing the
//! overlay forgets it.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    Div, Font, FontWeight, Hsla, IntoElement, StatefulInteractiveElement, StyledText, TextRun,
    Window, div, prelude::*, px,
};
use sourcefour_doc::{DocBlock, DocBlockKind, DocSpan, DocumentKind};
use sourcefour_git::{DocSource, ImageResolution};
use sourcefour_model::{DiffContent, RepoLocation, RepoPath};

use crate::theme::MONO_FONT;

use super::SourcefourWindow;
use super::diff::{DiffOrigin, DiffView, render_image};

/// What the preview shows when the side being diffed does not carry the
/// document — a deleted file's new side, most often.
const ABSENT_NOTICE: &str = "This document is not present on this side.";

/// Width of the rendered column, so prose stays readable in a wide window.
///
/// Fixed rather than capped: a percentage width clamped by `max_w` never
/// reaches the text measurement, and the paragraphs lay out against the whole
/// panel instead of the column. The narrowest window the app allows still
/// leaves room for this, so nothing is lost by pinning it.
const CONTENT_WIDTH: f32 = 720.0;

/// Padding inside the column, on every side.
const CONTENT_PADDING: f32 = 30.0;

/// Tallest an inline image draws before it is scaled down to fit.
const IMAGE_HEIGHT: f32 = 420.0;

/// One parsed document with every image reference already resolved.
pub(super) struct PreviewState {
    /// The document's blocks, in source order.
    pub(super) blocks: Vec<DocBlock>,
    /// One entry per distinct image reference in `blocks`.
    pub(super) images: HashMap<String, PreviewImage>,
}

/// What one image reference resolved to.
pub(super) enum PreviewImage {
    /// Bytes the renderer can draw.
    Loaded(Arc<gpui::Image>),
    /// An http(s) reference, which the preview never fetches.
    Remote,
    /// Absent from every side, or in a format the renderer cannot decode.
    Missing,
}

/// Whether the open diff could be previewed: a Markdown path whose diff came
/// back as text. Nothing else has a renderer yet.
pub(super) fn applies(view: &DiffView) -> bool {
    matches!(view.content, Some(DiffContent::Text { .. }))
        && DocumentKind::detect(&view.origin.path().0) == Some(DocumentKind::Markdown)
}

/// Whether the preview is what the body should draw right now.
pub(super) fn showing(view: &DiffView) -> bool {
    view.show_preview && applies(view)
}

/// The side of the repository a preview of `origin` reads.
///
/// Always the side the diff itself shows: the commit's own blob, the staged
/// blob for a staged working-tree entry, the file on disk otherwise.
fn doc_source(origin: &DiffOrigin) -> DocSource {
    match origin {
        DiffOrigin::Commit(request) => DocSource::Commit {
            oid: request.oid,
            path: request.path.clone(),
        },
        DiffOrigin::WorkingTree { path, staged: true } => DocSource::Index { path: path.clone() },
        DiffOrigin::WorkingTree { path, .. } => DocSource::Worktree { path: path.clone() },
    }
}

/// The blocks a preview renders for the bytes that side yielded.
///
/// A side without the document — the new side of a deletion — renders one
/// notice rather than an empty pane. Previewing the *old* side instead would
/// need an address for it that a commit diff does not hand out, so v1 says so
/// plainly and leaves that to whoever needs it.
fn document_blocks(bytes: Option<&[u8]>) -> Vec<DocBlock> {
    let Some(bytes) = bytes else {
        return vec![DocBlock {
            kind: DocBlockKind::Paragraph {
                spans: vec![DocSpan {
                    text: String::from(ABSENT_NOTICE),
                    ..DocSpan::default()
                }],
            },
            source_range: 0..0,
        }];
    };
    sourcefour_doc::parse_markdown(&String::from_utf8_lossy(bytes))
}

/// Reads, parses, and resolves one document. Runs off the main thread.
fn load(location: &RepoLocation, source: &DocSource, path: &RepoPath) -> PreviewState {
    let blocks = document_blocks(sourcefour_git::document_bytes(location, source).as_deref());
    let mut images = HashMap::new();
    for reference in sourcefour_doc::image_sources(&blocks) {
        images.entry(reference.to_owned()).or_insert_with(
            || match sourcefour_git::resolve_doc_image(location, source, path, reference) {
                ImageResolution::Found { bytes, format } => render_image(Some(&bytes), &format)
                    .map_or(PreviewImage::Missing, PreviewImage::Loaded),
                ImageResolution::Remote => PreviewImage::Remote,
                ImageResolution::Missing => PreviewImage::Missing,
            },
        );
    }
    PreviewState { blocks, images }
}

/// The pre-resolved preview `--scene preview` seeds (§12.4).
///
/// Parsing is the shipping parser; only the image resolution is faked, since
/// a capture has no repository to resolve against.
pub(super) fn demo_state() -> PreviewState {
    let mut images = HashMap::new();
    images.insert(
        String::from(crate::demo::PREVIEW_IMAGE_PATH),
        render_image(Some(crate::demo::PREVIEW_IMAGE), "png")
            .map_or(PreviewImage::Missing, PreviewImage::Loaded),
    );
    images.insert(
        String::from(crate::demo::PREVIEW_MISSING_IMAGE_PATH),
        PreviewImage::Missing,
    );
    images.insert(
        String::from(crate::demo::PREVIEW_REMOTE_IMAGE_URL),
        PreviewImage::Remote,
    );
    PreviewState {
        blocks: sourcefour_doc::parse_markdown(crate::demo::PREVIEW_MARKDOWN),
        images,
    }
}

impl SourcefourWindow {
    /// Switches the open diff between its lines and its rendered document,
    /// starting the document's load the first time the preview is asked for.
    pub(super) fn toggle_preview(&mut self, show: bool, cx: &mut gpui::Context<Self>) {
        let Some(view) = self.diff_view.as_mut() else {
            return;
        };
        view.show_preview = show;
        if !show || view.preview.is_some() {
            cx.notify();
            return;
        }
        let source = doc_source(&view.origin);
        let path = view.origin.path().clone();
        let Some(location) = self.location.clone() else {
            // No repository to read from — the demo seeds its own preview, so
            // this only happens before discovery finishes.
            view.preview = Some(PreviewState {
                blocks: document_blocks(None),
                images: HashMap::new(),
            });
            cx.notify();
            return;
        };
        // The diff's own token: a preview belongs to the file the overlay was
        // opened for, and opening another retires it.
        let token = self.diff_request;
        cx.spawn(async move |this, cx| {
            let state = cx
                .background_executor()
                .spawn(async move { load(&location, &source, &path) })
                .await;
            this.update(cx, |this, cx| {
                this.set_preview(token, state, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Applies a loaded preview, unless the overlay moved on to another file.
    fn set_preview(&mut self, token: u64, state: PreviewState, cx: &mut gpui::Context<Self>) {
        if self.diff_request != token {
            return;
        }
        if let Some(view) = &mut self.diff_view {
            view.preview = Some(state);
        }
        cx.notify();
    }

    /// The Source/Preview control, left of the layout control it mirrors.
    pub(super) fn preview_toggle(&self, showing: bool, cx: &mut gpui::Context<Self>) -> Div {
        div()
            .flex_none()
            .flex()
            .rounded(px(5.0))
            .border_1()
            .border_color(self.theme.border_strong)
            .overflow_hidden()
            .child(self.preview_button("Source", false, showing, cx))
            .child(self.preview_button("Preview", true, showing, cx))
    }

    /// One segment of the Source/Preview control.
    fn preview_button(
        &self,
        label: &'static str,
        show: bool,
        showing: bool,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        self.segment_button(label, show == showing)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_preview(show, cx);
            }))
    }

    /// The rendered document, scrolling on its own in a centred column.
    pub(super) fn preview_pane(
        &self,
        preview: &PreviewState,
        window: &Window,
    ) -> gpui::Stateful<Div> {
        // The ambient family, so only the runs that mean to change it do.
        let font = window.text_style().font();
        div()
            .id("preview-scroll")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    // Capped, not fixed: beside the source the pane is
                    // narrower than the cap, and clipping reads as a bug.
                    .w_full()
                    .max_w(px(CONTENT_WIDTH))
                    .px(px(CONTENT_PADDING))
                    .py(px(22.0))
                    .flex()
                    .flex_col()
                    .children(
                        preview
                            .blocks
                            .iter()
                            .map(|block| self.preview_block(block, preview, &font)),
                    ),
            )
    }

    /// One rendered block, recursing into the blocks quotes and items hold.
    fn preview_block(&self, block: &DocBlock, preview: &PreviewState, font: &Font) -> Div {
        match &block.kind {
            DocBlockKind::Heading { level, spans } => self.preview_heading(*level, spans, font),
            DocBlockKind::Paragraph { spans } => div()
                .mt(px(8.0))
                .text_size(px(12.5))
                .child(self.preview_spans(spans, font, self.theme.text_secondary)),
            DocBlockKind::Code { language, text } => self.preview_code(language.as_deref(), text),
            DocBlockKind::Quote { blocks } => div()
                .mt(px(10.0))
                .pl(px(12.0))
                .border_l_2()
                .border_color(self.theme.border_strong)
                .flex()
                .flex_col()
                .children(
                    blocks
                        .iter()
                        .map(|block| self.preview_block(block, preview, font)),
                ),
            DocBlockKind::List { ordered, items } => {
                self.preview_list(*ordered, items, preview, font)
            }
            DocBlockKind::Rule => div()
                .mt(px(16.0))
                .mb(px(4.0))
                .h(px(1.0))
                .bg(self.theme.border),
            DocBlockKind::Image { src, alt } => self.preview_image(src, alt, preview),
        }
    }

    /// A heading, sized by its level; the whole line carries the weight so a
    /// bold run inside one changes nothing.
    fn preview_heading(&self, level: u8, spans: &[DocSpan], font: &Font) -> Div {
        let size = match level {
            1 => 20.0,
            2 => 17.0,
            3 => 15.0,
            4 => 13.5,
            5 => 12.5,
            _ => 11.5,
        };
        let mut heading = font.clone();
        heading.weight = FontWeight::SEMIBOLD;
        div()
            .mt(px(20.0))
            .mb(px(2.0))
            .text_size(px(size))
            .child(self.preview_spans(spans, &heading, self.theme.text_primary))
    }

    /// A block of styled runs as one wrapping paragraph.
    ///
    /// Every mark lands on a text run rather than a nested element, because
    /// gpui wraps text inside one element and never across two.
    fn preview_spans(&self, spans: &[DocSpan], font: &Font, color: Hsla) -> StyledText {
        let mut text = String::new();
        let mut runs = Vec::with_capacity(spans.len());
        for span in spans {
            if span.text.is_empty() {
                continue;
            }
            runs.push(self.span_run(span, font, color));
            text.push_str(&span.text);
        }
        StyledText::new(text).with_runs(runs)
    }

    /// One styled run. A run carries font, colour, and decorations but no box,
    /// so inline code gets its wash and its family without the usual padding.
    fn span_run(&self, span: &DocSpan, font: &Font, color: Hsla) -> TextRun {
        let mut font = font.clone();
        if span.code {
            font.family = MONO_FONT.into();
        }
        if span.bold {
            font.weight = FontWeight::SEMIBOLD;
        }
        if span.italic {
            font.style = gpui::FontStyle::Italic;
        }
        let color = if span.link.is_some() {
            self.theme.accent
        } else if span.code {
            self.theme.text_primary
        } else {
            color
        };
        TextRun {
            len: span.text.len(),
            font,
            color,
            background_color: span.code.then_some(self.theme.bg_list),
            underline: span.link.is_some().then(|| gpui::UnderlineStyle {
                thickness: px(1.0),
                color: None,
                wavy: false,
            }),
            strikethrough: span.strike.then(|| gpui::StrikethroughStyle {
                thickness: px(1.0),
                color: None,
            }),
        }
    }

    /// A fenced or indented code block, clipped rather than wrapped: folding
    /// code at an arbitrary column reads worse than losing its right edge.
    fn preview_code(&self, language: Option<&str>, text: &str) -> Div {
        // Tables render as their source until a real table renderer exists;
        // the caption keeps that honest instead of passing them off as code.
        let table = language == Some("table");
        div()
            .mt(px(10.0))
            .flex()
            .flex_col()
            .when(table, |this| {
                this.child(
                    div()
                        .pb(px(3.0))
                        .text_size(px(9.5))
                        .text_color(self.theme.text_faint)
                        .child("table · shown as source"),
                )
            })
            .child(self.preview_code_body(text))
    }

    fn preview_code_body(&self, text: &str) -> Div {
        div()
            .px(px(10.0))
            .py(px(8.0))
            .rounded(px(5.0))
            .bg(self.theme.bg_list)
            .overflow_x_hidden()
            .whitespace_nowrap()
            .font_family(MONO_FONT)
            .text_size(px(11.0))
            .text_color(self.theme.text_secondary)
            .child(text.to_owned())
    }

    /// A list, one marker column beside each item's own blocks.
    fn preview_list(
        &self,
        ordered: bool,
        items: &[Vec<DocBlock>],
        preview: &PreviewState,
        font: &Font,
    ) -> Div {
        div()
            .mt(px(6.0))
            .flex()
            .flex_col()
            .children(items.iter().enumerate().map(|(index, blocks)| {
                let marker = if ordered {
                    format!("{}.", index + 1)
                } else {
                    String::from("•")
                };
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.0))
                    .child(
                        div()
                            // Matched to the first block's own top margin, so
                            // the marker sits on the line it belongs to.
                            .mt(px(8.0))
                            .w(px(16.0))
                            .flex_none()
                            .text_size(px(12.5))
                            .text_color(self.theme.text_faint)
                            .child(marker),
                    )
                    .child(
                        div().flex_1().min_w(px(1.0)).flex().flex_col().children(
                            blocks
                                .iter()
                                .map(|block| self.preview_block(block, preview, font)),
                        ),
                    )
            }))
    }

    /// One image block: the picture when it resolved, a framed note when it
    /// did not, and its alt text beneath either.
    fn preview_image(&self, src: &str, alt: &str, preview: &PreviewState) -> Div {
        let body = match preview.images.get(src) {
            // Both bounds are absolute for the same reason the column is: a
            // relative maximum leaves the image with no width to fit into.
            Some(PreviewImage::Loaded(image)) => gpui::img(image.clone())
                .max_w(px(CONTENT_WIDTH - 2.0 * CONTENT_PADDING))
                .max_h(px(IMAGE_HEIGHT))
                .object_fit(gpui::ObjectFit::Contain)
                .rounded(px(5.0))
                .into_any_element(),
            Some(PreviewImage::Remote) => self
                .preview_placeholder(format!("remote image · {src}"))
                .into_any_element(),
            _ => self
                .preview_placeholder(format!("image not found · {src}"))
                .into_any_element(),
        };
        div()
            .mt(px(12.0))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(4.0))
            .child(body)
            .children((!alt.is_empty()).then(|| {
                div()
                    .text_size(px(10.5))
                    .text_color(self.theme.text_faint)
                    .child(alt.to_owned())
            }))
    }

    /// The frame standing in for an image the preview will not draw.
    fn preview_placeholder(&self, label: String) -> Div {
        div()
            .px(px(12.0))
            .py(px(14.0))
            .rounded(px(5.0))
            .border_1()
            .border_color(self.theme.border)
            .bg(self.theme.bg_chrome)
            .text_size(px(11.0))
            .text_color(self.theme.text_faint)
            .child(label)
    }
}

#[cfg(test)]
mod tests {
    use sourcefour_doc::{DocBlockKind, DocSpan};
    use sourcefour_model::{DiffParent, FileDiffRequest, Oid, RepoPath};

    use super::{ABSENT_NOTICE, DiffOrigin, DocSource, doc_source, document_blocks};

    fn path(text: &str) -> RepoPath {
        RepoPath(text.as_bytes().to_vec())
    }

    #[test]
    fn a_commit_diff_previews_that_commit_s_own_blob() {
        let oid = Oid::from_hex(&format!("{:0<40}", "9f3e21a")).expect("hexadecimal");
        let origin = DiffOrigin::Commit(FileDiffRequest {
            oid,
            parent: DiffParent::FirstParent,
            path: path("README.md"),
        });

        assert!(
            matches!(
                doc_source(&origin),
                DocSource::Commit { oid: read, path: read_path }
                    if read == oid && read_path == path("README.md")
            ),
            "the preview must read the side the diff is showing"
        );
    }

    #[test]
    fn a_working_tree_diff_previews_the_side_that_was_clicked() {
        let origin = |staged| DiffOrigin::WorkingTree {
            path: path("docs/notes.md"),
            staged,
        };

        assert!(matches!(
            doc_source(&origin(true)),
            DocSource::Index { path: read } if read == path("docs/notes.md")
        ));
        assert!(matches!(
            doc_source(&origin(false)),
            DocSource::Worktree { path: read } if read == path("docs/notes.md")
        ));
    }

    #[test]
    fn a_side_without_the_document_renders_a_notice_rather_than_nothing() {
        let blocks = document_blocks(None);

        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0].kind,
            DocBlockKind::Paragraph {
                spans: vec![DocSpan {
                    text: String::from(ABSENT_NOTICE),
                    ..DocSpan::default()
                }],
            },
        );
    }

    #[test]
    fn present_bytes_parse_as_the_document_they_are() {
        let blocks = document_blocks(Some(b"# Title\n\ntext\n"));

        assert_eq!(blocks.len(), 2);
        assert!(matches!(
            blocks[0].kind,
            DocBlockKind::Heading { level: 1, .. }
        ));
    }

    #[test]
    fn the_demo_fixture_exercises_every_block_the_pane_draws() {
        let blocks = super::demo_state().blocks;
        let kinds: Vec<&DocBlockKind> = blocks.iter().map(|block| &block.kind).collect();
        let has = |matcher: fn(&DocBlockKind) -> bool| kinds.iter().any(|kind| matcher(kind));

        assert!(has(|kind| matches!(kind, DocBlockKind::Heading { .. })));
        assert!(has(|kind| matches!(kind, DocBlockKind::Paragraph { .. })));
        assert!(has(|kind| matches!(kind, DocBlockKind::Code { .. })));
        assert!(has(|kind| matches!(kind, DocBlockKind::Quote { .. })));
        assert!(has(|kind| matches!(kind, DocBlockKind::List { .. })));
        assert!(has(|kind| matches!(kind, DocBlockKind::Rule)));
        assert!(has(|kind| matches!(kind, DocBlockKind::Image { .. })));
        assert_eq!(
            sourcefour_doc::image_sources(&blocks).len(),
            3,
            "found, missing, and remote must each have a reference to render"
        );
    }
}
