//! The full-window diff overlay (§6.11): text unified/split layouts and the
//! image before/after views with the juxtapose slider.
//!
//! A video comparison reuses those same two views. Its poster frames arrive as
//! PNG, so once they are wrapped there is nothing left to tell apart — only the
//! chips along the bottom, and the card that stands in when no frame was
//! decoded, know a video from an image.

use std::{ops::Range, sync::Arc};

use gpui::{
    Div, FontWeight, IntoElement, ListHorizontalSizingBehavior, StatefulInteractiveElement,
    UniformListScrollHandle, Window, div, prelude::*, px, svg, uniform_list,
};
use sourcefour_model::{
    ChangeKind, ChangedFile, DiffContent, DiffCoordinate, DiffPaths, DiffSide, FileDiffRequest,
    RepoPath, TextDiff, VideoInfo,
};

use super::{
    Drag, SourcefourWindow, change_color, change_letter, counted,
    preview::{self, PreviewSide, PreviewState},
    row_count_as_f32,
};

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

/// Where an open diff came from, so a preview can read the same file back
/// out of the repository without re-deriving it from the display title.
#[derive(Clone)]
pub(super) enum DiffOrigin {
    /// One file of a commit, against the selected parent.
    Commit(FileDiffRequest),
    /// One working-tree file, on whichever side of the index was clicked.
    WorkingTree {
        /// Paths on the index/tree and commit/worktree sides.
        paths: DiffPaths,
        /// Whether the staged side is being shown.
        staged: bool,
    },
}

impl DiffOrigin {
    /// The path the diff is showing, as the repository names it.
    pub(super) fn path(&self) -> &RepoPath {
        match self {
            Self::Commit(request) => request.paths.display_path(),
            Self::WorkingTree { paths, .. } => paths.display_path(),
        }
    }
}

/// One open file diff: header info plus content once loaded (§6.11).
pub(super) struct DiffView {
    pub(super) title: String,
    pub(super) status: ChangeKind,
    pub(super) files: Arc<[ChangedFile]>,
    pub(super) file_index: usize,
    pub(super) current_hunk: usize,
    pub(super) selection: Option<DiffSelection>,
    pub(super) full_context: bool,
    /// Where the file came from, for the preview's own read.
    pub(super) origin: DiffOrigin,
    /// `None` while the read is in flight.
    pub(super) content: Option<DiffContent>,
    pub(super) mode: DiffMode,
    /// Whether the rendered document replaces the diff lines. Session-only:
    /// unlike the layout, nothing about a preview persists across launches.
    pub(super) show_preview: bool,
    /// The parsed document with its images, `None` until the load lands.
    pub(super) preview: Option<PreviewState>,
    /// Optional syntax and intraline layers, computed after plain rows land.
    pub(super) highlight: Option<Arc<crate::diff_highlight::DiffHighlight>>,
    pub(super) highlight_message: Option<&'static str>,
    /// Layout projections, computed once per loaded semantic diff.
    pub(super) unified: Option<Vec<crate::diff_split::DiffRow>>,
    pub(super) split: Option<Vec<crate::diff_split::DiffRow>>,
    pub(super) wrap_list: Option<gpui::ListState>,
    /// Renderable old-side image, wrapped once per content.
    pub(super) before_image: Option<Arc<gpui::Image>>,
    /// Renderable new-side image, wrapped once per content.
    pub(super) after_image: Option<Arc<gpui::Image>>,
    /// Juxtapose divider position as a fraction of the width.
    pub(super) slider: f32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct HunkEdges {
    outline: HunkOutline,
    top: bool,
    bottom: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum HunkOutline {
    #[default]
    None,
    Idle,
    Selected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DiffSelection {
    anchor: DiffCoordinate,
    head: DiffCoordinate,
}

impl DiffSelection {
    fn contains(self, cell: &crate::diff_split::DiffCell) -> bool {
        if self.anchor.side != cell.side || self.head.side != cell.side {
            return false;
        }
        let start = self.anchor.line.min(self.head.line);
        let end = self.anchor.line.max(self.head.line);
        (start..=end).contains(&cell.line)
    }
}

impl DiffView {
    fn new(
        status: ChangeKind,
        files: Arc<[ChangedFile]>,
        file_index: usize,
        origin: DiffOrigin,
        mode: DiffMode,
    ) -> Self {
        Self {
            title: origin.path().display_lossy(),
            status,
            files,
            file_index,
            current_hunk: 0,
            selection: None,
            full_context: false,
            origin,
            content: None,
            mode,
            show_preview: false,
            preview: None,
            highlight: None,
            highlight_message: None,
            unified: None,
            split: None,
            wrap_list: None,
            before_image: None,
            after_image: None,
            slider: 0.5,
        }
    }

    pub(super) fn for_commit(
        request: FileDiffRequest,
        status: ChangeKind,
        files: Arc<[ChangedFile]>,
        file_index: usize,
        mode: DiffMode,
    ) -> Self {
        Self::new(status, files, file_index, DiffOrigin::Commit(request), mode)
    }

    pub(super) fn for_working_tree(
        paths: DiffPaths,
        staged: bool,
        status: ChangeKind,
        files: Arc<[ChangedFile]>,
        file_index: usize,
        mode: DiffMode,
    ) -> Self {
        Self::new(
            status,
            files,
            file_index,
            DiffOrigin::WorkingTree { paths, staged },
            mode,
        )
    }

    pub(super) fn replace_content(&mut self, content: DiffContent) {
        let started = std::time::Instant::now();
        self.content = Some(content);
        self.current_hunk = 0;
        self.selection = None;
        self.highlight = None;
        self.highlight_message = None;
        self.unified = None;
        self.split = None;
        self.wrap_list = None;
        self.before_image = None;
        self.after_image = None;
        self.ensure_rows();
        self.ensure_images();
        if std::env::var_os("SOURCEFOUR_FRAME_LOG").is_some() {
            eprintln!(
                "diff-project-ms {:.3} unified_rows={} split_rows={}",
                started.elapsed().as_secs_f64() * 1000.0,
                self.unified.as_ref().map_or(0, Vec::len),
                self.split.as_ref().map_or(0, Vec::len),
            );
        }
    }

    fn set_mode(&mut self, mode: DiffMode) {
        self.mode = mode;
        self.ensure_rows();
        self.current_hunk = self
            .current_hunk
            .min(self.hunk_rows().len().saturating_sub(1));
        self.selection = None;
        self.wrap_list = None;
    }

    fn toggle_full_context(&mut self) {
        self.full_context = !self.full_context;
        self.current_hunk = 0;
        self.selection = None;
        self.unified = None;
        self.split = None;
        self.wrap_list = None;
        self.ensure_rows();
    }

    /// Builds lightweight layout projections when text content lands.
    pub(super) fn ensure_rows(&mut self) {
        if self.unified.is_some() && self.split.is_some() {
            return;
        }
        if let Some(DiffContent::Text(diff)) = &self.content {
            self.unified = Some(crate::diff_split::unified_rows_with_context(
                diff,
                self.full_context,
            ));
            self.split = Some(crate::diff_split::split_rows_with_context(
                diff,
                self.full_context,
            ));
        }
    }

    fn text_diff(&self) -> Option<&TextDiff> {
        match &self.content {
            Some(DiffContent::Text(diff)) => Some(diff),
            _ => None,
        }
    }

    fn rows(&self) -> &[crate::diff_split::DiffRow] {
        match self.mode {
            DiffMode::Unified => self.unified.as_deref().unwrap_or_default(),
            DiffMode::Split => self.split.as_deref().unwrap_or_default(),
        }
    }

    fn hunk_rows(&self) -> Vec<usize> {
        self.rows()
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                matches!(row, crate::diff_split::DiffRow::Hunk { .. }).then_some(index)
            })
            .collect()
    }

    fn hunk_edges(&self, row_index: usize) -> HunkEdges {
        let rows = self.rows();
        let Some((hunk_index, start)) = rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                matches!(row, crate::diff_split::DiffRow::Hunk { .. }).then_some(index)
            })
            .enumerate()
            .take_while(|(_, start)| *start <= row_index)
            .last()
        else {
            return HunkEdges::default();
        };
        let end = rows
            .iter()
            .enumerate()
            .skip(start + 1)
            .find_map(|(index, row)| {
                matches!(
                    row,
                    crate::diff_split::DiffRow::Hunk { .. }
                        | crate::diff_split::DiffRow::Gap { .. }
                )
                .then_some(index)
            })
            .unwrap_or(rows.len());
        let outlined = (start..end).contains(&row_index);
        HunkEdges {
            outline: if !outlined {
                HunkOutline::None
            } else if hunk_index == self.current_hunk {
                HunkOutline::Selected
            } else {
                HunkOutline::Idle
            },
            top: outlined && row_index == start,
            bottom: outlined && row_index + 1 == end,
        }
    }

    fn widest_row(&self) -> Option<usize> {
        let diff = self.text_diff()?;
        self.rows()
            .iter()
            .enumerate()
            .max_by_key(|(_, row)| match row {
                crate::diff_split::DiffRow::Line { left, right } => [left, right]
                    .into_iter()
                    .flatten()
                    .filter_map(|cell| {
                        let side = match cell.side {
                            DiffSide::Old => diff.old.as_ref(),
                            DiffSide::New => diff.new.as_ref(),
                        }?;
                        side.line(cell.line).map(str::len)
                    })
                    .max()
                    .unwrap_or_default(),
                crate::diff_split::DiffRow::Hunk { .. }
                | crate::diff_split::DiffRow::Gap { .. }
                | crate::diff_split::DiffRow::Marker { .. } => 0,
            })
            .map(|(index, _)| index)
    }

    /// Wraps image bytes for gpui once per loaded content; wrapping per frame
    /// would defeat the renderer's id-keyed image cache.
    ///
    /// A video's poster frames arrive here too, already PNG, which is what
    /// lets both comparison views draw one without knowing the difference.
    pub(super) fn ensure_images(&mut self) {
        if self.before_image.is_some() || self.after_image.is_some() {
            return;
        }
        match &self.content {
            Some(DiffContent::Image {
                before,
                after,
                format,
            }) => {
                self.before_image = render_image(before.as_deref(), format);
                self.after_image = render_image(after.as_deref(), format);
            }
            Some(DiffContent::Video { before, after, .. }) => {
                self.before_image = render_image(before.as_deref(), "png");
                self.after_image = render_image(after.as_deref(), "png");
            }
            _ => {}
        }
    }

    fn is_image(&self) -> bool {
        matches!(self.content, Some(DiffContent::Image { .. }))
    }

    fn is_video(&self) -> bool {
        matches!(self.content, Some(DiffContent::Video { .. }))
    }

    /// Whether the two layouts show frames rather than lines, which is what
    /// the layout control names itself after.
    fn compares_frames(&self) -> bool {
        self.is_image() || (self.is_video() && self.has_poster())
    }

    /// Whether the layout control has two distinct layouts to offer at all.
    fn offers_layouts(&self) -> bool {
        !self.is_video() || self.has_poster()
    }

    /// Whether a video comparison has a frame to show on either side.
    ///
    /// Without one there is nothing for the two layouts to lay out
    /// differently, so the card stands in and the layout control goes away.
    fn has_poster(&self) -> bool {
        self.before_image.is_some() || self.after_image.is_some()
    }

    /// What each side of a video comparison reports; empty for anything else.
    fn video_facts(&self) -> (Option<VideoInfo>, Option<VideoInfo>) {
        match &self.content {
            Some(DiffContent::Video {
                before_info,
                after_info,
                ..
            }) => (*before_info, *after_info),
            _ => (None, None),
        }
    }

    /// What the bottom chips say for each present media side.
    fn media_summaries(&self) -> (Option<gpui::SharedString>, Option<gpui::SharedString>) {
        match &self.content {
            Some(DiffContent::Image { before, after, .. }) => (
                before.as_ref().map(|bytes| image_summary(bytes.len())),
                after.as_ref().map(|bytes| image_summary(bytes.len())),
            ),
            Some(DiffContent::Video { .. }) => {
                let (before, after) = self.video_facts();
                (before.map(video_summary), after.map(video_summary))
            }
            _ => (None, None),
        }
    }
}

/// One side's numbers, in the order they change least. Used bare by the card,
/// which draws its own marker, and marked by [`video_summary`].
fn video_line(info: VideoInfo) -> String {
    let mut parts = Vec::new();
    if let Some(duration_ms) = info.duration_ms {
        parts.push(video_duration(duration_ms));
    }
    if let Some((width, height)) = info.dimensions {
        parts.push(format!("{width} × {height}"));
    }
    parts.push(byte_size(info.bytes));
    parts.join(" · ")
}

/// The same line as a chip, led by a marker.
///
/// The marker is the only thing separating this view from an image diff: same
/// panes, same slider, same poster drawn the same way. Without it a video reads
/// as a still, which is the one thing the view must not say.
fn video_summary(info: VideoInfo) -> gpui::SharedString {
    format!("▶ {}", video_line(info)).into()
}

fn image_summary(bytes: usize) -> gpui::SharedString {
    byte_size(u64::try_from(bytes).unwrap_or(u64::MAX)).into()
}

/// What to tell the reader when the card is bare because nothing was installed
/// to decode with, and nothing when it is bare for a reason they cannot act on.
///
/// Either side is enough: both are probed by the same absent tools, and a
/// comparison where only one side exists still has the same thing to say.
fn missing_tools_note(before: Option<VideoInfo>, after: Option<VideoInfo>) -> Option<&'static str> {
    [before, after]
        .into_iter()
        .flatten()
        .any(|info| info.tools_missing)
        .then_some("Install ffmpeg to see a preview frame")
}

/// A duration as `m:ss`, growing an hours field only when there is one.
fn video_duration(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// A byte count in the largest unit that leaves a number worth reading.
fn byte_size(bytes: u64) -> String {
    const STEP: f64 = 1024.0;
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    #[expect(
        clippy::cast_precision_loss,
        reason = "a file large enough to lose precision here is larger than any disk"
    )]
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= STEP && unit + 1 < UNITS.len() {
        size /= STEP;
        unit += 1;
    }
    // Whole bytes have no fraction worth showing, and every larger unit does.
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Wraps encoded image bytes as a gpui image with a fresh cache id.
pub(super) fn render_image(bytes: Option<&[u8]>, format: &str) -> Option<Arc<gpui::Image>> {
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

/// One image element in a diff pane, identified so gpui will animate it.
///
/// gpui only steps a multi-frame image forward for an element it can keep
/// state against, so a bare `img` leaves every animated GIF on frame zero. The
/// id carries the image's own identity: it must stay the same across frames
/// (or the animation restarts every paint) and differ between the two images
/// the slider shows at once, which the wrapper's process-unique counter gives
/// for free. It also keeps a stale frame index off a shorter image — gpui
/// indexes its frame list bare.
///
/// An id alone adds no hitbox, so the slider's drag handling is untouched.
fn media_image(image: Arc<gpui::Image>) -> gpui::Stateful<gpui::Img> {
    let id = image.id;
    gpui::img(image).id(("diff-image", id))
}

impl SourcefourWindow {
    /// Height of one rendered diff line in either layout, derived from the
    /// line-height setting. The `uniform_list` rows and the scrollbar math
    /// must use this same number or scrolling desynchronizes.
    fn diff_row_height(&self) -> f32 {
        self.settings.diff.row_height()
    }

    /// Closes the diff overlay, returning focus to the history.
    pub(crate) fn close_diff(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.diff_view = None;
        self.diff_switcher_open = false;
        self.clear_preview_selection();
        self.focus.focus(window);
        cx.notify();
    }

    pub(super) fn open_diff_file_switcher(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.diff_view.is_none() {
            return;
        }
        self.diff_switcher_open = true;
        self.diff_switcher_selection = 0;
        self.diff_file_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.diff_file_input
            .read(cx)
            .focus_handle
            .clone()
            .focus(window);
        cx.notify();
    }

    pub(super) fn close_diff_file_switcher(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.diff_switcher_open = false;
        self.diff_focus.focus(window);
        cx.notify();
    }

    pub(super) fn open_selected_diff_file(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(index) = self
            .diff_file_matches(cx)
            .get(self.diff_switcher_selection)
            .copied()
        else {
            return;
        };
        self.open_diff_file_at(index, window, cx);
    }

    pub(super) fn move_diff_switcher_selection(
        &mut self,
        delta: isize,
        cx: &mut gpui::Context<Self>,
    ) {
        let count = self.diff_file_matches(cx).len().min(10);
        self.diff_switcher_selection = offset_index(self.diff_switcher_selection, delta, count);
        cx.notify();
    }

    fn open_diff_file_at(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(view) = &self.diff_view else {
            return;
        };
        let Some(file) = view.files.get(index).cloned() else {
            return;
        };
        let staged = match &view.origin {
            DiffOrigin::Commit(_) => None,
            DiffOrigin::WorkingTree { staged, .. } => Some(*staged),
        };
        self.diff_switcher_open = false;
        if let Some(staged) = staged {
            self.open_worktree_diff(&file, staged, window, cx);
        } else {
            self.open_diff(&file, window, cx);
        }
    }

    fn diff_file_matches(&self, cx: &gpui::App) -> Vec<usize> {
        let Some(view) = &self.diff_view else {
            return Vec::new();
        };
        let query = self
            .diff_file_input
            .read(cx)
            .content
            .to_string()
            .to_lowercase();
        let mut matches: Vec<_> = view
            .files
            .iter()
            .enumerate()
            .filter_map(|(index, file)| {
                let path = file
                    .new_path
                    .as_ref()
                    .or(file.old_path.as_ref())?
                    .display_lossy();
                let lowercase = path.to_lowercase();
                lowercase.contains(&query).then(|| {
                    let basename = lowercase.rsplit('/').next().unwrap_or(&lowercase);
                    (index, usize::from(!basename.contains(&query)))
                })
            })
            .collect();
        matches.sort_by_key(|(index, basename_rank)| (*basename_rank, *index));
        matches.into_iter().map(|(index, _)| index).collect()
    }

    pub(super) fn clear_diff_selection(&mut self) -> bool {
        self.diff_view
            .as_mut()
            .is_some_and(|view| view.selection.take().is_some())
    }

    fn select_diff_line(
        &mut self,
        coordinate: DiffCoordinate,
        extend: bool,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(view) = &mut self.diff_view else {
            return;
        };
        view.selection = match (extend, view.selection) {
            (true, Some(selection)) if selection.anchor.side == coordinate.side => {
                Some(DiffSelection {
                    anchor: selection.anchor,
                    head: coordinate,
                })
            }
            _ => Some(DiffSelection {
                anchor: coordinate,
                head: coordinate,
            }),
        };
        cx.notify();
    }

    pub(super) fn copy_diff_selection(&self, cx: &mut gpui::App) -> bool {
        let Some(view) = &self.diff_view else {
            return false;
        };
        let Some(selection) = view.selection else {
            return false;
        };
        let Some(diff) = view.text_diff() else {
            return false;
        };
        let side = match selection.anchor.side {
            DiffSide::Old => diff.old.as_ref(),
            DiffSide::New => diff.new.as_ref(),
        };
        let Some(side) = side else {
            return false;
        };
        let start = selection.anchor.line.min(selection.head.line);
        let end = selection.anchor.line.max(selection.head.line);
        let text = (start..=end)
            .filter_map(|line| side.line(line))
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        true
    }

    /// Opens the diff overlay for one changed file of the selection (§6.11).
    pub(super) fn open_diff(
        &mut self,
        file: &ChangedFile,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(oid) = self.history.selected_commit() else {
            return;
        };
        let Some(location) = self.location.clone() else {
            return;
        };
        let Some(paths) = DiffPaths::for_file(file) else {
            return;
        };
        let request = FileDiffRequest {
            oid,
            parent: self.compare_parent,
            paths,
        };
        let cached = self.cached_commit_diff(&request);
        let files: Arc<[ChangedFile]> = self
            .files
            .as_ref()
            .map_or_else(|| vec![file.clone()], |files| files.files.clone())
            .into();
        let file_index = files
            .iter()
            .position(|candidate| candidate == file)
            .unwrap_or(0);
        self.diff_view = Some(DiffView::for_commit(
            request.clone(),
            file.status,
            files,
            file_index,
            self.preferred_diff_mode,
        ));
        self.diff_request += 1;
        let token = self.diff_request;
        self.diff_scroll = UniformListScrollHandle::new();
        self.diff_focus.focus(window);
        if let Some(diff) = cached {
            self.set_diff_content(token, Ok(DiffContent::Text(diff)), cx);
            return;
        }
        let ffmpeg_dir = self.settings.video.ffmpeg_dir.clone();
        cx.spawn(async move |this, cx| {
            let diff = cx
                .background_executor()
                .spawn(async move {
                    sourcefour_git::file_diff(&location, &request, ffmpeg_dir.as_deref())
                })
                .await;
            this.update(cx, |this, cx| {
                this.set_diff_content(token, diff.map(|diff| diff.content), cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Opens the diff overlay for one working-tree file: the index against
    /// HEAD for a staged entry, the filesystem against the index otherwise.
    pub(super) fn open_worktree_diff(
        &mut self,
        file: &ChangedFile,
        staged: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(location) = self.location.clone() else {
            return;
        };
        let Some(paths) = DiffPaths::for_file(file) else {
            return;
        };
        let files: Arc<[ChangedFile]> = self
            .working_tree_status
            .as_ref()
            .map(|status| {
                if staged {
                    &status.staged
                } else {
                    &status.unstaged
                }
            })
            .map_or_else(|| vec![file.clone()], Clone::clone)
            .into();
        let file_index = files
            .iter()
            .position(|candidate| candidate == file)
            .unwrap_or(0);
        self.diff_view = Some(DiffView::for_working_tree(
            paths.clone(),
            staged,
            file.status,
            files,
            file_index,
            self.preferred_diff_mode,
        ));
        self.diff_request += 1;
        let token = self.diff_request;
        self.diff_scroll = UniformListScrollHandle::new();
        self.diff_focus.focus(window);
        let ffmpeg_dir = self.settings.video.ffmpeg_dir.clone();
        cx.spawn(async move |this, cx| {
            let content = cx
                .background_executor()
                .spawn(async move {
                    sourcefour_git::worktree_file_diff(
                        &location,
                        &paths,
                        staged,
                        ffmpeg_dir.as_deref(),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                this.set_diff_content(token, content, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Applies a loaded diff to the open view, unless the request went stale.
    fn set_diff_content(
        &mut self,
        token: u64,
        content: Result<DiffContent, sourcefour_model::RepoFailure>,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.diff_request != token {
            return;
        }
        let syntax_enabled = self.settings.diff.syntax_highlighting;
        let mut enrichment = None;
        let mut cache_entry = None;
        if let Some(view) = &mut self.diff_view {
            let content = content.unwrap_or_else(|failure| DiffContent::Unavailable {
                message: failure.user.message,
            });
            if let (DiffOrigin::Commit(request), DiffContent::Text(diff)) = (&view.origin, &content)
            {
                cache_entry = Some((request.clone(), Arc::clone(diff)));
            }
            view.replace_content(content);
            if syntax_enabled && let Some(DiffContent::Text(diff)) = &view.content {
                enrichment = Some((Arc::clone(diff), view.origin.path().clone()));
            }
        }
        if let Some((request, diff)) = cache_entry {
            self.cache_commit_diff(request, diff);
        }
        self.reset_diff_wrap_list(cx);
        if let Some((diff, path)) = enrichment {
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { crate::diff_highlight::enrich(&diff, &path) })
                    .await;
                this.update(cx, |this, cx| {
                    if this.diff_request != token {
                        return;
                    }
                    if let Some(view) = &mut this.diff_view {
                        view.highlight = result.highlight.map(Arc::new);
                        view.highlight_message = result.message;
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn cached_commit_diff(&mut self, request: &FileDiffRequest) -> Option<Arc<TextDiff>> {
        let index = self
            .diff_cache
            .iter()
            .position(|entry| &entry.request == request)?;
        let entry = self.diff_cache.remove(index)?;
        let diff = Arc::clone(&entry.diff);
        self.diff_cache.push_back(entry);
        Some(diff)
    }

    fn cache_commit_diff(&mut self, request: FileDiffRequest, diff: Arc<TextDiff>) {
        const MAX_ENTRIES: usize = 8;
        const MAX_BYTES: usize = 64 * 1024 * 1024;
        let bytes = diff
            .old
            .iter()
            .chain(diff.new.iter())
            .map(|side| side.text().len() + side.line_count() * size_of::<Range<usize>>())
            .sum::<usize>()
            + diff.changes.len() * size_of::<sourcefour_model::TextChange>();
        if bytes > MAX_BYTES {
            return;
        }
        if let Some(index) = self
            .diff_cache
            .iter()
            .position(|entry| entry.request == request)
        {
            self.diff_cache.remove(index);
        }
        self.diff_cache.push_back(super::DiffCacheEntry {
            request,
            diff,
            bytes,
        });
        while self.diff_cache.len() > MAX_ENTRIES
            || self
                .diff_cache
                .iter()
                .map(|entry| entry.bytes)
                .sum::<usize>()
                > MAX_BYTES
        {
            self.diff_cache.pop_front();
        }
    }

    /// The full-window diff overlay (§6.11), closed by Escape, ✕, or a click
    /// outside the panel.
    pub(super) fn diff_overlay(
        &self,
        cx: &mut gpui::Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let view = self.diff_view.as_ref()?;
        let line_count = view.rows().len();
        let body = self.diff_body(view, line_count, cx);
        Some(
            super::modal_backdrop("diff-overlay", &self.theme)
                .key_context("Diff")
                .track_focus(&self.diff_focus)
                // Occlusion also stops the root's handlers, so scrub drags
                // route here.
                .on_mouse_move(
                    cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                        this.drag_move(
                            event.position.x.0,
                            event.position.y.0,
                            event.pressed_button,
                            window,
                            cx,
                        );
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
                                .children(if preview::showing(view) {
                                    None
                                } else {
                                    self.diff_scrollbar(cx)
                                })
                                .children(self.diff_file_switcher(view, cx)),
                        ),
                ),
        )
    }

    fn diff_file_switcher(
        &self,
        view: &DiffView,
        cx: &mut gpui::Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if !self.diff_switcher_open {
            return None;
        }
        let matches = self.diff_file_matches(cx);
        Some(
            div()
                .id("diff-file-switcher")
                .absolute()
                .top(px(12.0))
                .left(gpui::relative(0.5))
                .ml(px(-260.0))
                .w(px(520.0))
                .max_h(px(420.0))
                .overflow_hidden()
                .rounded(px(8.0))
                .border_1()
                .border_color(self.theme.border_strong)
                .bg(self.theme.bg_chrome)
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .h(px(38.0))
                        .flex()
                        .items_center()
                        .px(px(12.0))
                        .border_b_1()
                        .border_color(self.theme.border)
                        .text_size(px(12.0))
                        .text_color(self.theme.text_primary)
                        .child(self.diff_file_input.clone()),
                )
                .children(matches.iter().take(10).enumerate().filter_map(
                    |(match_index, file_index)| {
                        let file = view.files.get(*file_index)?;
                        let path = file
                            .new_path
                            .as_ref()
                            .or(file.old_path.as_ref())?
                            .display_lossy();
                        let target = *file_index;
                        Some(
                            div()
                                .id(("diff-file-result", target))
                                .h(px(34.0))
                                .flex()
                                .items_center()
                                .gap(px(9.0))
                                .px(px(12.0))
                                .cursor_pointer()
                                .bg(if match_index == self.diff_switcher_selection {
                                    self.theme.bg_selected
                                } else {
                                    self.theme.bg_chrome
                                })
                                .hover(|style| style.bg(self.theme.bg_hover))
                                .child(
                                    div()
                                        .w(px(14.0))
                                        .flex_none()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(change_color(&self.theme, file.status))
                                        .child(change_letter(file.status)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(1.0))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .font_family(self.mono_font())
                                        .text_size(px(11.5))
                                        .text_color(self.theme.text_primary)
                                        .child(path),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_diff_file_at(target, window, cx);
                                })),
                        )
                    },
                ))
                .children(matches.is_empty().then(|| {
                    div()
                        .h(px(44.0))
                        .flex()
                        .items_center()
                        .px(px(12.0))
                        .text_size(px(11.5))
                        .text_color(self.theme.text_faint)
                        .child("No changed files match")
                })),
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
            // Preview renders both versions of the document, the old side
            // left and the new side right — the raw text stays under Source.
            Some(DiffContent::Text(_)) if preview::showing(view) => match &view.preview {
                Some(preview) => {
                    let started = std::time::Instant::now();
                    // A layout belongs to the frame that painted it; the
                    // blocks below register this frame's as they build.
                    self.preview_layouts.borrow_mut().clear();
                    let panes = div()
                        .size_full()
                        .flex()
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(1.0))
                                .child(preview::pane(&preview.old, PreviewSide::Old)),
                        )
                        .child(div().w(px(1.0)).flex_none().bg(self.theme.border))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(1.0))
                                .child(preview::pane(&preview.new, PreviewSide::New)),
                        )
                        .into_any_element();
                    preview::build_probe(
                        started,
                        preview.old.blocks.len() + preview.new.blocks.len(),
                    );
                    panes
                }
                None => self.diff_notice("Rendering preview…").into_any_element(),
            },
            Some(DiffContent::Text(diff)) if diff.is_unchanged() => {
                self.diff_notice("No textual changes.").into_any_element()
            }
            Some(DiffContent::Text(_)) => self.diff_text_list(view, line_count, cx),
            // A video no decoder opened has nothing to lay out two ways, so
            // the card carries the numbers instead.
            Some(DiffContent::Video { .. }) if !view.has_poster() => {
                self.video_card_view(view).into_any_element()
            }
            // With a poster it is an image comparison that also has a caption,
            // and both layouts treat it as exactly that.
            Some(DiffContent::Image { .. } | DiffContent::Video { .. })
                if view.mode == DiffMode::Split =>
            {
                self.image_split_view(view).into_any_element()
            }
            Some(DiffContent::Image { .. } | DiffContent::Video { .. }) => {
                self.image_slider_view(view, cx).into_any_element()
            }
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

    /// The virtualized line list for text content, in the current layout.
    fn diff_text_list(
        &self,
        view: &DiffView,
        line_count: usize,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let started = std::time::Instant::now();
        if self.settings.diff.wrap
            && let Some(list) = &view.wrap_list
        {
            let element = gpui::list(list.clone()).size_full().into_any_element();
            diff_build_probe(started, view, true);
            return element;
        }
        let element = match &view.content {
            Some(DiffContent::Text(_)) if view.mode == DiffMode::Split => uniform_list(
                cx.entity(),
                "diff-split-rows",
                view.rows().len(),
                move |this, range, _window, cx| {
                    let Some(view) = this.diff_view.as_ref() else {
                        return Vec::new();
                    };
                    range
                        .filter_map(|index| view.rows().get(index).cloned().map(|row| (index, row)))
                        .map(|(index, row)| this.split_row_view(view, index, &row, cx))
                        .collect()
                },
            )
            .with_width_from_item(view.widest_row())
            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
            .track_scroll(self.diff_scroll.clone())
            .size_full()
            .into_any_element(),
            Some(DiffContent::Text(_)) => uniform_list(
                cx.entity(),
                "diff-lines",
                line_count,
                move |this, range, _window, cx| {
                    let Some(view) = this.diff_view.as_ref() else {
                        return Vec::new();
                    };
                    range
                        .filter_map(|index| view.rows().get(index).cloned().map(|row| (index, row)))
                        .map(|(index, row)| this.diff_line_row(view, index, &row, cx))
                        .collect()
                },
            )
            .with_width_from_item(view.widest_row())
            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
            .track_scroll(self.diff_scroll.clone())
            .size_full()
            .into_any_element(),
            // Only text content reaches here; diff_body routed the rest.
            _ => self.diff_notice("No textual changes.").into_any_element(),
        };
        diff_build_probe(started, view, false);
        element
    }

    /// The diff overlay's title bar: status, path, counts, and close.
    #[expect(
        clippy::too_many_lines,
        reason = "the diff toolbar composes independent controls in visual order"
    )]
    pub(super) fn diff_header(
        &self,
        view: &DiffView,
        line_count: usize,
        cx: &mut gpui::Context<Self>,
    ) -> Div {
        let status_color = change_color(&self.theme, view.status);
        let hunk_count = view.hunk_rows().len();
        let previous_hunk_enabled = view.current_hunk > 0;
        let next_hunk_enabled = view.current_hunk + 1 < hunk_count;
        let previous_file_enabled = view.file_index > 0;
        let next_file_enabled = view.file_index + 1 < view.files.len();
        div()
            .h(px(40.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.0))
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
            .children(
                view.text_diff()
                    .is_some_and(TextDiff::is_lossy)
                    .then_some("Invalid UTF-8 replaced")
                    .or(view.highlight_message)
                    .map(|message| {
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(self.theme.text_faint)
                            .child(message)
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(self.theme.text_faint)
                            .child(format!("{} / {}", view.file_index + 1, view.files.len())),
                    )
                    .child(
                        self.icon_segment_button(
                            "previous-diff-file",
                            "icons/chevron-left.svg",
                            previous_file_enabled,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.navigate_diff_file(-1, window, cx);
                        })),
                    )
                    .child(
                        self.icon_segment_button(
                            "next-diff-file",
                            "icons/chevron-right.svg",
                            next_file_enabled,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.navigate_diff_file(1, window, cx);
                        })),
                    ),
            )
            .children((hunk_count > 0).then(|| {
                let count = hunk_count;
                div()
                    .flex_none()
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(self.theme.text_faint)
                            .child(format!("hunk {} / {count}", view.current_hunk + 1)),
                    )
                    .child(
                        self.icon_segment_button(
                            "previous-diff-hunk",
                            "icons/chevron-up.svg",
                            previous_hunk_enabled,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.navigate_diff_hunk(-1, cx))),
                    )
                    .child(
                        self.icon_segment_button(
                            "next-diff-hunk",
                            "icons/chevron-down.svg",
                            next_hunk_enabled,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.navigate_diff_hunk(1, cx))),
                    )
            }))
            .children(preview::applies(view).then(|| self.preview_toggle(view.show_preview, cx)))
            // The layout control chooses how source lines lay out; while both
            // rendered documents show, it has nothing to say. Nor does it for a
            // video with no frame to lay out, where both choices draw the card.
            .children((!preview::showing(view) && view.offers_layouts()).then(|| {
                div()
                    .flex_none()
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(self.theme.border_strong)
                    .overflow_hidden()
                    .child(self.diff_mode_button(
                        if view.compares_frames() {
                            "Slider"
                        } else {
                            "Unified"
                        },
                        DiffMode::Unified,
                        view.mode,
                        cx,
                    ))
                    .child(self.diff_mode_button(
                        if view.compares_frames() {
                            "Side by side"
                        } else {
                            "Split"
                        },
                        DiffMode::Split,
                        view.mode,
                        cx,
                    ))
            }))
            .children(
                (view.text_diff().is_some() && !preview::showing(view)).then(|| {
                    div()
                        .flex_none()
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .child(
                            self.segment_button("Full context", view.full_context)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_diff_context(cx);
                                })),
                        )
                        .child(
                            self.segment_button("Wrap", self.settings.diff.wrap)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let enabled = !this.settings.diff.wrap;
                                    this.update_settings(cx, |settings| {
                                        settings.diff.wrap = enabled;
                                    });
                                    this.reset_diff_wrap_list(cx);
                                })),
                        )
                        .child(
                            self.segment_button("Whitespace", self.settings.diff.show_whitespace)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let enabled = !this.settings.diff.show_whitespace;
                                    this.update_settings(cx, |settings| {
                                        settings.diff.show_whitespace = enabled;
                                    });
                                })),
                        )
                }),
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
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(self.theme.text_faint)
                            .child("Esc"),
                    )
                    .child(
                        div()
                            .id("diff-close")
                            .h(px(24.0))
                            .flex()
                            .items_center()
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
                    ),
            )
    }

    /// The look of one segment in a header's segmented control, without the
    /// action: every control in the overlay's header wears the same segment
    /// and differs only in what clicking it does.
    pub(super) fn segment_button(
        &self,
        label: &'static str,
        selected: bool,
    ) -> gpui::Stateful<Div> {
        div()
            .id(label)
            .h(px(24.0))
            .flex()
            .items_center()
            .px(px(9.0))
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
            .child(label)
    }

    fn icon_segment_button(
        &self,
        id: &'static str,
        path: &'static str,
        enabled: bool,
    ) -> gpui::Stateful<Div> {
        div()
            .id(id)
            .size(px(24.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .bg(self.theme.bg_list)
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(self.theme.bg_hover))
            })
            .when(!enabled, |button| button.opacity(0.4))
            .child(
                svg()
                    .path(path)
                    .size(px(14.0))
                    .text_color(self.theme.text_secondary),
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
        self.segment_button(label, mode == active)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_diff_mode(mode, cx);
            }))
    }

    pub(super) fn set_diff_mode(&mut self, mode: DiffMode, cx: &mut gpui::Context<Self>) {
        if let Some(view) = &mut self.diff_view {
            view.set_mode(mode);
        }
        self.preferred_diff_mode = mode;
        self.diff_scroll = UniformListScrollHandle::new();
        self.reset_diff_wrap_list(cx);
        self.persist_ui_state(cx);
        cx.notify();
    }

    pub(super) fn apply_responsive_diff_mode(
        &mut self,
        viewport_width: f32,
        cx: &mut gpui::Context<Self>,
    ) {
        let desired = if viewport_width - 52.0 < 900.0 {
            DiffMode::Unified
        } else {
            self.preferred_diff_mode
        };
        let changed = self
            .diff_view
            .as_ref()
            .is_some_and(|view| view.mode != desired);
        if !changed {
            return;
        }
        if let Some(view) = &mut self.diff_view {
            view.set_mode(desired);
            view.current_hunk = 0;
        }
        self.diff_scroll = UniformListScrollHandle::new();
        self.reset_diff_wrap_list(cx);
    }

    fn toggle_diff_context(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some(view) = &mut self.diff_view {
            view.toggle_full_context();
        }
        self.diff_scroll = UniformListScrollHandle::new();
        self.reset_diff_wrap_list(cx);
        cx.notify();
    }

    pub(super) fn reset_diff_wrap_list(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(view) = &mut self.diff_view else {
            return;
        };
        let count = view.rows().len();
        let weak = cx.entity().downgrade();
        view.wrap_list = Some(gpui::ListState::new(
            count,
            gpui::ListAlignment::Top,
            px(200.0),
            move |index, _window, cx| {
                let Some(entity) = weak.upgrade() else {
                    return div().into_any_element();
                };
                let coordinate = entity.read(cx).diff_row_coordinate(index);
                let row = entity.read(cx).wrapped_diff_row(index);
                let Some(coordinate) = coordinate else {
                    return row;
                };
                let target = entity.downgrade();
                div()
                    .id(("wrapped-diff-row", cell_coordinate_key(coordinate)))
                    .cursor_pointer()
                    .child(row)
                    .on_click(move |event: &gpui::ClickEvent, _, cx| {
                        if let Some(entity) = target.upgrade() {
                            entity.update(cx, |this, cx| {
                                this.select_diff_line(coordinate, event.modifiers().shift, cx);
                            });
                        }
                    })
                    .into_any_element()
            },
        ));
    }

    fn diff_row_coordinate(&self, index: usize) -> Option<DiffCoordinate> {
        let view = self.diff_view.as_ref()?;
        let crate::diff_split::DiffRow::Line { left, right } = view.rows().get(index)? else {
            return None;
        };
        let cell = right.as_ref().or(left.as_ref())?;
        Some(DiffCoordinate {
            side: cell.side,
            line: cell.line,
            byte_offset: None,
        })
    }

    pub(super) fn navigate_diff_hunk(&mut self, delta: isize, cx: &mut gpui::Context<Self>) {
        let row_height = self.diff_row_height();
        let Some(view) = &mut self.diff_view else {
            return;
        };
        let hunks = view.hunk_rows();
        if hunks.len() < 2 {
            return;
        }
        view.current_hunk = offset_index(view.current_hunk, delta, hunks.len());
        let row = hunks[view.current_hunk];
        if self.settings.diff.wrap {
            if let Some(list) = &view.wrap_list {
                scroll_wrapped_list_to_hunk(list, row);
            }
        } else {
            let handle = self.diff_scroll.0.borrow();
            let offset = handle.base_handle.offset();
            let viewport_height = handle.base_handle.bounds().size.height.0;
            let scroll_top = hunk_scroll_top(row, row_height, view.rows().len(), viewport_height);
            handle
                .base_handle
                .set_offset(gpui::point(offset.x, px(-scroll_top)));
        }
        cx.notify();
    }

    pub(super) fn navigate_diff_file(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(view) = &self.diff_view else {
            return;
        };
        let next = offset_index(view.file_index, delta, view.files.len());
        if next == view.file_index {
            return;
        }
        let file = view.files[next].clone();
        let staged = match &view.origin {
            DiffOrigin::Commit(_) => None,
            DiffOrigin::WorkingTree { staged, .. } => Some(*staged),
        };
        if let Some(staged) = staged {
            self.open_worktree_diff(&file, staged, window, cx);
        } else {
            self.open_diff(&file, window, cx);
        }
    }

    /// Side-by-side before/after panes for an image comparison (§6.11).
    pub(super) fn image_split_view(&self, view: &DiffView) -> Div {
        let (before, after) = view.media_summaries();
        div()
            .size_full()
            .flex()
            .child(self.image_pane(
                "Before",
                view.before_image.clone(),
                "Added — no before",
                before,
            ))
            .child(div().w(px(1.0)).flex_none().h_full().bg(self.theme.border))
            .child(self.image_pane(
                "After",
                view.after_image.clone(),
                "Deleted — no after",
                after,
            ))
    }

    /// Side-by-side cards for a video no decoder could open a frame of, under
    /// the one sentence that says so when that is why.
    pub(super) fn video_card_view(&self, view: &DiffView) -> Div {
        let (before, after) = view.video_facts();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h(px(1.0))
                    .flex()
                    .child(self.video_card("Before", before, "Added — no before"))
                    .child(div().w(px(1.0)).flex_none().h_full().bg(self.theme.border))
                    .child(self.video_card("After", after, "Deleted — no after")),
            )
            .children(missing_tools_note(before, after).map(|note| {
                div()
                    .flex_none()
                    .flex()
                    .justify_center()
                    .pb(px(16.0))
                    .text_size(px(11.0))
                    .text_color(self.theme.text_faint)
                    .child(note)
            }))
    }

    /// One labeled half of the card view: what the container said, and no more.
    fn video_card(
        &self,
        label: &'static str,
        info: Option<VideoInfo>,
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
            .child(match info {
                Some(info) => div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .text_size(px(26.0))
                            .text_color(self.theme.text_faint)
                            .child("▶"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(self.theme.text_secondary)
                            .child(video_line(info)),
                    )
                    .into_any_element(),
                None => div()
                    .text_size(px(12.0))
                    .text_color(self.theme.text_faint)
                    .child(missing)
                    .into_any_element(),
            })
            .child(self.image_side_chips(&[label]))
    }

    /// One labeled half of the side-by-side image view.
    pub(super) fn image_pane(
        &self,
        label: &'static str,
        image: Option<Arc<gpui::Image>>,
        missing: &'static str,
        summary: Option<gpui::SharedString>,
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
                Some(image) => media_image(image)
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
            .children(summary.map(|summary| self.media_footer_chips(&[summary])))
    }

    /// A small chip naming an image side.
    pub(super) fn image_side_chip(&self, label: impl Into<gpui::SharedString>) -> Div {
        div()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(4.0))
            .bg(self.theme.hud())
            .text_size(px(9.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(self.theme.text_faint)
            .child(label.into())
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
            .children(labels.iter().map(|label| self.image_side_chip(*label)))
    }

    /// The same row along the bottom, for what a side measures rather than
    /// which side it is. One label sits left, two split to the edges — the
    /// same placement as the naming chips directly above them.
    fn media_footer_chips(&self, labels: &[gpui::SharedString]) -> Div {
        div()
            .absolute()
            .bottom(px(8.0))
            .left(px(8.0))
            .right(px(8.0))
            .flex()
            .justify_between()
            .children(
                labels
                    .iter()
                    .map(|label| self.image_side_chip(label.clone())),
            )
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
            .children(view.compares_frames().then(|| {
                let (before, after) = view.media_summaries();
                // Both edges always get a chip so the pair keeps the alignment
                // the naming chips above them set; a side that is not there
                // says so rather than letting the other slide across.
                self.media_footer_chips(&[
                    before.unwrap_or_else(|| "—".into()),
                    after.unwrap_or_else(|| "—".into()),
                ])
            }))
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
                media_image(image)
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
    pub(super) fn split_row_view(
        &self,
        view: &DiffView,
        row_index: usize,
        row: &crate::diff_split::DiffRow,
        cx: &mut gpui::Context<Self>,
    ) -> Div {
        use crate::diff_split::DiffRow;
        let row = match row {
            DiffRow::Hunk {
                old_lines,
                new_lines,
            } => self.diff_banner(hunk_label(old_lines, new_lines), false),
            DiffRow::Gap {
                old_lines,
                new_lines,
            } => self.diff_banner(gap_label(old_lines, new_lines), true),
            DiffRow::Marker { side } => self.diff_banner(eof_marker_label(*side), true),
            DiffRow::Line { left, right } => div()
                .h(px(self.diff_row_height()))
                .w_full()
                .flex()
                .child(self.split_half(view, left.as_ref(), cx))
                .child(div().w(px(1.0)).flex_none().h_full().bg(self.theme.border))
                .child(self.split_half(view, right.as_ref(), cx)),
        };
        self.outline_diff_row(row, view.hunk_edges(row_index))
    }

    /// One half of a split row: number, marker tint, and content.
    pub(super) fn split_half(
        &self,
        view: &DiffView,
        cell: Option<&crate::diff_split::DiffCell>,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        use crate::diff_split::CellKind;
        let base = div().flex_1().min_w(px(1.0)).h_full().flex().items_center();
        let rule = || {
            div()
                .w(px(1.0))
                .flex_none()
                .h_full()
                .mr(px(6.0))
                .bg(self.theme.gutter_rule())
        };
        let Some(cell) = cell else {
            return base
                .bg(self.theme.bg_panel.opacity(0.4))
                .child(div().w(px(44.0)).flex_none().h_full())
                .child(rule())
                .into_any_element();
        };
        let (marker, text_color, background) = match cell.kind {
            CellKind::Addition => ("+", self.theme.green, Some(self.theme.tint_added())),
            CellKind::Deletion => ("-", self.theme.red, Some(self.theme.tint_removed())),
            CellKind::Context => (" ", self.theme.text_secondary, None),
        };
        let selected = view
            .selection
            .is_some_and(|selection| selection.contains(cell));
        let coordinate = DiffCoordinate {
            side: cell.side,
            line: cell.line,
            byte_offset: None,
        };
        base.id(("split-diff-cell", cell_key(cell)))
            .bg(if selected {
                self.theme.accent.opacity(0.35)
            } else {
                background.unwrap_or(gpui::transparent_black())
            })
            .cursor_pointer()
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                this.select_diff_line(coordinate, event.modifiers().shift, cx);
            }))
            .child(div().w(px(2.0)).flex_none().h_full().bg(text_color))
            .child(
                div()
                    .w(px(44.0))
                    .flex_none()
                    .pr(px(6.0))
                    .font_family(self.mono_font())
                    .text_size(px(10.5))
                    .text_color(self.theme.text_faint)
                    .child((cell.line + 1).to_string()),
            )
            .child(rule())
            .child(
                div()
                    .w(px(14.0))
                    .flex_none()
                    .font_family(self.mono_font())
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
                    .font_family(self.mono_font())
                    .text_size(px(11.0))
                    .text_color(text_color)
                    .child(self.styled_diff_text(view, cell)),
            )
            .into_any_element()
    }

    fn styled_diff_text(
        &self,
        view: &DiffView,
        cell: &crate::diff_split::DiffCell,
    ) -> gpui::StyledText {
        use crate::diff_split::CellKind;
        let text = cell_text(view.text_diff(), cell);
        if self.settings.diff.show_whitespace {
            return gpui::StyledText::new(visible_whitespace(
                text,
                usize::from(self.settings.diff.tab_width.clamp(1, 16)),
            ));
        }
        let Some(highlight) = &view.highlight else {
            return gpui::StyledText::new(text.to_owned());
        };
        let syntax = highlight.syntax(cell.side, cell.line);
        let intraline = highlight.intraline(cell.side, cell.line);
        let mut boundaries = vec![0, text.len()];
        boundaries.extend(
            syntax
                .iter()
                .flat_map(|span| [span.range.start, span.range.end]),
        );
        boundaries.extend(intraline.iter().flat_map(|range| [range.start, range.end]));
        boundaries.retain(|boundary| *boundary <= text.len() && text.is_char_boundary(*boundary));
        boundaries.sort_unstable();
        boundaries.dedup();
        let semantic_color = match cell.kind {
            CellKind::Addition => self.theme.green,
            CellKind::Deletion => self.theme.red,
            CellKind::Context => self.theme.accent,
        };
        let runs = boundaries.windows(2).filter_map(|window| {
            let range = window[0]..window[1];
            let syntax = syntax
                .iter()
                .find(|span| span.range.start <= range.start && span.range.end >= range.end);
            let emphasized = intraline
                .iter()
                .any(|span| span.start <= range.start && span.end >= range.end);
            if syntax.is_none() && !emphasized {
                return None;
            }
            let color = syntax.map(|span| {
                let (red, green, blue) = span.rgb;
                gpui::rgb((u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue)).into()
            });
            Some((
                range,
                gpui::HighlightStyle {
                    color,
                    font_weight: syntax.filter(|span| span.bold).map(|_| FontWeight::BOLD),
                    font_style: syntax
                        .filter(|span| span.italic)
                        .map(|_| gpui::FontStyle::Italic),
                    background_color: emphasized.then(|| semantic_color.opacity(0.28)),
                    ..gpui::HighlightStyle::default()
                },
            ))
        });
        gpui::StyledText::new(text.to_owned()).with_highlights(runs)
    }

    fn wrapped_diff_row(&self, index: usize) -> gpui::AnyElement {
        use crate::diff_split::DiffRow;
        let Some(view) = &self.diff_view else {
            return div().into_any_element();
        };
        let Some(row) = view.rows().get(index) else {
            return div().into_any_element();
        };
        let row = match row {
            DiffRow::Hunk {
                old_lines,
                new_lines,
            } => self.diff_banner(hunk_label(old_lines, new_lines), false),
            DiffRow::Gap {
                old_lines,
                new_lines,
            } => self.diff_banner(gap_label(old_lines, new_lines), true),
            DiffRow::Marker { side } => self.diff_banner(eof_marker_label(*side), true),
            DiffRow::Line { left, right } if view.mode == DiffMode::Split => div()
                .w_full()
                .min_h(px(self.diff_row_height()))
                .flex()
                .child(self.wrapped_split_half(view, left.as_ref()))
                .child(div().w(px(1.0)).flex_none().h_full().bg(self.theme.border))
                .child(self.wrapped_split_half(view, right.as_ref())),
            DiffRow::Line { left, right } => {
                use crate::diff_split::CellKind;
                let Some(cell) = right.as_ref().or(left.as_ref()) else {
                    return div().into_any_element();
                };
                let (marker, text_color, background) = match cell.kind {
                    CellKind::Addition => ("+", self.theme.green, self.theme.tint_added()),
                    CellKind::Deletion => ("-", self.theme.red, self.theme.tint_removed()),
                    CellKind::Context => {
                        (" ", self.theme.text_secondary, gpui::transparent_black())
                    }
                };
                let number = |cell: Option<&crate::diff_split::DiffCell>| {
                    div()
                        .w(px(44.0))
                        .flex_none()
                        .pr(px(6.0))
                        .font_family(self.mono_font())
                        .text_size(px(10.5))
                        .text_color(self.theme.text_faint)
                        .child(cell.map_or_else(String::new, |cell| (cell.line + 1).to_string()))
                };
                div()
                    .w_full()
                    .min_h(px(self.diff_row_height()))
                    .flex()
                    .items_start()
                    .bg(background)
                    .child(div().w(px(2.0)).flex_none().h_full().bg(text_color))
                    .child(number(left.as_ref()))
                    .child(number(right.as_ref()))
                    .child(
                        div()
                            .w(px(1.0))
                            .flex_none()
                            .h_full()
                            .mr(px(6.0))
                            .bg(self.theme.gutter_rule()),
                    )
                    .child(
                        div()
                            .w(px(14.0))
                            .flex_none()
                            .font_family(self.mono_font())
                            .text_size(px(11.0))
                            .text_color(text_color)
                            .child(marker),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(1.0))
                            .whitespace_normal()
                            .font_family(self.mono_font())
                            .text_size(px(11.0))
                            .text_color(text_color)
                            .child(self.styled_diff_text(view, cell)),
                    )
            }
        };
        self.outline_diff_row(row, view.hunk_edges(index))
            .into_any_element()
    }

    fn wrapped_split_half(
        &self,
        view: &DiffView,
        cell: Option<&crate::diff_split::DiffCell>,
    ) -> Div {
        use crate::diff_split::CellKind;
        let Some(cell) = cell else {
            return div().flex_1().h_full().bg(self.theme.bg_panel.opacity(0.4));
        };
        let (marker, color, background) = match cell.kind {
            CellKind::Addition => ("+", self.theme.green, self.theme.tint_added()),
            CellKind::Deletion => ("-", self.theme.red, self.theme.tint_removed()),
            CellKind::Context => (" ", self.theme.text_secondary, gpui::transparent_black()),
        };
        div()
            .flex_1()
            .min_w(px(1.0))
            .h_full()
            .flex()
            .items_start()
            .bg(background)
            .child(div().w(px(2.0)).flex_none().h_full().bg(color))
            .child(
                div()
                    .w(px(44.0))
                    .flex_none()
                    .pr(px(6.0))
                    .font_family(self.mono_font())
                    .text_size(px(10.5))
                    .text_color(self.theme.text_faint)
                    .child((cell.line + 1).to_string()),
            )
            .child(
                div()
                    .w(px(14.0))
                    .flex_none()
                    .font_family(self.mono_font())
                    .text_size(px(11.0))
                    .text_color(color)
                    .child(marker),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(1.0))
                    .whitespace_normal()
                    .font_family(self.mono_font())
                    .text_size(px(11.0))
                    .text_color(color)
                    .child(self.styled_diff_text(view, cell)),
            )
    }

    /// Rows currently shown by the diff overlay's list, either layout.
    pub(super) fn diff_rows_len(&self) -> usize {
        let Some(view) = &self.diff_view else {
            return 0;
        };
        // A preview scrolls natively, so it owns no rows and shows no
        // scrollbar; reporting its blocks here would scrub the wrong list.
        if preview::showing(view) {
            return 0;
        }
        if self.settings.diff.wrap {
            return 0;
        }
        usize::from(matches!(view.content, Some(DiffContent::Text(_)))) * view.rows().len()
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
        let content = row_count_as_f32(rows) * self.diff_row_height();
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
        let content = row_count_as_f32(rows) * self.diff_row_height();
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

    fn diff_banner(&self, label: String, gap: bool) -> Div {
        div()
            .h(px(self.diff_row_height()))
            .w_full()
            .flex()
            .items_center()
            .px(px(10.0))
            .bg(if gap {
                self.theme.bg_panel
            } else {
                self.theme.bg_hover
            })
            .font_family(self.mono_font())
            .text_size(px(11.0))
            .text_color(if gap {
                self.theme.text_faint
            } else {
                self.theme.accent
            })
            .child(label)
    }

    fn outline_diff_row<R: gpui::Styled>(&self, row: R, edges: HunkEdges) -> R {
        let color = match edges.outline {
            HunkOutline::None => return row,
            HunkOutline::Idle => gpui::transparent_black(),
            HunkOutline::Selected => self.theme.active_hunk_border(),
        };
        let row = row.border_l_1().border_r_1();
        let row = if edges.top { row.border_t_1() } else { row };
        let row = if edges.bottom { row.border_b_1() } else { row };
        row.border_color(color)
    }

    /// One rendered unified row, resolved lazily from source indices.
    pub(super) fn diff_line_row(
        &self,
        view: &DiffView,
        row_index: usize,
        row: &crate::diff_split::DiffRow,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        use crate::diff_split::{CellKind, DiffRow};
        let DiffRow::Line { left, right } = row else {
            let row = match row {
                DiffRow::Hunk {
                    old_lines,
                    new_lines,
                } => self.diff_banner(hunk_label(old_lines, new_lines), false),
                DiffRow::Gap {
                    old_lines,
                    new_lines,
                } => self.diff_banner(gap_label(old_lines, new_lines), true),
                DiffRow::Marker { side } => self.diff_banner(eof_marker_label(*side), true),
                DiffRow::Line { .. } => unreachable!(),
            };
            return self
                .outline_diff_row(row, view.hunk_edges(row_index))
                .into_any_element();
        };
        let cell = right
            .as_ref()
            .or(left.as_ref())
            .expect("line rows have a side");
        let (marker, text_color, background) = match cell.kind {
            CellKind::Addition => ("+", self.theme.green, Some(self.theme.tint_added())),
            CellKind::Deletion => ("-", self.theme.red, Some(self.theme.tint_removed())),
            CellKind::Context => (" ", self.theme.text_secondary, None),
        };
        let number = |value: Option<usize>| {
            div()
                .w(px(44.0))
                .flex_none()
                .pr(px(6.0))
                .font_family(self.mono_font())
                .text_size(px(10.5))
                .text_color(self.theme.text_faint)
                .child(value.map_or_else(String::new, |value| (value + 1).to_string()))
        };
        let selected = view
            .selection
            .is_some_and(|selection| selection.contains(cell));
        let coordinate = DiffCoordinate {
            side: cell.side,
            line: cell.line,
            byte_offset: None,
        };
        let row = div()
            .id(("unified-diff-cell", cell_key(cell)))
            .h(px(self.diff_row_height()))
            .w_full()
            .flex()
            .items_center()
            .bg(if selected {
                self.theme.accent.opacity(0.35)
            } else {
                background.unwrap_or(gpui::transparent_black())
            })
            .cursor_pointer()
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                this.select_diff_line(coordinate, event.modifiers().shift, cx);
            }))
            .child(div().w(px(2.0)).flex_none().h_full().bg(text_color))
            .child(number(left.as_ref().map(|cell| cell.line)))
            .child(number(right.as_ref().map(|cell| cell.line)))
            .child(
                div()
                    .w(px(1.0))
                    .flex_none()
                    .h_full()
                    .mr(px(6.0))
                    .bg(self.theme.gutter_rule()),
            )
            .child(
                div()
                    .w(px(14.0))
                    .flex_none()
                    .font_family(self.mono_font())
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
                    .font_family(self.mono_font())
                    .text_size(px(11.0))
                    .text_color(text_color)
                    .child(self.styled_diff_text(view, cell)),
            );
        self.outline_diff_row(row, view.hunk_edges(row_index))
            .into_any_element()
    }
}

fn cell_text<'a>(diff: Option<&'a TextDiff>, cell: &crate::diff_split::DiffCell) -> &'a str {
    let side = match cell.side {
        DiffSide::Old => diff.and_then(|diff| diff.old.as_ref()),
        DiffSide::New => diff.and_then(|diff| diff.new.as_ref()),
    };
    side.and_then(|side| side.line(cell.line))
        .unwrap_or_default()
}

fn hunk_label(old_lines: &Range<usize>, new_lines: &Range<usize>) -> String {
    format!(
        "@@ -{} +{} @@",
        source_range_label(old_lines),
        source_range_label(new_lines)
    )
}

fn source_range_label(lines: &Range<usize>) -> String {
    if lines.is_empty() {
        String::from("0,0")
    } else {
        format!("{},{}", lines.start + 1, lines.len())
    }
}

fn gap_label(old_lines: &Range<usize>, new_lines: &Range<usize>) -> String {
    let hidden = old_lines.len().max(new_lines.len());
    format!("⋯ {hidden} unchanged lines")
}

fn eof_marker_label(side: DiffSide) -> String {
    let side = match side {
        DiffSide::Old => "old",
        DiffSide::New => "new",
    };
    format!("\\ No newline at end of file ({side})")
}

fn offset_index(index: usize, delta: isize, len: usize) -> usize {
    index
        .saturating_add_signed(delta)
        .min(len.saturating_sub(1))
}

fn hunk_scroll_top(
    row_index: usize,
    row_height: f32,
    row_count: usize,
    viewport_height: f32,
) -> f32 {
    let target = row_count_as_f32(row_index) * row_height;
    let maximum = (row_count_as_f32(row_count) * row_height - viewport_height).max(0.0);
    target.min(maximum)
}

fn scroll_wrapped_list_to_hunk(list: &gpui::ListState, row_index: usize) {
    list.scroll_to(gpui::ListOffset {
        item_ix: row_index,
        offset_in_item: px(0.0),
    });
}

fn cell_key(cell: &crate::diff_split::DiffCell) -> usize {
    cell_coordinate_key(DiffCoordinate {
        side: cell.side,
        line: cell.line,
        byte_offset: None,
    })
}

fn cell_coordinate_key(coordinate: DiffCoordinate) -> usize {
    coordinate.line.saturating_mul(2)
        + match coordinate.side {
            DiffSide::Old => 0,
            DiffSide::New => 1,
        }
}

fn visible_whitespace(text: &str, tab_width: usize) -> String {
    let trailing = text.len() - text.trim_end_matches(' ').len();
    let body_end = text.len() - trailing;
    let mut output = String::new();
    let mut column = 0;
    for (offset, character) in text.char_indices() {
        if offset >= body_end && character == ' ' {
            output.push('·');
            column += 1;
        } else if character == '\t' {
            output.push('→');
            let spaces = tab_width - column % tab_width;
            output.extend(std::iter::repeat_n(' ', spaces.saturating_sub(1)));
            column += spaces;
        } else {
            output.push(character);
            column += 1;
        }
    }
    output
}

fn diff_build_probe(started: std::time::Instant, view: &DiffView, wrapped: bool) {
    if std::env::var_os("SOURCEFOUR_FRAME_LOG").is_none() {
        return;
    }
    eprintln!(
        "diff-build-ms {:.3} rows={} hunks={} mode={} wrapped={wrapped}",
        started.elapsed().as_secs_f64() * 1000.0,
        view.rows().len(),
        view.hunk_rows().len(),
        view.mode.name(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_view() -> DiffView {
        DiffView::for_working_tree(
            DiffPaths::same(RepoPath(b"src/main.rs".to_vec())),
            false,
            ChangeKind::Modified,
            Arc::from([]),
            0,
            DiffMode::Unified,
        )
    }

    #[test]
    fn diff_construction_derives_its_title_from_the_origin() {
        let view = empty_view();

        assert_eq!(view.title, "src/main.rs");
        assert!((view.slider - 0.5).abs() < f32::EPSILON);
        assert!(view.content.is_none());
    }

    #[test]
    fn replacing_content_clears_derived_and_interaction_state() {
        let mut view = empty_view();
        view.current_hunk = 8;
        view.selection = Some(DiffSelection {
            anchor: DiffCoordinate {
                side: DiffSide::New,
                line: 2,
                byte_offset: None,
            },
            head: DiffCoordinate {
                side: DiffSide::New,
                line: 4,
                byte_offset: None,
            },
        });
        view.highlight_message = Some("old enrichment");
        view.unified = Some(Vec::new());
        view.split = Some(Vec::new());

        view.replace_content(DiffContent::Binary {
            message: "Binary files differ".to_owned(),
        });

        assert_eq!(view.current_hunk, 0);
        assert!(view.selection.is_none());
        assert!(view.highlight_message.is_none());
        assert!(view.unified.is_none());
        assert!(view.split.is_none());
    }

    #[test]
    fn context_changes_clear_layout_dependent_state() {
        let mut view = empty_view();
        view.current_hunk = 3;
        view.selection = Some(DiffSelection {
            anchor: DiffCoordinate {
                side: DiffSide::Old,
                line: 1,
                byte_offset: None,
            },
            head: DiffCoordinate {
                side: DiffSide::Old,
                line: 1,
                byte_offset: None,
            },
        });

        view.toggle_full_context();

        assert!(view.full_context);
        assert_eq!(view.current_hunk, 0);
        assert!(view.selection.is_none());
        assert!(view.wrap_list.is_none());
    }

    #[test]
    fn every_hunk_reserves_outline_geometry_and_only_selection_changes() {
        let mut view = empty_view();
        view.replace_content(crate::demo::Scene::Diff.content());
        let hunks = view.hunk_rows();
        assert!(hunks.len() > 1, "the demo diff must exercise navigation");
        let first_end = view.rows()[hunks[0] + 1..]
            .iter()
            .position(|row| {
                matches!(
                    row,
                    crate::diff_split::DiffRow::Hunk { .. }
                        | crate::diff_split::DiffRow::Gap { .. }
                )
            })
            .map_or(view.rows().len(), |offset| hunks[0] + 1 + offset);

        assert_eq!(
            view.hunk_edges(hunks[0]),
            HunkEdges {
                outline: HunkOutline::Selected,
                top: true,
                bottom: false,
            }
        );
        assert_ne!(view.hunk_edges(hunks[0] + 1).outline, HunkOutline::None);
        assert!(view.hunk_edges(first_end - 1).bottom);
        assert_eq!(view.hunk_edges(first_end), HunkEdges::default());
        assert_eq!(
            view.hunk_edges(hunks[1]),
            HunkEdges {
                outline: HunkOutline::Idle,
                top: true,
                bottom: false,
            }
        );

        view.current_hunk = 1;
        let first = view.hunk_edges(hunks[0]);
        assert_eq!(first.outline, HunkOutline::Idle);
        let second = view.hunk_edges(hunks[1]);
        assert!(second.top);
        assert_eq!(second.outline, HunkOutline::Selected);
    }

    #[test]
    fn whitespace_markers_expand_tabs_and_mark_only_trailing_spaces() {
        assert_eq!(visible_whitespace("\tvalue  ", 4), "→   value··");
        assert_eq!(visible_whitespace("a b", 4), "a b");
    }

    #[test]
    fn bounded_navigation_does_not_wrap() {
        assert_eq!(offset_index(0, -1, 3), 0);
        assert_eq!(offset_index(1, 1, 3), 2);
        assert_eq!(offset_index(2, 1, 3), 2);
    }

    #[test]
    fn a_fixed_height_hunk_scroll_target_places_its_header_at_the_top() {
        assert!((hunk_scroll_top(0, 18.0, 20, 180.0) - 0.0).abs() < f32::EPSILON);
        assert!((hunk_scroll_top(7, 18.0, 20, 180.0) - 126.0).abs() < f32::EPSILON);
        assert!((hunk_scroll_top(7, 24.0, 20, 180.0) - 168.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_fixed_height_hunk_scroll_target_clamps_to_natural_content_bounds() {
        assert!((hunk_scroll_top(15, 18.0, 20, 180.0) - 180.0).abs() < f32::EPSILON);
        assert!((hunk_scroll_top(2, 18.0, 5, 180.0) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_wrapped_hunk_becomes_the_logical_top_item() {
        let list = gpui::ListState::new(12, gpui::ListAlignment::Top, px(0.0), |_, _, _| {
            div().into_any_element()
        });

        scroll_wrapped_list_to_hunk(&list, 7);

        let top = list.logical_scroll_top();
        assert_eq!(top.item_ix, 7);
        assert_eq!(top.offset_in_item, px(0.0));
    }

    #[test]
    fn a_duration_grows_an_hours_field_only_when_it_has_one() {
        assert_eq!(video_duration(0), "0:00");
        assert_eq!(video_duration(8_100), "0:08");
        assert_eq!(video_duration(65_000), "1:05");
        assert_eq!(video_duration(600_000), "10:00");
        assert_eq!(video_duration(3_725_000), "1:02:05");
    }

    #[test]
    fn a_size_climbs_to_the_unit_that_reads() {
        assert_eq!(byte_size(0), "0 B");
        assert_eq!(byte_size(999), "999 B");
        // The step is 1024, so a kilobyte's worth of bytes is where it turns.
        assert_eq!(byte_size(1_023), "1023 B");
        assert_eq!(byte_size(1_024), "1.0 KB");
        assert_eq!(byte_size(4_404_019), "4.2 MB");
        // The table stops at terabytes and saturates rather than wrapping.
        // Nothing that opens in this viewer gets near it; the check is only
        // that the loop terminates instead of indexing off the end.
        assert_eq!(byte_size(u64::MAX), "16777216.0 TB");
    }

    #[test]
    fn a_summary_names_only_what_the_container_answered() {
        let full = VideoInfo {
            bytes: 4_404_019,
            duration_ms: Some(12_400),
            dimensions: Some((1920, 1080)),
            ..VideoInfo::default()
        };
        assert_eq!(video_line(full), "0:12 · 1920 × 1080 · 4.2 MB");
        // The chip is what tells a poster apart from an image diff, so the
        // marker is not decoration and the card's own line is the bare one.
        assert_eq!(video_summary(full), "▶ 0:12 · 1920 × 1080 · 4.2 MB");

        // With no decoder installed only the byte count survives, and the chip
        // has to stay a chip rather than becoming an empty pill.
        let bare = VideoInfo {
            bytes: 2_202_009,
            ..VideoInfo::default()
        };
        assert_eq!(video_line(bare), "2.1 MB");
        assert_eq!(video_summary(bare), "▶ 2.1 MB");
    }

    #[test]
    fn only_an_absent_decoder_earns_a_sentence_on_the_card() {
        let probed = VideoInfo {
            bytes: 2_202_009,
            ..VideoInfo::default()
        };
        let unprobed = VideoInfo {
            tools_missing: true,
            ..probed
        };

        // A container ffmpeg could not read, or a blob past the cap: the card
        // is just as bare, and there is nothing the reader could install.
        assert_eq!(missing_tools_note(Some(probed), Some(probed)), None);
        assert_eq!(missing_tools_note(None, None), None);
        // An added or deleted video has one side, and it still knows.
        assert_eq!(
            missing_tools_note(None, Some(unprobed)),
            Some("Install ffmpeg to see a preview frame")
        );
        assert_eq!(
            missing_tools_note(Some(unprobed), Some(unprobed)),
            Some("Install ffmpeg to see a preview frame")
        );
    }
}
