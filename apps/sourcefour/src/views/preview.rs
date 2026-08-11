//! The diff overlay's rendered-document pane: documents, not diff lines.
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
use sourcefour_doc::{
    CellAlignment, DocBlock, DocBlockKind, DocColor, DocSpan, DocumentKind, EmbeddedImageFormat,
    EmbeddedMedia, ParagraphAlignment, ParagraphStyle,
};
use sourcefour_git::{DocSource, ImageResolution};
use sourcefour_model::{DiffContent, RepoLocation, RepoPath};

use crate::theme::Theme;

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
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
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
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
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

/// The text one press, or one drag, has selected in one preview block.
///
/// One block at a time: a drag that leaves the block it started in keeps
/// extending inside that block rather than reaching into the next. The panes
/// are virtualized lists, so a block that scrolled away has no layout left to
/// measure a position against, and a selection spanning blocks would have to
/// carry text the pane is no longer drawing.
pub(super) struct PreviewSelection {
    /// The pane the selection belongs to.
    side: PreviewSide,
    /// The block inside that pane.
    block: BlockId,
    /// That block's whole text, as the runs were built from it, so a copy
    /// survives the block scrolling out of the pane.
    text: String,
    /// Byte index the press landed on.
    anchor: usize,
    /// Byte index the drag has reached; dragging backwards selects too.
    head: usize,
}

impl PreviewSelection {
    /// The highlighted range, whichever way the drag went.
    fn range(&self) -> std::ops::Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    /// The selected text; empty for a press that never dragged. Both bounds
    /// came from [`clamp_to_char_boundary`], so the slice cannot split a
    /// character.
    fn selected(&self) -> &str {
        &self.text[self.range()]
    }
}

/// The text layouts of the blocks one frame drew, by pane and block.
///
/// A `TextLayout` is `Arc`-backed state the element fills in as it lays out
/// and paints, so registering the handle while a block is built hands the drag
/// the same layout the frame goes on to paint. Cleared where the panes are
/// built, so a lookup only ever finds this frame's blocks — a drag reads back
/// the block its press landed in, which is a block the frame painted.
pub(super) type PreviewLayouts =
    std::cell::RefCell<HashMap<(PreviewSide, BlockId), gpui::TextLayout>>;

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
    /// A parse failure shown in this side's pane instead of an empty document.
    error: Option<String>,
}

/// One side's document: its blocks, the images they resolved to, and the
/// list that draws them.
pub(super) struct PreviewDoc {
    /// The document's blocks, in source order.
    pub(super) blocks: Vec<DocBlock>,
    /// One entry per distinct image reference in `blocks`.
    pub(super) images: HashMap<String, PreviewImage>,
    /// A format parser failure for this side.
    error: Option<String>,
    /// One list item per top-level block, so a frame builds the blocks on
    /// screen instead of the document. Holds the scroll position, so it is
    /// built once per loaded document and never per frame.
    list: gpui::ListState,
    /// Which pane draws this document, so a block under it can name itself
    /// in the selection and in the layout registry.
    side: PreviewSide,
    /// The window that owns the preview. A block's own mouse handler runs
    /// outside any listener the window registered, so it reaches back through
    /// this to set the selection.
    view: gpui::WeakEntity<SourcefourWindow>,
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
                {
                    let view = view.clone();
                    move |index, window, cx| {
                        view.upgrade().map_or_else(
                            || div().into_any_element(),
                            |view| view.read(cx).preview_item(side, index, window),
                        )
                    }
                },
            ),
            blocks: parse.blocks,
            images: parse.images,
            error: parse.error,
            side,
            view,
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

/// Whether the open text diff has a document parser and can be previewed.
pub(super) fn applies(view: &DiffView) -> bool {
    matches!(view.content, Some(DiffContent::Text(_)))
        && matches!(
            DocumentKind::detect(&view.origin.path().0),
            Some(DocumentKind::Markdown | DocumentKind::Rtf)
        )
}

/// Whether the preview is what the body should draw right now.
pub(super) fn showing(view: &DiffView) -> bool {
    view.show_preview && applies(view)
}

/// The new side of `origin`: the commit's own blob, the staged blob for a
/// staged working-tree entry, the file on disk otherwise.
fn doc_source(origin: &DiffOrigin) -> Option<DocSource> {
    match origin {
        DiffOrigin::Commit(request) => {
            request
                .paths
                .new_path()
                .cloned()
                .map(|path| DocSource::Commit {
                    oid: request.oid,
                    path,
                })
        }
        DiffOrigin::WorkingTree {
            paths,
            staged: true,
        } => paths
            .new_path()
            .cloned()
            .map(|path| DocSource::Index { path }),
        DiffOrigin::WorkingTree { paths, .. } => paths
            .new_path()
            .cloned()
            .map(|path| DocSource::Worktree { path }),
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
            let path = request.paths.old_path()?.clone();
            sourcefour_git::parent_commit_oid(location, request.oid, request.parent)
                .map(|oid| DocSource::Commit { oid, path })
        }
        DiffOrigin::WorkingTree {
            paths,
            staged: true,
        } => {
            let path = paths.old_path()?.clone();
            head.map(|oid| DocSource::Commit { oid, path })
        }
        DiffOrigin::WorkingTree { paths, .. } => paths
            .old_path()
            .cloned()
            .map(|path| DocSource::Index { path }),
    }
}

/// The blocks a preview renders for the bytes that side yielded.
///
/// A side without the document — the new side of a deletion, the old side of
/// an addition — renders one notice rather than an empty pane.
fn document_blocks(
    kind: DocumentKind,
    bytes: Option<&[u8]>,
) -> Result<Vec<DocBlock>, sourcefour_doc::DocumentParseError> {
    let Some(bytes) = bytes else {
        return Ok(vec![DocBlock {
            kind: DocBlockKind::Paragraph {
                spans: vec![DocSpan {
                    text: String::from(ABSENT_NOTICE),
                    ..DocSpan::default()
                }],
            },
            source_range: 0..0,
        }]);
    };
    sourcefour_doc::parse_document(kind, bytes)
}

/// Reads, parses, and resolves both sides, old first. Runs off the main
/// thread, which is why it stops at the parse.
fn load(
    location: &RepoLocation,
    origin: &DiffOrigin,
    head: Option<sourcefour_model::Oid>,
) -> (PreviewParse, PreviewParse) {
    let path = origin.path().clone();
    let kind = DocumentKind::detect(&path.0).unwrap_or(DocumentKind::Markdown);
    (
        load_side(
            location,
            old_doc_source(location, origin, head).as_ref(),
            &path,
            kind,
        ),
        load_side(location, doc_source(origin).as_ref(), &path, kind),
    )
}

/// One side's document, parsed and with every image reference resolved
/// against that side's own tree.
fn load_side(
    location: &RepoLocation,
    source: Option<&DocSource>,
    path: &RepoPath,
    kind: DocumentKind,
) -> PreviewParse {
    let bytes = source.and_then(|source| sourcefour_git::document_bytes(location, source));
    let parsed = document_blocks(kind, bytes.as_deref());
    let (blocks, error) = match parsed {
        Ok(blocks) => (blocks, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
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
    PreviewParse {
        blocks,
        images,
        error,
    }
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
            error: None,
        },
        PreviewParse {
            blocks: sourcefour_doc::parse_markdown(&stress_body()),
            images,
            error: None,
        },
    )
}

/// One block's text and the runs that style it.
///
/// Pure, so the styling is testable without a window and the selection wash
/// stays a transform over the runs rather than a branch woven through them.
fn spans_to_runs(
    spans: &[DocSpan],
    font: &Font,
    mono: &gpui::SharedString,
    color: Hsla,
    theme: &Theme,
) -> (String, Vec<TextRun>) {
    let mut text = String::new();
    let mut runs = Vec::with_capacity(spans.len());
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        runs.push(span_run(span, font, mono, color, theme));
        text.push_str(&span.text);
    }
    (text, runs)
}

/// One styled run. A run carries font, colour, and decorations but no box,
/// so inline code gets its wash and its family without the usual padding.
fn span_run(
    span: &DocSpan,
    font: &Font,
    mono: &gpui::SharedString,
    color: Hsla,
    theme: &Theme,
) -> TextRun {
    let mut font = font.clone();
    if span.code {
        font.family = mono.clone();
    }
    if span.bold {
        font.weight = FontWeight::SEMIBOLD;
    }
    if span.italic {
        font.style = gpui::FontStyle::Italic;
    }
    if let Some(family) = &span.font_family {
        font.family = family.clone().into();
    }
    let color = if span.link.is_some() {
        theme.accent
    } else if span.code {
        theme.text_primary
    } else if let Some(document_color) = span.foreground {
        theme_adapted_foreground(document_color, theme)
    } else {
        color
    };
    TextRun {
        len: span.text.len(),
        font,
        color,
        background_color: span
            .highlight
            .map(|color| theme_adapted_highlight(color, theme))
            .or_else(|| span.code.then_some(theme.bg_list)),
        underline: (span.underline || span.link.is_some()).then(|| gpui::UnderlineStyle {
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

fn document_color(color: DocColor) -> Hsla {
    gpui::rgb((u32::from(color.red) << 16) | (u32::from(color.green) << 8) | u32::from(color.blue))
        .into()
}

fn theme_adapted_foreground(color: DocColor, theme: &Theme) -> Hsla {
    let color = document_color(color);
    if (color.l - theme.bg_list.l).abs() >= 0.28 {
        color
    } else {
        theme.text_secondary
    }
}

fn theme_adapted_highlight(color: DocColor, theme: &Theme) -> Hsla {
    let color = document_color(color);
    if (color.l - theme.bg_list.l).abs() >= 0.18 {
        color.opacity(0.38)
    } else {
        theme.accent.opacity(0.24)
    }
}

/// The runs with `range` washed in `background`, split where the range cuts
/// through a run. A selection is a run split rather than a painted quad,
/// because gpui lays text out inside one element and a quad would have to
/// follow the wrapping.
///
/// The indices are the caller's: they must already sit on character
/// boundaries of the text the runs were built from — [`clamp_to_char_boundary`]
/// is what puts them there.
fn highlight_runs(
    runs: Vec<TextRun>,
    range: std::ops::Range<usize>,
    background: Hsla,
) -> Vec<TextRun> {
    if range.is_empty() {
        return runs;
    }
    let mut washed = Vec::with_capacity(runs.len() + 2);
    let mut offset = 0;
    for run in runs {
        let end = offset + run.len;
        // Both bounds inside this run, so a range reaching past either side
        // of it contributes nothing there.
        let start = range.start.clamp(offset, end);
        let stop = range.end.clamp(offset, end);
        for (len, inside) in [
            (start - offset, false),
            (stop - start, true),
            (end - stop, false),
        ] {
            if len == 0 {
                continue;
            }
            washed.push(TextRun {
                len,
                background_color: if inside {
                    Some(background)
                } else {
                    run.background_color
                },
                ..run.clone()
            });
        }
        offset = end;
    }
    washed
}

/// `index` moved back to the character boundary at or before it, and never
/// past the end of the text.
///
/// Every index taken from a pointer position goes through here: one landing
/// inside a multi-byte character would panic the slice a copy takes and cut a
/// glyph in half in the runs.
fn clamp_to_char_boundary(text: &str, mut index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The byte index one window position falls on inside a laid-out block.
///
/// `index_for_position` answers `Err` with the nearest index when the position
/// is outside the text, which is what a drag past the last line wants. Both
/// sides are window coordinates: a layout keeps the bounds it was painted at,
/// and gpui's own interactive text hands it the event position unchanged.
fn index_at(layout: &gpui::TextLayout, text: &str, position: gpui::Point<gpui::Pixels>) -> usize {
    let index = layout
        .index_for_position(position)
        .unwrap_or_else(|nearest| nearest);
    clamp_to_char_boundary(text, index)
}

/// The rendered document: one list item per top-level block, so a frame
/// costs what is on screen rather than what the document holds.
///
/// The pane's own id scopes every element id the blocks under it build.
pub(super) fn pane(preview: &PreviewDoc, side: PreviewSide, theme: &Theme) -> gpui::Stateful<Div> {
    let pane = div().id(side.pane_id()).size_full();
    if let Some(error) = &preview.error {
        pane.flex()
            .items_center()
            .justify_center()
            .p(px(24.0))
            .text_size(px(12.0))
            .text_color(theme.text_faint)
            .child(format!("Unable to preview this RTF: {error}"))
    } else {
        pane.child(gpui::list(preview.list.clone()).size_full())
    }
}

impl SourcefourWindow {
    /// Switches the open diff between its lines and its rendered document,
    /// starting the document's load the first time the preview is asked for.
    pub(super) fn toggle_preview(&mut self, show: bool, cx: &mut gpui::Context<Self>) {
        // A selection belongs to the rendered document that is on screen.
        self.clear_preview_selection();
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
                blocks: document_blocks(DocumentKind::Markdown, None)
                    .expect("an absent side does not parse bytes"),
                images: HashMap::new(),
                error: None,
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

    /// Extends the open selection to where a held drag has reached.
    ///
    /// Only the block the press landed in is measured, and only while this
    /// frame drew it: a drag that left the block keeps the nearest index
    /// inside it, which is what `index_for_position` answers with.
    pub(super) fn drag_preview_selection(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some((side, block)) = self
            .preview_selection
            .as_ref()
            .map(|selection| (selection.side, selection.block))
        else {
            return;
        };
        let layout = self.preview_layouts.borrow().get(&(side, block)).cloned();
        let (Some(layout), Some(selection)) = (layout, self.preview_selection.as_mut()) else {
            return;
        };
        let head = index_at(&layout, &selection.text, position);
        if selection.head != head {
            selection.head = head;
            cx.notify();
        }
    }

    /// Puts the selected text on the clipboard. A press that never dragged
    /// selected nothing, and copies nothing.
    pub(super) fn copy_preview_selection(&self, cx: &mut gpui::App) {
        let Some(selected) = self
            .preview_selection
            .as_ref()
            .map(PreviewSelection::selected)
            .filter(|selected| !selected.is_empty())
        else {
            return;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected.to_owned()));
    }

    /// Drops the selection, reporting whether anything was highlighted.
    ///
    /// Escape asks before it closes the overlay, so the first Escape gives
    /// back the highlight and the second closes; a press that never dragged
    /// has nothing to give back and closes on the first.
    pub(super) fn clear_preview_selection(&mut self) -> bool {
        let highlighted = self
            .preview_selection
            .as_ref()
            .is_some_and(|selection| !selection.range().is_empty());
        self.preview_selection = None;
        highlighted
    }

    /// The Source/Preview control, left of the layout control it mirrors.
    pub(super) fn preview_toggle(&self, showing: bool, cx: &mut gpui::Context<Self>) -> Div {
        div()
            .flex_none()
            .h(px(24.0))
            .flex()
            .items_center()
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
            // The list measures items with the pane's width available, but an
            // auto-width flex container still sizes to its content — only
            // long paragraphs coincidentally reached the cap.
            .w_full()
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
            DocBlockKind::Heading { level, spans } => {
                self.preview_heading(*level, spans, font, preview, id)
            }
            DocBlockKind::Paragraph { spans } => div()
                .mt(px(8.0))
                .text_size(px(12.5))
                .child(self.selectable_text(spans, font, self.theme.text_secondary, preview, id)),
            DocBlockKind::RichParagraph { spans, style } => {
                self.preview_rich_paragraph(spans, *style, font, preview, id)
            }
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
            DocBlockKind::PageBreak => div()
                .mt(px(16.0))
                .mb(px(6.0))
                .border_t_1()
                .border_color(self.theme.border)
                .pt(px(4.0))
                .text_size(px(9.0))
                .text_color(self.theme.text_faint)
                .child("page break"),
            DocBlockKind::Aside { label, blocks } => div()
                .mt(px(10.0))
                .p(px(10.0))
                .rounded(px(5.0))
                .border_1()
                .border_color(self.theme.border)
                .bg(self.theme.bg_chrome)
                .child(
                    div()
                        .mb(px(4.0))
                        .text_size(px(9.5))
                        .text_color(self.theme.text_faint)
                        .child(label.clone()),
                )
                .children(blocks.iter().enumerate().map(|(ordinal, block)| {
                    self.preview_block(block, preview, font, id.child(ordinal))
                })),
            DocBlockKind::EmbeddedMedia { media, alt } => {
                self.preview_embedded_media(media, alt, id)
            }
            DocBlockKind::Image { src, alt } => self.preview_image(src, alt, preview, id),
            DocBlockKind::Table {
                alignments,
                header,
                rows,
            } => self.preview_table(alignments, header, rows, font, preview, id),
        }
    }

    fn preview_rich_paragraph(
        &self,
        spans: &[DocSpan],
        style: ParagraphStyle,
        font: &Font,
        preview: &PreviewDoc,
        id: BlockId,
    ) -> Div {
        let twips = |value: i32| {
            let bounded = i16::try_from(value.clamp(0, 1080)).unwrap_or_default();
            px(f32::from(bounded) / 15.0)
        };
        let dominant_size = spans
            .iter()
            .filter_map(|span| {
                span.font_size_half_points
                    .map(|size| (size, span.text.len()))
            })
            .fold(HashMap::<u16, usize>::new(), |mut counts, (size, len)| {
                *counts.entry(size).or_default() += len;
                counts
            })
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map_or(12.5, |(size, _)| (f32::from(size) / 2.0).clamp(9.0, 24.0));
        div()
            .mt(twips(style.space_before_twips.max(120)))
            .mb(twips(style.space_after_twips))
            .pl(twips(
                style.left_indent_twips + style.first_line_indent_twips.max(0),
            ))
            .pr(twips(style.right_indent_twips))
            .text_size(px(dominant_size))
            .map(|paragraph| match style.alignment {
                ParagraphAlignment::Center => paragraph.text_center(),
                ParagraphAlignment::Right => paragraph.text_right(),
                ParagraphAlignment::Left | ParagraphAlignment::Justify => paragraph.text_left(),
            })
            .child(self.selectable_text(spans, font, self.theme.text_secondary, preview, id))
    }

    /// A heading, sized by its level; the whole line carries the weight so a
    /// bold run inside one changes nothing.
    fn preview_heading(
        &self,
        level: u8,
        spans: &[DocSpan],
        font: &Font,
        preview: &PreviewDoc,
        id: BlockId,
    ) -> Div {
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
            .child(self.selectable_text(spans, &heading, self.theme.text_primary, preview, id))
    }

    /// A block of styled runs as one wrapping paragraph, selectable by a
    /// press and a drag across it.
    ///
    /// Every mark lands on a text run rather than a nested element, because
    /// gpui wraps text inside one element and never across two — and the
    /// selection washes runs for the same reason.
    fn selectable_text(
        &self,
        spans: &[DocSpan],
        font: &Font,
        color: Hsla,
        preview: &PreviewDoc,
        id: BlockId,
    ) -> Div {
        let (text, runs) = spans_to_runs(spans, font, &self.mono_font(), color, &self.theme);
        let text = gpui::SharedString::from(text);
        let selected = self
            .preview_selection
            .as_ref()
            .filter(|selection| selection.side == preview.side && selection.block == id)
            .map(PreviewSelection::range);
        let runs = match selected {
            Some(range) => highlight_runs(runs, range, self.theme.accent.opacity(0.35)),
            None => runs,
        };
        let element = StyledText::new(text.clone()).with_runs(runs);
        let layout = element.layout().clone();
        self.preview_layouts
            .borrow_mut()
            .insert((preview.side, id), layout.clone());
        let view = preview.view.clone();
        let side = preview.side;
        div()
            .cursor_text()
            .on_mouse_down(
                gpui::MouseButton::Left,
                move |event: &gpui::MouseDownEvent, _, cx| {
                    // The press fixes the anchor; the drag it may turn into is
                    // routed by the overlay's own move handler, since a drag
                    // leaves this block long before it ends.
                    let index = index_at(&layout, &text, event.position);
                    view.update(cx, |this, cx| {
                        this.preview_selection = Some(PreviewSelection {
                            side,
                            block: id,
                            text: text.to_string(),
                            anchor: index,
                            head: index,
                        });
                        this.drag = Some(super::Drag::PreviewText);
                        cx.notify();
                    })
                    .ok();
                    // The overlay closes on a click that reaches its backdrop.
                    cx.stop_propagation();
                },
            )
            .child(element)
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
            .font_family(self.mono_font())
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
        preview: &PreviewDoc,
        id: BlockId,
    ) -> Div {
        let mut strong = font.clone();
        strong.weight = FontWeight::SEMIBOLD;
        div()
            .mt(px(10.0))
            // The well spans the column even when its rows are narrow;
            // shrink-to-fit reads as a layout bug beside full-width blocks.
            .w_full()
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
                            preview,
                            // A cell is its own selectable block, addressed by
                            // its place in the grid: the header is row zero.
                            id.child(0).child(column),
                        )
                    })),
            )
            .children(rows.iter().enumerate().map(|(row_index, row)| {
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
                            preview,
                            id.child(row_index + 1).child(column),
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
        preview: &PreviewDoc,
        id: BlockId,
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
            .child(self.selectable_text(spans, font, color, preview, id))
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
    fn preview_image(&self, src: &str, alt: &str, preview: &PreviewDoc, id: BlockId) -> Div {
        let body = match preview.images.get(src) {
            // Both bounds are absolute for the same reason the column is: a
            // relative maximum leaves the image with no width to fit into.
            //
            // The id is what makes an animated GIF advance: gpui only steps
            // frames for an element it can keep state against. It has to be the
            // block's address rather than the image's own id — one document can
            // reference the same source twice, and `images` hands both the same
            // `Arc`, so two elements would share one frame counter.
            Some(PreviewImage::Loaded(image)) => gpui::img(image.clone())
                .id(id.element("preview-image"))
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

    fn preview_embedded_media(&self, media: &EmbeddedMedia, alt: &str, id: BlockId) -> Div {
        let body = match media {
            EmbeddedMedia::Image { format, bytes } => {
                let format = match format {
                    EmbeddedImageFormat::Png => "png",
                    EmbeddedImageFormat::Jpeg => "jpeg",
                };
                render_image(Some(bytes), format).map_or_else(
                    || {
                        self.preview_placeholder(String::from(
                            "embedded image could not be decoded",
                        ))
                        .into_any_element()
                    },
                    |image| {
                        gpui::img(image)
                            .id(id.element("preview-embedded-image"))
                            .max_w(px(CONTENT_WIDTH - 2.0 * CONTENT_PADDING))
                            .max_h(px(IMAGE_HEIGHT))
                            .object_fit(gpui::ObjectFit::Contain)
                            .rounded(px(5.0))
                            .into_any_element()
                    },
                )
            }
            EmbeddedMedia::Unsupported { label } => self
                .preview_placeholder(format!("unsupported embedded content · {label}"))
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
    use gpui::{FontWeight, Hsla, TextRun};
    use sourcefour_doc::{DocBlockKind, DocSpan, DocumentKind};
    use sourcefour_model::{DiffParent, DiffPaths, FileDiffRequest, Oid, RepoPath};

    use crate::theme::Theme;

    use super::{
        ABSENT_NOTICE, DiffOrigin, DocSource, clamp_to_char_boundary, doc_source, document_blocks,
        highlight_runs, spans_to_runs, stress_repeats,
    };

    fn path(text: &str) -> RepoPath {
        RepoPath(text.as_bytes().to_vec())
    }

    /// A mono family no platform default is, so a code span's font proves it
    /// came from the caller rather than from a constant.
    const MONO: &str = "Fira Code";

    /// The wash a selection paints, distinct from anything the palette hands
    /// a run, so a test can tell the two apart.
    const WASH: Hsla = Hsla {
        h: 0.6,
        s: 0.9,
        l: 0.6,
        a: 0.35,
    };

    /// Plain runs of the given byte lengths, nothing washed.
    fn runs(lengths: &[usize]) -> Vec<TextRun> {
        lengths
            .iter()
            .map(|&len| TextRun {
                len,
                font: gpui::font("Helvetica"),
                color: gpui::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            })
            .collect()
    }

    /// What a split left behind: each run's length and whether it is washed.
    fn shape(runs: &[TextRun]) -> Vec<(usize, bool)> {
        runs.iter()
            .map(|run| (run.len, run.background_color == Some(WASH)))
            .collect()
    }

    fn span(text: &str) -> DocSpan {
        DocSpan {
            text: String::from(text),
            ..DocSpan::default()
        }
    }

    #[test]
    fn a_selection_on_run_boundaries_washes_whole_runs() {
        let washed = highlight_runs(runs(&[3, 4, 5]), 3..7, WASH);

        assert_eq!(shape(&washed), vec![(3, false), (4, true), (5, false)]);
    }

    #[test]
    fn a_selection_inside_one_run_splits_it_in_three() {
        let washed = highlight_runs(runs(&[10]), 3..6, WASH);

        assert_eq!(shape(&washed), vec![(3, false), (3, true), (4, false)]);
    }

    #[test]
    fn a_selection_over_everything_washes_every_run() {
        let washed = highlight_runs(runs(&[2, 3]), 0..5, WASH);

        assert_eq!(shape(&washed), vec![(2, true), (3, true)]);
    }

    #[test]
    fn an_empty_selection_leaves_the_runs_alone() {
        let washed = highlight_runs(runs(&[2, 3]), 4..4, WASH);

        assert_eq!(shape(&washed), vec![(2, false), (3, false)]);
    }

    #[test]
    fn a_selection_past_the_text_stops_at_its_end() {
        let washed = highlight_runs(runs(&[2, 3]), 3..99, WASH);

        assert_eq!(
            shape(&washed),
            vec![(2, false), (1, false), (2, true)],
            "no run may be emitted empty, and none may reach past the text"
        );
    }

    #[test]
    fn a_split_never_loses_a_byte() {
        for range in [0..0, 0..11, 1..3, 2..7, 6..11, 4..99] {
            let washed = highlight_runs(runs(&[6, 5]), range.clone(), WASH);
            let total: usize = washed.iter().map(|run| run.len).sum();

            assert_eq!(total, 11, "{range:?} changed the text length");
            assert!(washed.iter().all(|run| run.len > 0));
        }
    }

    #[test]
    fn runs_over_multibyte_text_split_exactly_where_the_caller_says() {
        // "æøå 🌍": 2 + 2 + 2 + 1 + 4 bytes, as two runs of 6 and 5.
        let text = "æøå 🌍";
        let washed = highlight_runs(runs(&[6, 5]), 2..7, WASH);

        assert_eq!(
            shape(&washed),
            vec![(2, false), (4, true), (1, true), (4, false)]
        );
        assert_eq!(&text[2..7], "øå ", "the split must follow the byte indices");
        assert_eq!(
            shape(&highlight_runs(runs(&[6, 5]), 1..3, WASH)),
            vec![(1, false), (2, true), (3, false), (5, false)],
            "the function trusts its caller: clamping indices is the caller's job"
        );
    }

    #[test]
    fn a_byte_index_snaps_back_to_the_character_it_landed_in() {
        let text = "æøå 🌍";

        assert_eq!(clamp_to_char_boundary(text, 0), 0);
        assert_eq!(clamp_to_char_boundary(text, 1), 0);
        assert_eq!(clamp_to_char_boundary(text, 2), 2);
        assert_eq!(clamp_to_char_boundary(text, 7), 7);
        assert_eq!(clamp_to_char_boundary(text, 8), 7);
        assert_eq!(clamp_to_char_boundary(text, 10), 7);
        assert_eq!(clamp_to_char_boundary(text, 11), 11);
        assert_eq!(clamp_to_char_boundary(text, 99), 11);
        assert_eq!(clamp_to_char_boundary("", 4), 0);
    }

    #[test]
    fn every_clamped_index_is_one_the_text_can_be_sliced_at() {
        let text = "æ 🌍 ok";

        for index in 0..=text.len() + 3 {
            let clamped = clamp_to_char_boundary(text, index);

            assert!(text.is_char_boundary(clamped), "{index} landed mid-glyph");
            assert!(clamped <= index.min(text.len()));
            let _ = &text[..clamped];
        }
    }

    #[test]
    fn spans_become_one_string_and_one_run_each() {
        let theme = Theme::dark();
        let (text, runs) = spans_to_runs(
            &[span("æøå"), span(""), span(" tail")],
            &gpui::font("Helvetica"),
            &MONO.into(),
            theme.text_secondary,
            &theme,
        );

        assert_eq!(text, "æøå tail");
        assert_eq!(
            runs.iter().map(|run| run.len).collect::<Vec<_>>(),
            vec![6, 5],
            "an empty span contributes no run"
        );
        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
    }

    #[test]
    fn a_run_carries_the_marks_its_span_had() {
        let theme = Theme::dark();
        let (_, runs) = spans_to_runs(
            &[
                DocSpan {
                    text: String::from("code"),
                    code: true,
                    ..DocSpan::default()
                },
                DocSpan {
                    text: String::from("link"),
                    link: Some(String::from("https://example.com")),
                    ..DocSpan::default()
                },
                DocSpan {
                    text: String::from("bold"),
                    bold: true,
                    ..DocSpan::default()
                },
                DocSpan {
                    text: String::from("gone"),
                    italic: true,
                    strike: true,
                    ..DocSpan::default()
                },
                DocSpan {
                    text: String::from("under"),
                    underline: true,
                    ..DocSpan::default()
                },
            ],
            &gpui::font("Helvetica"),
            &MONO.into(),
            theme.text_secondary,
            &theme,
        );

        assert_eq!(
            runs[0].font.family, MONO,
            "the mono family is the one the caller named"
        );
        assert_eq!(runs[0].background_color, Some(theme.bg_list));
        assert_eq!(runs[1].color, theme.accent);
        assert!(runs[1].underline.is_some());
        assert_eq!(runs[2].font.weight, FontWeight::SEMIBOLD);
        assert_eq!(runs[3].font.style, gpui::FontStyle::Italic);
        assert!(runs[3].strikethrough.is_some());
        assert!(runs[4].underline.is_some());
    }

    #[test]
    fn the_selection_wash_replaces_the_one_a_run_already_carried() {
        let theme = Theme::dark();
        let (text, runs) = spans_to_runs(
            &[DocSpan {
                text: String::from("code"),
                code: true,
                ..DocSpan::default()
            }],
            &gpui::font("Helvetica"),
            &MONO.into(),
            theme.text_secondary,
            &theme,
        );
        let washed = highlight_runs(runs, 0..text.len(), WASH);

        assert_eq!(shape(&washed), vec![(4, true)]);
        assert_eq!(washed[0].font.family, MONO, "the marks stay");
    }

    #[test]
    fn a_commit_diff_previews_that_commit_s_own_blob() {
        let oid = Oid::from_hex(&format!("{:0<40}", "9f3e21a")).expect("hexadecimal");
        let origin = DiffOrigin::Commit(FileDiffRequest {
            oid,
            parent: DiffParent::FirstParent,
            paths: DiffPaths::same(path("README.md")),
        });

        assert!(
            matches!(
                doc_source(&origin),
                Some(DocSource::Commit { oid: read, path: read_path })
                    if read == oid && read_path == path("README.md")
            ),
            "the preview must read the side the diff is showing"
        );
    }

    #[test]
    fn a_working_tree_diff_previews_the_side_that_was_clicked() {
        let origin = |staged| DiffOrigin::WorkingTree {
            paths: DiffPaths::same(path("docs/notes.md")),
            staged,
        };

        assert!(matches!(
            doc_source(&origin(true)),
            Some(DocSource::Index { path: read }) if read == path("docs/notes.md")
        ));
        assert!(matches!(
            doc_source(&origin(false)),
            Some(DocSource::Worktree { path: read }) if read == path("docs/notes.md")
        ));
    }

    #[test]
    fn a_side_without_the_document_renders_a_notice_rather_than_nothing() {
        let blocks = document_blocks(DocumentKind::Markdown, None).unwrap();

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
        let blocks = document_blocks(DocumentKind::Markdown, Some(b"# Title\n\ntext\n")).unwrap();

        assert_eq!(blocks.len(), 2);
        assert!(matches!(
            blocks[0].kind,
            DocBlockKind::Heading { level: 1, .. }
        ));
    }

    #[test]
    fn rtf_bytes_use_the_rtf_parser() {
        let blocks =
            document_blocks(DocumentKind::Rtf, Some(br"{\rtf1\ansi Plain {\b bold}.}")).unwrap();

        let DocBlockKind::RichParagraph { spans, .. } = &blocks[0].kind else {
            panic!("RTF prose should become a rich paragraph");
        };
        assert_eq!(
            spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            "Plain bold."
        );
        assert!(spans.iter().any(|span| span.bold && span.text == "bold"));
    }

    #[test]
    fn malformed_rtf_is_a_parse_error() {
        assert!(document_blocks(DocumentKind::Rtf, Some(br"{\rtf1 broken")).is_err());
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
