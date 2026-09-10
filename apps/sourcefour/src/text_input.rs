//! Text inputs with native editing behavior.
//!
//! Adapted from the pinned GPUI's `input` example, themed for Sourcefour and
//! extended with the standard macOS word commands. It implements
//! [`EntityInputHandler`], so IME composition, dictation, and the character
//! palette work exactly as they do in any Mac text field.

use std::ops::Range;

use gpui::{
    App, Bounds, CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Focusable, GlobalElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render, SharedString, Style, TextAlign,
    TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill, point,
    prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::Theme;

mod editor;
use editor::{EditKind, EditorState};

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
        Undo,
        Redo,
    ]
);

/// Key bindings for the input, scoped to its own context.
pub(crate) fn keymap() -> Vec<gpui::KeyBinding> {
    let mut bindings = editing_keymap(Some("TextInput"));
    bindings.extend([
        gpui::KeyBinding::new(
            "enter",
            InsertNewline,
            Some("TextInput && mode == multiline"),
        ),
        gpui::KeyBinding::new("up", Up, Some("TextInput && mode == multiline")),
        gpui::KeyBinding::new("down", Down, Some("TextInput && mode == multiline")),
    ]);
    bindings
}

fn editing_keymap(context: Option<&'static str>) -> Vec<gpui::KeyBinding> {
    vec![
        gpui::KeyBinding::new("backspace", Backspace, context),
        gpui::KeyBinding::new("delete", Delete, context),
        gpui::KeyBinding::new("alt-backspace", DeleteWord, context),
        gpui::KeyBinding::new("cmd-backspace", DeleteToStart, context),
        gpui::KeyBinding::new("left", Left, context),
        gpui::KeyBinding::new("right", Right, context),
        gpui::KeyBinding::new("alt-left", WordLeft, context),
        gpui::KeyBinding::new("alt-right", WordRight, context),
        gpui::KeyBinding::new("alt-shift-left", SelectWordLeft, context),
        gpui::KeyBinding::new("alt-shift-right", SelectWordRight, context),
        gpui::KeyBinding::new("shift-left", SelectLeft, context),
        gpui::KeyBinding::new("shift-right", SelectRight, context),
        gpui::KeyBinding::new("cmd-a", SelectAll, context),
        gpui::KeyBinding::new("ctrl-a", SelectAll, context),
        gpui::KeyBinding::new("home", Home, context),
        gpui::KeyBinding::new("cmd-left", Home, context),
        gpui::KeyBinding::new("shift-home", SelectHome, context),
        gpui::KeyBinding::new("cmd-shift-left", SelectHome, context),
        gpui::KeyBinding::new("end", End, context),
        gpui::KeyBinding::new("cmd-right", End, context),
        gpui::KeyBinding::new("shift-end", SelectEnd, context),
        gpui::KeyBinding::new("cmd-shift-right", SelectEnd, context),
        gpui::KeyBinding::new("cmd-up", DocumentStart, context),
        gpui::KeyBinding::new("cmd-down", DocumentEnd, context),
        gpui::KeyBinding::new("cmd-shift-up", SelectDocumentStart, context),
        gpui::KeyBinding::new("cmd-shift-down", SelectDocumentEnd, context),
        gpui::KeyBinding::new("cmd-v", Paste, context),
        gpui::KeyBinding::new("cmd-c", Copy, context),
        gpui::KeyBinding::new("cmd-x", Cut, context),
        gpui::KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, context),
        gpui::KeyBinding::new("cmd-z", Undo, context),
        gpui::KeyBinding::new("cmd-shift-z", Redo, context),
        gpui::KeyBinding::new("cmd-y", Redo, context),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputMode {
    SingleLine,
    Multiline { rows: usize },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum InputRole {
    #[default]
    Plain,
    HistoryFilter,
    DiffSwitcher,
    BranchName,
    GithubToken,
    CommitMessage,
    RepoPath,
}

#[derive(Default)]
struct LayoutSnapshot {
    lines: Vec<WrappedLine>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
    scroll_y: Pixels,
    revision: u64,
}

/// A themed editable text field, single-line unless explicitly configured.
pub(crate) struct TextInput {
    menus: Option<gpui::WeakEntity<crate::context_menu::MenuHost>>,
    pub(crate) focus_handle: FocusHandle,
    /// Draw mask characters instead of the content (token fields). The mask
    /// is one `*` per byte so every caret offset stays valid; secrets are
    /// ASCII, so bytes and characters agree.
    masked: bool,
    editor: EditorState,
    next_edit_kind: EditKind,
    placeholder: SharedString,
    theme: Theme,
    mode: InputMode,
    role: InputRole,
    layout: LayoutSnapshot,
    is_selecting: bool,
}

impl TextInput {
    pub(crate) fn new(
        placeholder: impl Into<SharedString>,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        Self {
            menus: None,
            focus_handle: cx.focus_handle(),
            masked: false,
            editor: EditorState::new(),
            next_edit_kind: EditKind::Typing,
            placeholder: placeholder.into(),
            theme: *theme,
            mode: InputMode::SingleLine,
            role: InputRole::Plain,
            layout: LayoutSnapshot::default(),
            is_selecting: false,
        }
    }

    /// Makes this input a fixed-height multiline editor.
    pub(crate) fn multiline(mut self, rows: usize) -> Self {
        self.mode = InputMode::Multiline { rows: rows.max(1) };
        self
    }

    pub(crate) fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    pub(crate) fn role(mut self, role: InputRole) -> Self {
        self.role = role;
        self
    }

    pub(crate) fn set_menu_host(&mut self, host: gpui::WeakEntity<crate::context_menu::MenuHost>) {
        self.menus = Some(host);
    }

    fn context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        use crate::context_menu::{MenuEntry, show};
        let Some(host) = self.menus.clone() else {
            return;
        };
        self.is_selecting = false;
        self.focus_handle.focus(window);
        let selected = !self.editor.selection_is_empty();
        let entry =
            |label, enabled, operation: fn(&mut Self, &mut Window, &mut gpui::Context<Self>)| {
                if enabled {
                    MenuEntry::command(label, cx, operation)
                } else {
                    MenuEntry::disabled(label)
                }
            };
        let entries = vec![
            entry("Undo", self.editor.can_undo(), |this, window, cx| {
                this.undo(&Undo, window, cx);
            }),
            entry("Redo", self.editor.can_redo(), |this, window, cx| {
                this.redo(&Redo, window, cx);
            }),
            MenuEntry::Separator,
            entry("Cut", selected, |this, window, cx| {
                this.cut(&Cut, window, cx);
            }),
            entry("Copy", selected, |this, window, cx| {
                this.copy(&Copy, window, cx);
            }),
            entry("Paste", true, |this, window, cx| {
                this.paste(&Paste, window, cx);
            }),
            MenuEntry::Separator,
            entry(
                "Select all",
                !self.editor.text().is_empty(),
                |this, window, cx| this.select_all(&SelectAll, window, cx),
            ),
        ];
        show(&host, position, entries, window, cx);
    }

    fn context_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let index = self.index_for_mouse_position(event.position);
        let selection = self.editor.selection();
        if selection.is_empty() || !selection.contains(&index) {
            self.move_to(index, cx);
        }
        self.context_menu(event.position, window, cx);
    }

    fn key_context(&self) -> &'static str {
        match (self.mode, self.role) {
            (InputMode::Multiline { .. }, InputRole::CommitMessage) => {
                "TextInput mode = multiline role = commit_message"
            }
            (InputMode::Multiline { .. }, _) => "TextInput mode = multiline",
            (_, InputRole::HistoryFilter) => "TextInput mode = singleline role = history_filter",
            (_, InputRole::DiffSwitcher) => "TextInput mode = singleline role = diff_switcher",
            (_, InputRole::BranchName) => "TextInput mode = singleline role = branch_name",
            (_, InputRole::GithubToken) => "TextInput mode = singleline role = github_token",
            (_, InputRole::RepoPath) => "TextInput mode = singleline role = repo_path",
            _ => "TextInput mode = singleline",
        }
    }

    pub(crate) fn text(&self) -> &str {
        self.editor.text()
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
        if let Some(host) = &self.menus {
            host.update(cx, |host, cx| {
                host.invalidate_for_focus(&self.focus_handle, cx);
            })
            .ok();
        }
        self.editor.set_text(text);
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut gpui::Context<Self>) {
        let range = if self.editor.selection_is_empty() {
            self.previous_boundary(self.cursor_offset())..self.cursor_offset()
        } else {
            self.editor.selection()
        };
        self.editor.replace(range, "", EditKind::Backspace);
        cx.notify();
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut gpui::Context<Self>) {
        let range = if self.editor.selection_is_empty() {
            self.cursor_offset()..self.next_boundary(self.cursor_offset())
        } else {
            self.editor.selection()
        };
        self.editor.replace(range, "", EditKind::Delete);
        cx.notify();
    }

    /// Alt+Backspace: delete to the start of the previous word.
    fn delete_word(&mut self, _: &DeleteWord, _: &mut Window, cx: &mut gpui::Context<Self>) {
        let range = if self.editor.selection_is_empty() {
            self.previous_word_boundary(self.cursor_offset())..self.cursor_offset()
        } else {
            self.editor.selection()
        };
        self.editor.replace(range, "", EditKind::Backspace);
        cx.notify();
    }

    /// Cmd+Backspace: delete from the caret to the start of the line.
    fn delete_to_start(&mut self, _: &DeleteToStart, _: &mut Window, cx: &mut gpui::Context<Self>) {
        let range = if self.editor.selection_is_empty() {
            self.current_line_range().start..self.cursor_offset()
        } else {
            self.editor.selection()
        };
        self.editor.replace(range, "", EditKind::Backspace);
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.editor.selection_is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.editor.selection().start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.editor.selection_is_empty() {
            self.move_to(self.next_boundary(self.editor.selection().end), cx);
        } else {
            self.move_to(self.editor.selection().end, cx);
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
        self.select_to(self.editor.text().len(), cx);
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
        self.move_to(self.editor.text().len(), cx);
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
        self.select_to(self.editor.text().len(), cx);
    }

    fn insert_newline(
        &mut self,
        _: &InsertNewline,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.is_multiline() {
            self.next_edit_kind = EditKind::Newline;
            self.replace_text_in_range(None, "\n", window, cx);
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.move_vertical(-1, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.move_vertical(1, cx);
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.editor.undo() {
            cx.notify();
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.editor.redo() {
            cx.notify();
        }
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
            self.next_edit_kind = EditKind::Paste;
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut gpui::Context<Self>) {
        let selection = self.editor.selection();
        if !selection.is_empty() {
            crate::context_menu::copy_text(self.editor.text()[selection].to_string(), cx);
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let selection = self.editor.selection();
        if !selection.is_empty() {
            crate::context_menu::copy_text(self.editor.text()[selection].to_string(), cx);
            self.next_edit_kind = EditKind::Cut;
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if crate::context_menu::is_context_click(event) {
            return;
        }
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
        self.editor.collapse(offset);
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        self.editor.head()
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.editor.text().is_empty() {
            return 0;
        }
        let Some(layout) = self.valid_layout() else {
            return 0;
        };
        let bounds = &layout.bounds;
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.editor.text().len();
        }
        index_for_position(
            &layout.lines,
            position - bounds.origin + point(px(0.0), layout.scroll_y),
            layout.line_height,
            self.editor.text().len(),
        )
    }

    fn select_to(&mut self, offset: usize, cx: &mut gpui::Context<Self>) {
        self.editor.extend(offset);
        cx.notify();
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for character in self.editor.text().chars() {
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
        for character in self.editor.text().chars() {
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
        self.editor
            .text()
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.editor
            .text()
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.editor.text().len())
    }

    fn previous_word_boundary(&self, offset: usize) -> usize {
        previous_word_boundary(self.editor.text(), offset)
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        next_word_boundary(self.editor.text(), offset)
    }

    fn current_line_range(&self) -> Range<usize> {
        line_range(self.editor.text(), self.cursor_offset())
    }

    fn visual_row_range(&self) -> Range<usize> {
        if self.editor.text().is_empty() {
            return 0..0;
        }
        self.valid_layout()
            .and_then(|layout| {
                visual_row_range(&layout.lines, self.cursor_offset(), layout.line_height)
            })
            .filter(|range| {
                range.end <= self.editor.text().len()
                    && self.editor.text().is_char_boundary(range.start)
                    && self.editor.text().is_char_boundary(range.end)
            })
            .unwrap_or_else(|| self.current_line_range())
    }

    fn valid_layout(&self) -> Option<&LayoutSnapshot> {
        (self.layout.revision == self.editor.revision()).then_some(&self.layout)
    }

    fn home_offset(&self) -> usize {
        smart_home_offset(
            self.editor.text(),
            self.visual_row_range(),
            self.cursor_offset(),
        )
    }

    fn move_vertical(&mut self, direction: isize, cx: &mut gpui::Context<Self>) {
        let cursor = self.cursor_offset();
        if let Some(layout) = self.valid_layout()
            && let Some(position) = position_for_index(&layout.lines, cursor, layout.line_height)
        {
            let x = self.editor.preferred_x().unwrap_or(position.x.0);
            let target_y = if direction < 0 {
                position.y - layout.line_height
            } else {
                position.y + layout.line_height
            };
            let target = point(px(x), target_y);
            let offset = index_for_position(
                &layout.lines,
                target,
                layout.line_height,
                self.editor.text().len(),
            );
            self.editor.collapse(offset);
            self.editor.set_preferred_x(Some(x));
            cx.notify();
            return;
        }
        let current = line_range(self.editor.text(), cursor);
        let column = cursor - current.start;
        let target = if direction < 0 {
            current
                .start
                .checked_sub(1)
                .map(|offset| line_range(self.editor.text(), offset))
        } else if current.end < self.editor.text().len() {
            Some(line_range(self.editor.text(), current.end + 1))
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

fn range_from_utf16_in(text: &str, range: &Range<usize>) -> Range<usize> {
    fn offset(text: &str, target: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for character in text.chars() {
            if utf16 >= target {
                break;
            }
            utf16 += character.len_utf16();
            utf8 += character.len_utf8();
        }
        utf8
    }
    offset(text, range.start)..offset(text, range.end)
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
        if position.y < origin_y + height {
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
        Some(self.editor.text()[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.editor.selection()),
            reversed: self.editor.is_reversed(),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<Range<usize>> {
        self.editor
            .marked()
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) {
        self.editor.unmark();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let cancels_composition =
            range_utf16.is_some() && new_text.is_empty() && self.editor.is_composing();
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.editor.marked())
            .unwrap_or_else(|| self.editor.selection());
        let kind = std::mem::replace(&mut self.next_edit_kind, EditKind::Typing);
        if cancels_composition {
            self.editor.cancel_composition();
        } else {
            self.editor.commit_composition(range, new_text, kind);
        }
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
            .or(self.editor.marked())
            .unwrap_or_else(|| self.editor.selection());
        let selection = new_selected_range_utf16.as_ref().map_or_else(
            || new_text.len()..new_text.len(),
            |range_utf16| range_from_utf16_in(new_text, range_utf16),
        );
        self.editor.replace_marked(range, new_text, selection);
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
        let layout = self.valid_layout()?;
        let start = position_for_index(&layout.lines, range.start, layout.line_height)?;
        let end = position_for_index(&layout.lines, range.end, layout.line_height)?;
        Some(Bounds::from_corners(
            bounds.origin + start - point(px(0.0), layout.scroll_y),
            bounds.origin + end + point(px(0.0), layout.line_height - layout.scroll_y),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<usize> {
        let layout = self.valid_layout()?;
        let bounds = layout.bounds;
        let line_point = bounds.localize(&point)?;
        let utf8_index = index_for_position(
            &layout.lines,
            line_point + gpui::point(px(0.0), layout.scroll_y),
            layout.line_height,
            self.editor.text().len(),
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

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
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
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.editor.shared_text();
        let theme = input.theme;
        let selected_range = input.editor.selection();
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
        let runs = if let Some(marked_range) = input.editor.marked().as_ref() {
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
        _inspector_id: Option<&gpui::InspectorElementId>,
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
            input.layout = LayoutSnapshot {
                lines: std::mem::take(&mut prepaint.lines),
                bounds,
                line_height: window.line_height(),
                scroll_y: prepaint.scroll_y,
                revision: input.editor.revision(),
            };
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        use crate::context_menu::ContextMenuExt as _;
        div()
            .flex()
            .flex_1()
            .min_w(px(1.0))
            .line_height(INPUT_LINE_HEIGHT)
            .key_context(self.key_context())
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
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_context_menu(cx.listener(Self::context_mouse_down))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "f10" && event.keystroke.modifiers.shift {
                    cx.stop_propagation();
                    this.context_menu(this.layout.bounds.origin, window, cx);
                }
            }))
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
    use gpui::{
        AppContext, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext, div, px,
    };

    use super::{
        Down, INPUT_LINE_HEIGHT, TextInput, Up, caret_geometry, line_range, next_word_boundary,
        previous_word_boundary, range_from_utf16_in, smart_home_offset,
    };
    use crate::theme::Theme;

    fn draw_input(cx: &mut gpui::VisualTestContext) {
        let fixture = cx.update(|window, _| window.root::<TextAreaFixture>().unwrap().unwrap());
        cx.draw(
            gpui::Point::default(),
            gpui::size(px(500.0), px(300.0)),
            |_, _| fixture.into_any_element(),
        );
    }

    #[gpui::test]
    fn context_cut_preserves_selection_and_undo_targets_the_input(cx: &mut TestAppContext) {
        let (input, cx) = text_area("alpha beta", 0, cx);
        input.update(cx, |input, cx| {
            input.editor.extend(5);
            cx.notify();
        });
        draw_input(cx);
        let position =
            cx.update(|_, cx| input.read(cx).layout.bounds.origin + gpui::point(px(5.0), px(8.0)));
        cx.simulate_mouse_down(
            position,
            gpui::MouseButton::Right,
            gpui::Modifiers::default(),
        );
        draw_input(cx);
        assert_eq!(cx.update(|_, cx| input.read(cx).editor.selection()), 0..5);
        assert!(!cx.update(|_, cx| input.read(cx).is_selecting));
        // Undo and Redo are disabled initially, so Cut is the first entry.
        cx.simulate_keystrokes("enter");
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("alpha")
        );
        assert_eq!(cx.update(|_, cx| input.read(cx).text().to_owned()), " beta");
        draw_input(cx);
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(
            cx.update(|_, cx| input.read(cx).text().to_owned()),
            "alpha beta"
        );
        cx.update(|window, cx| assert!(input.read(cx).focus_handle.is_focused(window)));
    }

    #[cfg(target_os = "macos")]
    #[gpui::test]
    fn input_control_click_outside_selection_moves_caret_without_dragging(cx: &mut TestAppContext) {
        let (input, cx) = text_area("alpha beta", 0, cx);
        input.update(cx, |input, cx| {
            input.editor.extend(5);
            cx.notify();
        });
        draw_input(cx);
        let position = cx
            .update(|_, cx| input.read(cx).layout.bounds.origin + gpui::point(px(200.0), px(8.0)));
        cx.simulate_click(
            position,
            gpui::Modifiers {
                control: true,
                ..gpui::Modifiers::default()
            },
        );
        draw_input(cx);
        assert_eq!(cx.update(|_, cx| input.read(cx).editor.selection()), 10..10);
        assert!(!cx.update(|_, cx| input.read(cx).is_selecting));
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| assert!(input.read(cx).focus_handle.is_focused(window)));
    }

    struct TextAreaFixture {
        input: Entity<TextInput>,
        menus: Entity<crate::context_menu::MenuHost>,
    }

    impl Render for TextAreaFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .w(px(500.0))
                .child(self.input.clone())
                .child(self.menus.clone())
        }
    }

    fn text_area<'a>(
        text: &'static str,
        cursor: usize,
        cx: &'a mut TestAppContext,
    ) -> (Entity<TextInput>, &'a mut gpui::VisualTestContext) {
        cx.update(|cx| cx.bind_keys(super::keymap()));
        let (fixture, cx) = cx.add_window_view(|window, cx| {
            let menus = cx.new(|cx| crate::context_menu::MenuHost::new(window, cx));
            let input = cx.new(|cx| {
                let mut input = TextInput::new("", &Theme::dark(), cx).multiline(4);
                input.editor.set_text(text);
                input.editor.collapse(cursor);
                input
            });
            input.update(cx, |input, _| input.set_menu_host(menus.downgrade()));
            window.focus(&input.read(cx).focus_handle);
            TextAreaFixture { input, menus }
        });
        let input = cx.update(|_, cx| fixture.read(cx).input.clone());
        (input, cx)
    }

    #[gpui::test]
    fn down_moves_to_the_same_column_on_the_next_logical_line(cx: &mut TestAppContext) {
        let (input, cx) = text_area("abcd\nabcd", 2, cx);

        cx.dispatch_action(Down);

        assert_eq!(cx.update(|_, cx| input.read(cx).cursor_offset()), 7);
    }

    #[gpui::test]
    fn down_can_enter_an_empty_logical_line(cx: &mut TestAppContext) {
        let (input, cx) = text_area("abcd\n\nefgh", 2, cx);

        cx.dispatch_action(Down);

        assert_eq!(cx.update(|_, cx| input.read(cx).cursor_offset()), 5);
    }

    #[gpui::test]
    fn up_moves_to_the_same_column_on_the_previous_logical_line(cx: &mut TestAppContext) {
        let (input, cx) = text_area("abcd\nabcd\nabcd", 12, cx);

        cx.dispatch_action(Up);

        assert_eq!(cx.update(|_, cx| input.read(cx).cursor_offset()), 7);
    }

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

    #[test]
    fn ime_selection_ranges_are_relative_to_the_composed_text() {
        assert_eq!(range_from_utf16_in("a😀b", &(1..3)), 1..5);
    }
}
