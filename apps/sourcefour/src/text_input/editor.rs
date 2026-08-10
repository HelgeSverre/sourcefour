//! Logical editing state shared by every Sourcefour text input.

use std::{
    ops::Range,
    time::{Duration, Instant},
};

use gpui::SharedString;

const UNDO_LIMIT: usize = 100;
const MERGE_WINDOW: Duration = Duration::from_millis(300);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    text: SharedString,
    anchor: usize,
    head: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditKind {
    Typing,
    Backspace,
    Delete,
    Paste,
    Cut,
    Newline,
    Composition,
}

impl EditKind {
    fn merges(self) -> bool {
        matches!(self, Self::Typing | Self::Backspace | Self::Delete)
    }
}

#[derive(Clone, Debug)]
struct HistoryEntry {
    before: Snapshot,
    after: Snapshot,
    kind: EditKind,
    at: Instant,
}

#[derive(Debug)]
pub(super) struct EditorState {
    text: SharedString,
    anchor: usize,
    head: usize,
    marked: Option<Range<usize>>,
    composition_before: Option<Snapshot>,
    revision: u64,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    group_broken: bool,
    preferred_x: Option<f32>,
}

impl EditorState {
    pub(super) fn new() -> Self {
        Self {
            text: SharedString::default(),
            anchor: 0,
            head: 0,
            marked: None,
            composition_before: None,
            revision: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            group_broken: true,
            preferred_x: None,
        }
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }
    pub(super) fn shared_text(&self) -> SharedString {
        self.text.clone()
    }
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }
    pub(super) fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }
    pub(super) fn head(&self) -> usize {
        self.head
    }
    pub(super) fn is_reversed(&self) -> bool {
        self.head < self.anchor
    }
    pub(super) fn selection(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
    pub(super) fn selection_is_empty(&self) -> bool {
        self.anchor == self.head
    }
    pub(super) fn preferred_x(&self) -> Option<f32> {
        self.preferred_x
    }
    pub(super) fn set_preferred_x(&mut self, x: Option<f32>) {
        self.preferred_x = x;
    }

    pub(super) fn set_text(&mut self, text: &str) {
        self.text = SharedString::from(text.to_owned());
        self.anchor = self.text.len();
        self.head = self.anchor;
        self.marked = None;
        self.composition_before = None;
        self.undo.clear();
        self.redo.clear();
        self.changed();
    }

    pub(super) fn collapse(&mut self, offset: usize) {
        self.anchor = offset;
        self.head = offset;
        self.break_group();
    }

    pub(super) fn extend(&mut self, offset: usize) {
        self.head = offset;
        self.break_group();
    }

    pub(super) fn break_group(&mut self) {
        self.group_broken = true;
        self.preferred_x = None;
    }

    pub(super) fn replace(&mut self, range: Range<usize>, text: &str, kind: EditKind) {
        self.replace_at(range, text, kind, Instant::now());
    }

    fn replace_at(&mut self, range: Range<usize>, text: &str, kind: EditKind, at: Instant) {
        let before = self.snapshot();
        self.replace_without_history(range.clone(), text);
        let after = self.snapshot();
        self.record(before, after, kind, at);
    }

    pub(super) fn replace_marked(
        &mut self,
        range: Range<usize>,
        text: &str,
        selection: Range<usize>,
    ) {
        if self.composition_before.is_none() {
            self.composition_before = Some(self.snapshot());
        }
        self.replace_without_history(range.clone(), text);
        self.marked = Some(range.start..range.start + text.len());
        self.anchor = range.start + selection.start;
        self.head = range.start + selection.end;
    }

    pub(super) fn unmark(&mut self) {
        self.marked = None;
        if let Some(before) = self.composition_before.take() {
            let after = self.snapshot();
            self.record(before, after, EditKind::Composition, Instant::now());
        }
    }

    pub(super) fn undo(&mut self) -> bool {
        self.unmark();
        let Some(entry) = self.undo.pop() else {
            return false;
        };
        self.restore(&entry.before);
        self.redo.push(entry);
        self.group_broken = true;
        true
    }

    pub(super) fn redo(&mut self) -> bool {
        let Some(entry) = self.redo.pop() else {
            return false;
        };
        self.restore(&entry.after);
        self.undo.push(entry);
        self.group_broken = true;
        true
    }

    fn replace_without_history(&mut self, range: Range<usize>, text: &str) {
        self.text = (self.text[..range.start].to_owned() + text + &self.text[range.end..]).into();
        let cursor = range.start + text.len();
        self.anchor = cursor;
        self.head = cursor;
        self.marked = None;
        self.changed();
    }

    fn record(&mut self, before: Snapshot, after: Snapshot, kind: EditKind, at: Instant) {
        let merge = !self.group_broken
            && kind.merges()
            && self.undo.last().is_some_and(|last| {
                last.kind == kind && at.saturating_duration_since(last.at) <= MERGE_WINDOW
            });
        if merge {
            if let Some(last) = self.undo.last_mut() {
                last.after = after;
                last.at = at;
            }
        } else {
            self.undo.push(HistoryEntry {
                before,
                after,
                kind,
                at,
            });
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.group_broken = !kind.merges();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            anchor: self.anchor,
            head: self.head,
        }
    }

    fn restore(&mut self, snapshot: &Snapshot) {
        self.text = snapshot.text.clone();
        self.anchor = snapshot.anchor;
        self.head = snapshot.head;
        self.marked = None;
        self.composition_before = None;
        self.changed();
    }

    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{EditKind, EditorState};

    #[test]
    fn reversed_selections_keep_anchor_and_head() {
        let mut state = EditorState::new();
        state.set_text("abcdef");
        state.collapse(5);
        state.extend(2);
        assert_eq!(state.selection(), 2..5);
        assert!(state.is_reversed());
    }

    #[test]
    fn compatible_typing_is_one_undo_step() {
        let mut state = EditorState::new();
        state.replace(0..0, "a", EditKind::Typing);
        state.replace(1..1, "b", EditKind::Typing);
        assert!(state.undo());
        assert_eq!(state.text(), "");
    }

    #[test]
    fn movement_breaks_an_undo_group() {
        let mut state = EditorState::new();
        state.replace(0..0, "a", EditKind::Typing);
        state.collapse(1);
        state.replace(1..1, "b", EditKind::Typing);
        assert!(state.undo());
        assert_eq!(state.text(), "a");
    }

    #[test]
    fn a_typing_pause_breaks_an_undo_group() {
        let mut state = EditorState::new();
        let now = Instant::now();
        state.replace_at(0..0, "a", EditKind::Typing, now);
        state.replace_at(
            1..1,
            "b",
            EditKind::Typing,
            now + Duration::from_millis(301),
        );
        assert!(state.undo());
        assert_eq!(state.text(), "a");
    }

    #[test]
    fn redo_restores_text_and_selection() {
        let mut state = EditorState::new();
        state.replace(0..0, "word", EditKind::Paste);
        assert!(state.undo());
        assert!(state.redo());
        assert_eq!(state.text(), "word");
        assert_eq!(state.selection(), 4..4);
    }

    #[test]
    fn a_new_edit_discards_redo_history() {
        let mut state = EditorState::new();
        state.replace(0..0, "a", EditKind::Paste);
        assert!(state.undo());
        state.replace(0..0, "b", EditKind::Paste);
        assert!(!state.redo());
    }

    #[test]
    fn history_retains_only_the_newest_hundred_transactions() {
        let mut state = EditorState::new();
        for _ in 0..101 {
            let end = state.text().len();
            state.replace(end..end, "x", EditKind::Paste);
        }
        for _ in 0..100 {
            assert!(state.undo());
        }
        assert_eq!(state.text(), "x");
        assert!(!state.undo());
    }

    #[test]
    fn composition_is_one_undo_step() {
        let mut state = EditorState::new();
        state.replace_marked(0..0, "a", 1..1);
        state.replace_marked(0..1, "æ", 2..2);
        state.unmark();
        assert!(state.undo());
        assert_eq!(state.text(), "");
    }
}
