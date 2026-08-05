//! The diff overlay's rendered-document pane: Markdown, not diff lines.
//!
//! A Markdown file is two things at once — a patch and a document — and the
//! header's Source/Preview toggle picks which one is on screen. The preview
//! reads the same side the diff is showing, parses it off the main thread
//! through `sourcefour_doc`, and resolves every image reference against the
//! repository, so a README renders with the pictures the commit shipped.
//!
//! Each pane is a list of top-level blocks rather than one tall column, so a
//! frame costs the blocks on screen and a long README scrolls like a short
//! one.
//!
//! Nothing here persists: the toggle is per-open-file, and closing the
//! overlay forgets it.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    Div, Font, FontWeight, Hsla, IntoElement, StatefulInteractiveElement, StyledText, TextRun,
    Window, div, prelude::*, px,
};
use sourcefour_doc::{CellAlignment, DocBlock, DocBlockKind, DocSpan, DocumentKind};
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

/// Padding above the document's first block and below its last.
const COLUMN_PADDING_Y: f32 = 22.0;

/// Tallest an inline image draws before it is scaled down to fit.
const IMAGE_HEIGHT: f32 = 420.0;

/// How far past the visible pane the list measures blocks, so scrolling
/// reaches already-measured content instead of popping.
const OVERDRAW: f32 = 400.0;

/// The hover group every code fence joins, so a fence can reveal its own
/// copy control without each one inventing a name.
const CODE_GROUP: &str = "preview-code";

/// Ordinals a block may have at one nesting level before its ids alias its
/// parent's. Documents the pane draws stay far under it; a list of a thousand
/// items would have to nest inside a single top-level block to collide.
const NESTED_STRIDE: usize = 1000;

/// One parsed document with every image reference already resolved.
pub(super) struct PreviewState {
    /// The diff's old side, rendered.
    pub(super) old: PreviewDoc,
    /// The diff's new side, rendered.
    pub(super) new: PreviewDoc,
}

/// Which document a pane draws.
///
/// Also names the pane, because both panes are on screen at once and the
/// element ids under them must not collide.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum PreviewSide {
    Old,
    New,
}

impl PreviewSide {
    /// The pane's element id, and the root every id below it extends.
    fn pane_id(self) -> &'static str {
        match self {
            Self::Old => "preview-old",
            Self::New => "preview-new",
        }
    }
}

/// One block's address inside one pane: the list item that draws it, and
/// where it sits under that item.
///
/// The walk is deterministic, so the same block keeps the same address across
/// frames — which is what an element id and, later, a selection need. Nested
/// ordinals fold into one number by [`NESTED_STRIDE`], and the item's own
/// element id scopes the result, so an address is unique in its pane.
#[derive(Clone, Copy)]
pub(super) struct BlockId {
    /// The top-level block this one sits under — the list item's index.
    item: usize,
    /// Where under that item the block sits; `0` is the item's own block.
    nested: usize,
}

impl BlockId {
    /// The address of one top-level block: the list item itself.
    fn top(item: usize) -> Self {
        Self { item, nested: 0 }
    }

    /// The address of the `ordinal`th block directly inside this one.
    fn child(self, ordinal: usize) -> Self {
        Self {
            nested: self.nested * NESTED_STRIDE + ordinal + 1,
            ..self
        }
    }

    /// The list item's element id, which scopes every id under it.
    fn item_id(self) -> gpui::ElementId {
        ("preview-item", self.item).into()
    }

    /// The element id of one control on this block. Unique in the pane
    /// because [`Self::item_id`] scopes it.
    fn element(self, name: &'static str) -> gpui::ElementId {
        (name, self.nested).into()
    }
}

/// One side's document as the background load hands it over.
///
/// Separate from [`PreviewDoc`] because a `ListState` is `Rc`-backed and
/// cannot cross threads: the parse travels, the list is built where it will
/// be drawn.
pub(super) struct PreviewParse {
    /// The document's blocks, in source order.
    blocks: Vec<DocBlock>,
    /// One entry per distinct image reference in `blocks`.
    images: HashMap<String, PreviewImage>,
}

/// One side's document: its blocks, the images they resolved to, and the
/// list that draws them.
pub(super) struct PreviewDoc {
    /// The document's blocks, in source order.
    pub(super) blocks: Vec<DocBlock>,
    /// One entry per distinct image reference in `blocks`.
    pub(super) images: HashMap<String, PreviewImage>,
    /// One list item per top-level block, so a frame builds the blocks on
    /// screen instead of the document. Holds the scroll position, so it is
    /// built once per loaded document and never per frame.
    list: gpui::ListState,
}

impl PreviewDoc {
    /// Virtualizes one parsed side for the pane that will draw it.
    ///
    /// The list renders through the window entity rather than a copy of the
    /// document, so a reloaded preview draws the blocks it just loaded.
    fn new(
        parse: PreviewParse,
        side: PreviewSide,
        cx: &mut gpui::Context<SourcefourWindow>,
    ) -> Self {
        let view = cx.entity().downgrade();
        Self {
            list: gpui::ListState::new(
                parse.blocks.len(),
                gpui::ListAlignment::Top,
                px(OVERDRAW),
                move |index, window, cx| {
                    view.upgrade().map_or_else(
                        || div().into_any_element(),
                        |view| view.read(cx).preview_item(side, index, window),
                    )
                },
            ),
            blocks: parse.blocks,
            images: parse.images,
        }
    }
}

/// What one image reference resolved to.
#[derive(Clone)]
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

/// The new side of `origin`: the commit's own blob, the staged blob for a
/// staged working-tree entry, the file on disk otherwise.
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

/// The old side of `origin`: the parent commit's blob, HEAD's blob for a
/// staged entry, the index blob for an unstaged one. `None` when there is no
/// old side — a root commit, or a staged file on an unborn HEAD.
fn old_doc_source(
    location: &RepoLocation,
    origin: &DiffOrigin,
    head: Option<sourcefour_model::Oid>,
) -> Option<DocSource> {
    match origin {
        DiffOrigin::Commit(request) => {
            sourcefour_git::parent_commit_oid(location, request.oid, request.parent).map(|oid| {
                DocSource::Commit {
                    oid,
                    path: request.path.clone(),
                }
            })
        }
        DiffOrigin::WorkingTree { path, staged: true } => head.map(|oid| DocSource::Commit {
            oid,
            path: path.clone(),
        }),
        DiffOrigin::WorkingTree { path, .. } => Some(DocSource::Index { path: path.clone() }),
    }
}

/// The blocks a preview renders for the bytes that side yielded.
///
/// A side without the document — the new side of a deletion, the old side of
/// an addition — renders one notice rather than an empty pane.
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

/// Reads, parses, and resolves both sides, old first. Runs off the main
/// thread, which is why it stops at the parse.
fn load(
    location: &RepoLocation,
    origin: &DiffOrigin,
    head: Option<sourcefour_model::Oid>,
) -> (PreviewParse, PreviewParse) {
    let path = origin.path().clone();
    (
        load_side(
            location,
            old_doc_source(location, origin, head).as_ref(),
            &path,
        ),
        load_side(location, Some(&doc_source(origin)), &path),
    )
}

/// One side's document, parsed and with every image reference resolved
/// against that side's own tree.
fn load_side(location: &RepoLocation, source: Option<&DocSource>, path: &RepoPath) -> PreviewParse {
    let bytes = source.and_then(|source| sourcefour_git::document_bytes(location, source));
    let blocks = document_blocks(bytes.as_deref());
    let mut images = HashMap::new();
    if let Some(source) = source {
        for reference in sourcefour_doc::image_sources(&blocks) {
            images.entry(reference.to_owned()).or_insert_with(|| {
                match sourcefour_git::resolve_doc_image(location, source, path, reference) {
                    ImageResolution::Found { bytes, format } => render_image(Some(&bytes), &format)
                        .map_or(PreviewImage::Missing, PreviewImage::Loaded),
                    ImageResolution::Remote => PreviewImage::Remote,
                    ImageResolution::Missing => PreviewImage::Missing,
                }
            });
        }
    }
    PreviewParse { blocks, images }
}

/// §12.5 preview probe; prints only under `SOURCEFOUR_FRAME_LOG`.
///
/// `started` must be taken where the panes' elements begin building, so the
/// line covers the work one frame does for the rendered document and nothing
/// else around it.
pub(super) fn build_probe(started: std::time::Instant, blocks: usize) {
    if std::env::var_os("SOURCEFOUR_FRAME_LOG").is_some() {
        eprintln!(
            "preview-build-ms {:.3} blocks {blocks}",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
}

/// The demo preview's new side, the fixture repeated as many times as
/// `SOURCEFOUR_PREVIEW_STRESS` asks for.
///
/// A deterministic stress body for the ledger (§12.5): the same document over
/// and over means the block count is exactly N times the fixture's, so two
/// runs at the same N measure the same document.
fn stress_body() -> String {
    let repeats = stress_repeats(std::env::var("SOURCEFOUR_PREVIEW_STRESS").ok().as_deref());
    vec![crate::demo::PREVIEW_MARKDOWN; repeats].join("\n\n")
}

/// How many times [`stress_body`] repeats the fixture. Absent, unreadable,
/// and below one all mean the document as written.
fn stress_repeats(value: Option<&str>) -> usize {
    value
        .and_then(|value| value.parse().ok())
        .filter(|&repeats| repeats >= 1)
        .unwrap_or(1)
}

/// The pre-resolved preview `--scene preview` seeds (§12.4), virtualized for
/// the panes that will draw it.
pub(super) fn demo_state(cx: &mut gpui::Context<SourcefourWindow>) -> PreviewState {
    let (old, new) = demo_parse();
    PreviewState {
        old: PreviewDoc::new(old, PreviewSide::Old, cx),
        new: PreviewDoc::new(new, PreviewSide::New, cx),
    }
}

/// Both sides of the capture fixture, old first.
///
/// Parsing is the shipping parser; only the image resolution is faked, since
/// a capture has no repository to resolve against.
fn demo_parse() -> (PreviewParse, PreviewParse) {
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
    (
        PreviewParse {
            blocks: sourcefour_doc::parse_markdown(crate::demo::PREVIEW_MARKDOWN_OLD),
            images: images.clone(),
        },
        PreviewParse {
            blocks: sourcefour_doc::parse_markdown(&stress_body()),
            images,
        },
    )
}

/// The rendered document: one list item per top-level block, so a frame
/// costs what is on screen rather than what the document holds.
///
/// The pane's own id scopes every element id the blocks under it build.
pub(super) fn pane(preview: &PreviewDoc, side: PreviewSide) -> gpui::Stateful<Div> {
    div()
        .id(side.pane_id())
        .size_full()
        .child(gpui::list(preview.list.clone()).size_full())
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
        let origin = view.origin.clone();
        let Some(location) = self.location.clone() else {
            // No repository to read from — the demo seeds its own preview, so
            // this only happens before discovery finishes.
            let absent = || PreviewParse {
                blocks: document_blocks(None),
                images: HashMap::new(),
            };
            view.preview = Some(PreviewState {
                old: PreviewDoc::new(absent(), PreviewSide::Old, cx),
                new: PreviewDoc::new(absent(), PreviewSide::New, cx),
            });
            cx.notify();
            return;
        };
        // The old side of a staged entry is HEAD, which only the snapshot
        // knows; resolve it here so the background load needs no ref reads.
        let head = self.snapshot().and_then(|snapshot| match &snapshot.head {
            sourcefour_model::HeadSnapshot::Branch { oid, .. }
            | sourcefour_model::HeadSnapshot::Detached { oid } => Some(*oid),
            _ => None,
        });
        // The diff's own token: a preview belongs to the file the overlay was
        // opened for, and opening another retires it.
        let token = self.diff_request;
        cx.spawn(async move |this, cx| {
            let parse = cx
                .background_executor()
                .spawn(async move { load(&location, &origin, head) })
                .await;
            this.update(cx, |this, cx| {
                this.set_preview(token, parse, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Virtualizes a loaded preview and applies it, unless the overlay moved
    /// on to another file. The lists are built here, on the main thread, and
    /// a preview loaded again starts at the top with fresh ones.
    fn set_preview(
        &mut self,
        token: u64,
        parse: (PreviewParse, PreviewParse),
        cx: &mut gpui::Context<Self>,
    ) {
        if self.diff_request != token {
            return;
        }
        let (old, new) = parse;
        let state = PreviewState {
            old: PreviewDoc::new(old, PreviewSide::Old, cx),
            new: PreviewDoc::new(new, PreviewSide::New, cx),
        };
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

    /// One list item: a top-level block in the centred column.
    ///
    /// The column lives here rather than around the list, since the list is
    /// what fills the pane and scrolls.
    fn preview_item(&self, side: PreviewSide, index: usize, window: &Window) -> gpui::AnyElement {
        let Some(preview) = self.preview_doc(side) else {
            return div().into_any_element();
        };
        let Some(block) = preview.blocks.get(index) else {
            return div().into_any_element();
        };
        // The ambient family, so only the runs that mean to change it do.
        let font = window.text_style().font();
        let id = BlockId::top(index);
        div()
            // Scopes every element id the block below builds.
            .id(id.item_id())
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
                    // The document's own top and bottom, not every block's.
                    .when(index == 0, |column| column.pt(px(COLUMN_PADDING_Y)))
                    .when(index + 1 == preview.blocks.len(), |column| {
                        column.pb(px(COLUMN_PADDING_Y))
                    })
                    .flex()
                    .flex_col()
                    .child(self.preview_block(block, preview, &font, id)),
            )
            .into_any_element()
    }

    /// The document one pane draws, while a preview is open.
    fn preview_doc(&self, side: PreviewSide) -> Option<&PreviewDoc> {
        let preview = self.diff_view.as_ref()?.preview.as_ref()?;
        Some(match side {
            PreviewSide::Old => &preview.old,
            PreviewSide::New => &preview.new,
        })
    }

    /// One rendered block, recursing into the blocks quotes and items hold.
    fn preview_block(
        &self,
        block: &DocBlock,
        preview: &PreviewDoc,
        font: &Font,
        id: BlockId,
    ) -> Div {
        match &block.kind {
            DocBlockKind::Heading { level, spans } => self.preview_heading(*level, spans, font),
            DocBlockKind::Paragraph { spans } => div()
                .mt(px(8.0))
                .text_size(px(12.5))
                .child(self.preview_spans(spans, font, self.theme.text_secondary)),
            DocBlockKind::Code { text, .. } => self.preview_code(text, id),
            DocBlockKind::Quote { blocks } => div()
                .mt(px(10.0))
                .pl(px(12.0))
                .border_l_2()
                .border_color(self.theme.border_strong)
                .flex()
                .flex_col()
                .children(blocks.iter().enumerate().map(|(ordinal, block)| {
                    self.preview_block(block, preview, font, id.child(ordinal))
                })),
            DocBlockKind::List { ordered, items } => {
                self.preview_list(*ordered, items, preview, font, id)
            }
            DocBlockKind::Rule => div()
                .mt(px(16.0))
                .mb(px(4.0))
                .h(px(1.0))
                .bg(self.theme.border),
            DocBlockKind::Image { src, alt } => self.preview_image(src, alt, preview),
            DocBlockKind::Table {
                alignments,
                header,
                rows,
            } => self.preview_table(alignments, header, rows, font),
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
    ///
    /// Recessed rather than `bg_list`, since the pane it sits on is already
    /// that surface and a fence must read as sunk into it.
    fn preview_code(&self, text: &str, id: BlockId) -> Div {
        div()
            .relative()
            // Every fence answers to one group name: gpui resolves a hover
            // group to the innermost element that registered it, so the fence
            // under the pointer is the one that reveals its control.
            .group(CODE_GROUP)
            .mt(px(10.0))
            .px(px(10.0))
            .py(px(8.0))
            .rounded(px(5.0))
            .bg(self.theme.recessed())
            .overflow_x_hidden()
            .whitespace_nowrap()
            .font_family(MONO_FONT)
            .text_size(px(11.0))
            .text_color(self.theme.text_secondary)
            .child(text.to_owned())
            .child(self.preview_copy(text, id))
    }

    /// The fence's copy control: absent until the fence is hovered, then the
    /// same word the Actions log footer offers.
    fn preview_copy(&self, text: &str, id: BlockId) -> gpui::Stateful<Div> {
        let text = text.to_owned();
        div()
            .id(id.element("preview-copy"))
            .absolute()
            .top(px(4.0))
            .right(px(6.0))
            .cursor_pointer()
            .text_size(px(10.0))
            .text_color(self.theme.accent)
            .invisible()
            .group_hover(CODE_GROUP, Styled::visible)
            .on_click(move |_, _, cx| {
                // The fence sits inside the overlay's own click handling.
                cx.stop_propagation();
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
            })
            .child("copy")
    }

    /// A table: a washed header row over body rows, inside one rounded frame.
    ///
    /// Columns share the width evenly — a cell's own text never widens it.
    /// Measuring the widest cell per column and distributing by that is the
    /// upgrade, once a table with one long column asks for it.
    fn preview_table(
        &self,
        alignments: &[CellAlignment],
        header: &[Vec<DocSpan>],
        rows: &[Vec<Vec<DocSpan>>],
        font: &Font,
    ) -> Div {
        let mut strong = font.clone();
        strong.weight = FontWeight::SEMIBOLD;
        div()
            .mt(px(10.0))
            .border_1()
            .border_color(self.theme.border)
            .rounded(px(5.0))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .bg(self.theme.bg_chrome)
                    .children(header.iter().enumerate().map(|(column, spans)| {
                        self.preview_cell(
                            spans,
                            alignments.get(column),
                            &strong,
                            self.theme.text_primary,
                        )
                    })),
            )
            .children(rows.iter().map(|row| {
                div()
                    .flex()
                    .border_t_1()
                    .border_color(self.theme.border)
                    // A row shorter than the header renders what it has; flex
                    // spreads the cells over the width either way.
                    .children(row.iter().enumerate().map(|(column, spans)| {
                        self.preview_cell(
                            spans,
                            alignments.get(column),
                            font,
                            self.theme.text_secondary,
                        )
                    }))
            }))
    }

    /// One cell, justified by its column's alignment and clipped at its edge.
    fn preview_cell(
        &self,
        spans: &[DocSpan],
        alignment: Option<&CellAlignment>,
        font: &Font,
        color: Hsla,
    ) -> Div {
        div()
            .flex_1()
            .min_w(px(1.0))
            .px(px(8.0))
            .py(px(4.0))
            .flex()
            .overflow_hidden()
            .text_size(px(11.5))
            .map(|cell| match alignment {
                Some(CellAlignment::Center) => cell.justify_center(),
                Some(CellAlignment::Right) => cell.justify_end(),
                // A column past the delimiter row's end reads as left.
                Some(CellAlignment::Left) | None => cell,
            })
            .child(self.preview_spans(spans, font, color))
    }

    /// A list, one marker column beside each item's own blocks.
    fn preview_list(
        &self,
        ordered: bool,
        items: &[Vec<DocBlock>],
        preview: &PreviewDoc,
        font: &Font,
        id: BlockId,
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
                    .child(div().flex_1().min_w(px(1.0)).flex().flex_col().children(
                        blocks.iter().enumerate().map(|(ordinal, block)| {
                            self.preview_block(block, preview, font, id.child(index).child(ordinal))
                        }),
                    ))
            }))
    }

    /// One image block: the picture when it resolved, a framed note when it
    /// did not, and its alt text beneath either.
    fn preview_image(&self, src: &str, alt: &str, preview: &PreviewDoc) -> Div {
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

    use super::{
        ABSENT_NOTICE, DiffOrigin, DocSource, doc_source, document_blocks, stress_repeats,
    };

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
    fn only_a_readable_count_of_at_least_one_stresses_the_demo_document() {
        assert_eq!(stress_repeats(Some("50")), 50);
        assert_eq!(stress_repeats(None), 1);
        assert_eq!(stress_repeats(Some("")), 1);
        assert_eq!(stress_repeats(Some("0")), 1);
        assert_eq!(stress_repeats(Some("-3")), 1);
        assert_eq!(stress_repeats(Some("many")), 1);
    }

    #[test]
    fn the_demo_fixture_exercises_every_block_the_pane_draws() {
        let blocks = super::demo_parse().1.blocks;
        let kinds: Vec<&DocBlockKind> = blocks.iter().map(|block| &block.kind).collect();
        let has = |matcher: fn(&DocBlockKind) -> bool| kinds.iter().any(|kind| matcher(kind));

        assert!(has(|kind| matches!(kind, DocBlockKind::Heading { .. })));
        assert!(has(|kind| matches!(kind, DocBlockKind::Paragraph { .. })));
        assert!(has(|kind| matches!(kind, DocBlockKind::Code { .. })));
        assert!(has(|kind| matches!(kind, DocBlockKind::Quote { .. })));
        assert!(has(|kind| matches!(kind, DocBlockKind::List { .. })));
        assert!(has(|kind| matches!(kind, DocBlockKind::Table { .. })));
        assert!(has(|kind| matches!(kind, DocBlockKind::Rule)));
        assert!(has(|kind| matches!(kind, DocBlockKind::Image { .. })));
        assert_eq!(
            sourcefour_doc::image_sources(&blocks).len(),
            3,
            "found, missing, and remote must each have a reference to render"
        );
    }
}
