//! Text inputs with native editing behavior.
//!
//! Adapted from the pinned GPUI's `input` example, themed for Sourcefour and
//! extended with the standard macOS word commands. It implements
//! [`EntityInputHandler`], so IME composition, dictation, and the character
//! palette work exactly as they do in any Mac text field.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render,
    SharedString, Style, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine,
    actions, div, fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::Theme;

const INPUT_LINE_HEIGHT: Pixels = Pixels(18.0);

actions!(
    filter_input,
    [
        Backspace,
        Delete,
        DeleteWord,
        DeleteToStart,
        Left,
        Right,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        DocumentStart,
        DocumentEnd,
        SelectDocumentStart,
        SelectDocumentEnd,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        InsertNewline,
        Up,
        Down,
    ]
);

/// Key bindings for the input, scoped to its own context.
pub(crate) fn keymap() -> Vec<gpui::KeyBinding> {
    const CONTEXT: Option<&str> = Some("FilterInput");
    vec![
        gpui::KeyBinding::new("backspace", Backspace, CONTEXT),
        gpui::KeyBinding::new("delete", Delete, CONTEXT),
        gpui::KeyBinding::new("alt-backspace", DeleteWord, CONTEXT),
        gpui::KeyBinding::new("cmd-backspace", DeleteToStart, CONTEXT),
        gpui::KeyBinding::new("left", Left, CONTEXT),
        gpui::KeyBinding::new("right", Right, CONTEXT),
        gpui::KeyBinding::new("alt-left", WordLeft, CONTEXT),
        gpui::KeyBinding::new("alt-right", WordRight, CONTEXT),
        gpui::KeyBinding::new("alt-shift-left", SelectWordLeft, CONTEXT),
        gpui::KeyBinding::new("alt-shift-right", SelectWordRight, CONTEXT),
        gpui::KeyBinding::new("shift-left", SelectLeft, CONTEXT),
        gpui::KeyBinding::new("shift-right", SelectRight, CONTEXT),
        gpui::KeyBinding::new("cmd-a", SelectAll, CONTEXT),
        gpui::KeyBinding::new("ctrl-a", SelectAll, CONTEXT),
        gpui::KeyBinding::new("home", Home, CONTEXT),
        gpui::KeyBinding::new("cmd-left", Home, CONTEXT),
        gpui::KeyBinding::new("shift-home", SelectHome, CONTEXT),
        gpui::KeyBinding::new("cmd-shift-left", SelectHome, CONTEXT),
        gpui::KeyBinding::new("end", End, CONTEXT),
        gpui::KeyBinding::new("cmd-right", End, CONTEXT),
        gpui::KeyBinding::new("shift-end", SelectEnd, CONTEXT),
        gpui::KeyBinding::new("cmd-shift-right", SelectEnd, CONTEXT),
        gpui::KeyBinding::new("cmd-up", DocumentStart, CONTEXT),
        gpui::KeyBinding::new("cmd-down", DocumentEnd, CONTEXT),
        gpui::KeyBinding::new("cmd-shift-up", SelectDocumentStart, CONTEXT),
        gpui::KeyBinding::new("cmd-shift-down", SelectDocumentEnd, CONTEXT),
        gpui::KeyBinding::new("cmd-v", Paste, CONTEXT),
        gpui::KeyBinding::new("cmd-c", Copy, CONTEXT),
        gpui::KeyBinding::new("cmd-x", Cut, CONTEXT),
        gpui::KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, CONTEXT),
        gpui::KeyBinding::new("backspace", Backspace, Some("MultilineInput")),
        gpui::KeyBinding::new("delete", Delete, Some("MultilineInput")),
        gpui::KeyBinding::new("alt-backspace", DeleteWord, Some("MultilineInput")),
        gpui::KeyBinding::new("cmd-backspace", DeleteToStart, Some("MultilineInput")),
        gpui::KeyBinding::new("left", Left, Some("MultilineInput")),
        gpui::KeyBinding::new("right", Right, Some("MultilineInput")),
        gpui::KeyBinding::new("alt-left", WordLeft, Some("MultilineInput")),
        gpui::KeyBinding::new("alt-right", WordRight, Some("MultilineInput")),
        gpui::KeyBinding::new("alt-shift-left", SelectWordLeft, Some("MultilineInput")),
        gpui::KeyBinding::new("alt-shift-right", SelectWordRight, Some("MultilineInput")),
        gpui::KeyBinding::new("shift-left", SelectLeft, Some("MultilineInput")),
        gpui::KeyBinding::new("shift-right", SelectRight, Some("MultilineInput")),
        gpui::KeyBinding::new("cmd-a", SelectAll, Some("MultilineInput")),
        gpui::KeyBinding::new("ctrl-a", SelectAll, Some("MultilineInput")),
        gpui::KeyBinding::new("home", Home, Some("MultilineInput")),
        gpui::KeyBinding::new("cmd-left", Home, Some("MultilineInput")),
        gpui::KeyBinding::new("shift-home", SelectHome, Some("MultilineInput")),
        gpui::KeyBinding::new("cmd-shift-left", SelectHome, Some("MultilineInput")),
        gpui::KeyBinding::new("end", End, Some("MultilineInput")),
        gpui::KeyBinding::new("cmd-right", End, Some("MultilineInput")),
        gpui::KeyBinding::new("shift-end", SelectEnd, Some("MultilineInput")),
        gpui::KeyBinding::new("cmd-shift-right", SelectEnd, Some("MultilineInput")),
        gpui::KeyBinding::new("cmd-up", DocumentStart, Some("MultilineInput")),
        gpui::KeyBinding::new("cmd-down", DocumentEnd, Some("MultilineInput")),
        gpui::KeyBinding::new("cmd-shift-up", SelectDocumentStart, Some("MultilineInput")),
        gpui::KeyBinding::new("cmd-shift-down", SelectDocumentEnd, Some("MultilineInput")),
        gpui::KeyBinding::new("cmd-v", Paste, Some("MultilineInput")),
        gpui::KeyBinding::new("cmd-c", Copy, Some("MultilineInput")),
        gpui::KeyBinding::new("cmd-x", Cut, Some("MultilineInput")),
        gpui::KeyBinding::new(
            "ctrl-cmd-space",
            ShowCharacterPalette,
            Some("MultilineInput"),
        ),
        gpui::KeyBinding::new("enter", InsertNewline, Some("MultilineInput")),
        gpui::KeyBinding::new("up", Up, Some("MultilineInput")),
        gpui::KeyBinding::new("down", Down, Some("MultilineInput")),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputMode {
    SingleLine,
    Multiline { rows: usize },
}

/// A themed editable text field, single-line unless explicitly configured.
pub(crate) struct TextInput {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) content: SharedString,
    /// Draw mask characters instead of the content (token fields). The mask
    /// is one `*` per byte so every caret offset stays valid; secrets are
    /// ASCII, so bytes and characters agree.
    pub(crate) masked: bool,
    placeholder: SharedString,
    theme: Theme,
    mode: InputMode,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Vec<WrappedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    last_line_height: Pixels,
    last_scroll_y: Pixels,
    is_selecting: bool,
}

impl TextInput {
    pub(crate) fn new(
        placeholder: impl Into<SharedString>,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::default(),
            masked: false,
            placeholder: placeholder.into(),
            theme: *theme,
            mode: InputMode::SingleLine,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: Vec::new(),
            last_bounds: None,
            last_line_height: Pixels::ZERO,
            last_scroll_y: Pixels::ZERO,
            is_selecting: false,
        }
    }

    /// Makes this input a fixed-height multiline editor.
    pub(crate) fn multiline(mut self, rows: usize) -> Self {
        self.mode = InputMode::Multiline { rows: rows.max(1) };
        self
    }

    fn is_multiline(&self) -> bool {
        matches!(self.mode, InputMode::Multiline { .. })
    }

    fn rows(&self) -> usize {
        match self.mode {
            InputMode::SingleLine => 1,
            InputMode::Multiline { rows } => rows,
        }
    }

    /// Replaces the whole content, moving the caret to the end.
    pub(crate) fn set_text(&mut self, text: &str, cx: &mut gpui::Context<Self>) {
        self.content = SharedString::from(text.to_owned());
        self.selected_range = self.content.len()..self.content.len();
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    /// Alt+Backspace: delete to the start of the previous word.
    fn delete_word(&mut self, _: &DeleteWord, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_word_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    /// Cmd+Backspace: delete from the caret to the start of the line.
    fn delete_to_start(
        &mut self,
        _: &DeleteToStart,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            self.select_to(self.current_line_range().start, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.move_to(self.previous_word_boundary(self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.move_to(self.next_word_boundary(self.cursor_offset()), cx);
    }

    fn select_word_left(
        &mut self,
        _: &SelectWordLeft,
        _: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.select_to(self.previous_word_boundary(self.cursor_offset()), cx);
    }

    fn select_word_right(
        &mut self,
        _: &SelectWordRight,
        _: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.select_to(self.next_word_boundary(self.cursor_offset()), cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.move_to(self.home_offset(), cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.move_to(self.visual_row_range().end, cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.select_to(self.home_offset(), cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.select_to(self.visual_row_range().end, cx);
    }

    fn document_start(&mut self, _: &DocumentStart, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.move_to(0, cx);
    }

    fn document_end(&mut self, _: &DocumentEnd, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_document_start(
        &mut self,
        _: &SelectDocumentStart,
        _: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.select_to(0, cx);
    }

    fn select_document_end(
        &mut self,
        _: &SelectDocumentEnd,
        _: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.select_to(self.content.len(), cx);
    }

    fn insert_newline(
        &mut self,
        _: &InsertNewline,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.is_multiline() {
            self.replace_text_in_range(None, "\n", window, cx);
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.move_vertical(-1, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.move_vertical(1, cx);
    }

    #[expect(clippy::unused_self, reason = "action listeners take &mut self")]
    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut gpui::Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = if self.is_multiline() {
                text
            } else {
                text.replace('\n', " ")
            };
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut gpui::Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut gpui::Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut gpui::Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        index_for_position(
            &self.last_layout,
            position - bounds.origin + point(px(0.0), self.last_scroll_y),
            self.last_line_height,
            self.content.len(),
        )
    }

    fn select_to(&mut self, offset: usize, cx: &mut gpui::Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify();
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for character in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += character.len_utf16();
            utf8_offset += character.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for character in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += character.len_utf8();
            utf16_offset += character.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }

    fn previous_word_boundary(&self, offset: usize) -> usize {
        previous_word_boundary(&self.content, offset)
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        next_word_boundary(&self.content, offset)
    }

    fn current_line_range(&self) -> Range<usize> {
        line_range(&self.content, self.cursor_offset())
    }

    fn visual_row_range(&self) -> Range<usize> {
        if self.content.is_empty() {
            return 0..0;
        }
        visual_row_range(
            &self.last_layout,
            self.cursor_offset(),
            self.last_line_height,
        )
        .filter(|range| {
            range.end <= self.content.len()
                && self.content.is_char_boundary(range.start)
                && self.content.is_char_boundary(range.end)
        })
        .unwrap_or_else(|| self.current_line_range())
    }

    fn home_offset(&self) -> usize {
        smart_home_offset(&self.content, self.visual_row_range(), self.cursor_offset())
    }

    fn move_vertical(&mut self, direction: isize, cx: &mut gpui::Context<Self>) {
        let cursor = self.cursor_offset();
        let current = line_range(&self.content, cursor);
        let column = cursor - current.start;
        let target = if direction < 0 {
            current
                .start
                .checked_sub(1)
                .map(|offset| line_range(&self.content, offset))
        } else if current.end < self.content.len() {
            Some(line_range(&self.content, current.end + 1))
        } else {
            None
        };
        if let Some(target) = target {
            self.move_to(target.start + column.min(target.len()), cx);
        }
    }
}

fn line_range(text: &str, offset: usize) -> Range<usize> {
    let start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index);
    start..end
}

fn previous_word_boundary(text: &str, offset: usize) -> usize {
    text.unicode_word_indices()
        .rev()
        .find_map(|(index, _)| (index < offset).then_some(index))
        .unwrap_or(0)
}

fn next_word_boundary(text: &str, offset: usize) -> usize {
    text.unicode_word_indices()
        .find_map(|(index, word)| {
            let end = index + word.len();
            (end > offset).then_some(end)
        })
        .unwrap_or(text.len())
}

fn visual_row_range(
    lines: &[WrappedLine],
    index: usize,
    line_height: Pixels,
) -> Option<Range<usize>> {
    if line_height <= Pixels::ZERO {
        return None;
    }

    let mut line_start = 0;
    for line in lines {
        let line_end = line_start + line.len();
        if index <= line_end {
            let cursor = line.position_for_index(index - line_start, line_height)?;
            let row_position = cursor + point(px(0.0), line_height / 2.0);
            let row_start = line
                .closest_index_for_position(point(px(-1.0), row_position.y), line_height)
                .unwrap_or_else(|index| index);
            let row_end = line
                .closest_index_for_position(point(px(f32::MAX), row_position.y), line_height)
                .unwrap_or_else(|index| index);
            return Some(line_start + row_start..line_start + row_end);
        }
        line_start = line_end + 1;
    }
    None
}

fn smart_home_offset(text: &str, row: Range<usize>, cursor: usize) -> usize {
    let first_non_whitespace = text[row.clone()]
        .char_indices()
        .find_map(|(index, character)| (!character.is_whitespace()).then_some(row.start + index))
        .unwrap_or(row.start);
    if cursor == first_non_whitespace {
        row.start
    } else {
        first_non_whitespace
    }
}

fn position_for_index(
    lines: &[WrappedLine],
    index: usize,
    line_height: gpui::Pixels,
) -> Option<Point<gpui::Pixels>> {
    let mut origin = Point::default();
    let mut line_start = 0;
    for line in lines {
        let line_end = line_start + line.len();
        if index <= line_end {
            return line
                .position_for_index(index - line_start, line_height)
                .map(|position| origin + position);
        }
        origin.y += line.size(line_height).height;
        line_start = line_end + 1;
    }
    None
}

fn line_for_index(lines: &[WrappedLine], index: usize) -> Option<&WrappedLine> {
    let mut line_start = 0;
    for line in lines {
        let line_end = line_start + line.len();
        if index <= line_end {
            return Some(line);
        }
        line_start = line_end + 1;
    }
    None
}

fn caret_geometry(line_height: Pixels, ascent: Pixels, descent: Pixels) -> (Pixels, Pixels) {
    let height = (ascent + descent).min(line_height).max(Pixels::ZERO);
    ((line_height - height) / 2.0, height)
}

fn index_for_position(
    lines: &[WrappedLine],
    position: Point<gpui::Pixels>,
    line_height: gpui::Pixels,
    content_len: usize,
) -> usize {
    let mut origin_y = gpui::Pixels::ZERO;
    let mut line_start = 0;
    let line_height = line_height.max(px(1.0));
    for line in lines {
        let height = line.size(line_height).height;
        if position.y <= origin_y + height {
            let local = point(position.x, (position.y - origin_y).max(gpui::Pixels::ZERO));
            let index = line
                .closest_index_for_position(local, line_height)
                .unwrap_or_else(|index| index);
            return (line_start + index).min(content_len);
        }
        origin_y += height;
        line_start += line.len() + 1;
    }
    content_len
}

fn selection_quads(
    lines: &[WrappedLine],
    range: &Range<usize>,
    bounds: Bounds<gpui::Pixels>,
    line_height: gpui::Pixels,
    accent: gpui::Hsla,
) -> Vec<PaintQuad> {
    let Some(start) = position_for_index(lines, range.start, line_height) else {
        return Vec::new();
    };
    let Some(end) = position_for_index(lines, range.end, line_height) else {
        return Vec::new();
    };
    let first_top = line_height * (start.y / line_height).floor();
    let last_top = line_height * (end.y / line_height).floor();
    let mut row_top = first_top;
    let mut quads = Vec::new();
    while row_top <= last_top {
        quads.push({
            let left = if row_top == first_top {
                bounds.left() + start.x
            } else {
                bounds.left()
            };
            let right = if row_top == last_top {
                bounds.left() + end.x
            } else {
                bounds.right()
            };
            fill(
                Bounds::from_corners(
                    point(left, bounds.top() + row_top),
                    point(right, bounds.top() + row_top + line_height),
                ),
                accent.opacity(0.3),
            )
        });
        row_top += line_height;
    }
    quads
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.marked_range = Some(range.start..range.start + new_text.len());
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map_or_else(
                || range.start + new_text.len()..range.start + new_text.len(),
                |new_range| new_range.start + range.start..new_range.end + range.end,
            );
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let start = position_for_index(&self.last_layout, range.start, self.last_line_height)?;
        let end = position_for_index(&self.last_layout, range.end, self.last_line_height)?;
        Some(Bounds::from_corners(
            bounds.origin + start - point(px(0.0), self.last_scroll_y),
            bounds.origin + end + point(px(0.0), self.last_line_height - self.last_scroll_y),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line_point = bounds.localize(&point)?;
        let utf8_index = index_for_position(
            &self.last_layout,
            line_point + gpui::point(px(0.0), self.last_scroll_y),
            self.last_line_height,
            self.content.len(),
        );
        Some(self.offset_to_utf16(utf8_index))
    }
}

/// The custom element that shapes, paints, and registers the input handler.
struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    lines: Vec<WrappedLine>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    scroll_y: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (window.line_height() * self.input.read(cx).rows()).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let theme = input.theme;
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), theme.text_faint)
        } else if input.masked {
            (SharedString::from("*".repeat(content.len())), style.color)
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run.clone()
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let Ok(lines) = window.text_system().shape_text(
            display_text,
            font_size,
            &runs,
            input.is_multiline().then_some(bounds.size.width),
            None,
        ) else {
            return PrepaintState {
                lines: Vec::new(),
                cursor: None,
                selection: Vec::new(),
                scroll_y: Pixels::ZERO,
            };
        };

        let line_height = window.line_height();
        let cursor_pos = position_for_index(&lines, cursor, line_height).unwrap_or_default();
        let (cursor_offset_y, cursor_height) = line_for_index(&lines, cursor)
            .map_or((Pixels::ZERO, line_height), |line| {
                caret_geometry(line_height, line.ascent(), line.descent())
            });
        let scroll_y = (cursor_pos.y + line_height - bounds.size.height).max(Pixels::ZERO);
        let paint_bounds = Bounds::new(bounds.origin - point(px(0.0), scroll_y), bounds.size);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                Vec::new(),
                Some(fill(
                    Bounds::new(
                        paint_bounds.origin + cursor_pos + point(px(0.0), cursor_offset_y),
                        size(px(1.5), cursor_height),
                    ),
                    theme.accent,
                )),
            )
        } else {
            (
                selection_quads(
                    &lines,
                    &selected_range,
                    paint_bounds,
                    line_height,
                    theme.accent,
                ),
                None,
            )
        };
        PrepaintState {
            lines: lines.into_vec(),
            cursor,
            selection,
            scroll_y,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for selection in prepaint.selection.drain(..) {
            window.paint_quad(selection);
        }
        let mut origin = bounds.origin - point(px(0.0), prepaint.scroll_y);
        for line in &prepaint.lines {
            if line
                .paint(
                    origin,
                    window.line_height(),
                    TextAlign::Left,
                    Some(bounds),
                    window,
                    cx,
                )
                .is_err()
            {
                return;
            }
            origin.y += line.size(window.line_height()).height;
        }
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _cx| {
            input.last_layout = std::mem::take(&mut prepaint.lines);
            input.last_bounds = Some(bounds);
            input.last_line_height = window.line_height();
            input.last_scroll_y = prepaint.scroll_y;
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .min_w(px(1.0))
            .line_height(INPUT_LINE_HEIGHT)
            .key_context(if self.is_multiline() {
                "MultilineInput"
            } else {
                "FilterInput"
            })
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word))
            .on_action(cx.listener(Self::delete_to_start))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::document_start))
            .on_action(cx.listener(Self::document_end))
            .on_action(cx.listener(Self::select_document_start))
            .on_action(cx.listener(Self::select_document_end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::insert_newline))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .text_size(px(12.0))
            .child(TextElement {
                input: cx.entity().clone(),
            })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        INPUT_LINE_HEIGHT, caret_geometry, line_range, next_word_boundary, previous_word_boundary,
        smart_home_offset,
    };

    #[test]
    fn four_multiline_rows_have_a_seventy_two_pixel_viewport() {
        assert_eq!(INPUT_LINE_HEIGHT * 4, gpui::px(72.0));
    }

    #[test]
    fn a_glyph_height_caret_is_centered_inside_its_row() {
        assert_eq!(
            caret_geometry(gpui::px(18.0), gpui::px(11.0), gpui::px(3.0)),
            (gpui::px(2.0), gpui::px(14.0))
        );
    }

    #[test]
    fn caret_metrics_are_clamped_to_the_row() {
        assert_eq!(
            caret_geometry(gpui::px(18.0), gpui::px(16.0), gpui::px(6.0)),
            (gpui::px(0.0), gpui::px(18.0))
        );
    }

    #[test]
    fn line_ranges_exclude_their_newlines() {
        let text = "subject\n\nbody";
        assert_eq!(line_range(text, 3), 0..7);
        assert_eq!(line_range(text, 8), 8..8);
        assert_eq!(line_range(text, text.len()), 9..13);
    }

    #[test]
    fn a_single_line_uses_the_whole_string() {
        assert_eq!(line_range("subject", 4), 0..7);
    }

    #[test]
    fn smart_home_stops_at_indentation_then_row_start() {
        let text = "first\n    indented";
        let row = 6..text.len();
        assert_eq!(smart_home_offset(text, row.clone(), text.len()), 10);
        assert_eq!(smart_home_offset(text, row, 10), 6);
    }

    #[test]
    fn smart_home_uses_row_start_for_whitespace_only_rows() {
        assert_eq!(smart_home_offset("    ", 0..4, 3), 0);
    }

    #[test]
    fn smart_home_offsets_remain_utf8_boundaries() {
        let text = "  ærlig";
        assert_eq!(smart_home_offset(text, 0..text.len(), text.len()), 2);
    }

    #[test]
    fn word_navigation_skips_punctuation_and_whitespace() {
        let text = "one,  two! tre";
        assert_eq!(next_word_boundary(text, 3), 9);
        assert_eq!(previous_word_boundary(text, 10), 6);
    }

    #[test]
    fn word_navigation_keeps_unicode_byte_offsets_valid() {
        let text = "blåbær grøt";
        assert_eq!(next_word_boundary(text, 0), "blåbær".len());
        assert_eq!(previous_word_boundary(text, text.len()), "blåbær ".len());
    }
}
