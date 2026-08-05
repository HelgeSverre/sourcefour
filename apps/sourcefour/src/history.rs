//! Loaded history and the decisions the list makes about it.
//!
//! Everything here is plain data: no GPUI entity exists per commit (§15).

use std::collections::HashSet;

use sourcefour_graph::GraphState;
use sourcefour_model::{BranchSnapshot, CommitRow, GitTime, GraphRow, HistoryScope, Oid};

/// Rows of the loaded tail within which the next batch is requested (§6.9).
const PREFETCH_ROWS: usize = 30;

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const MONTH: i64 = 30 * DAY;
const YEAR: i64 = 365 * DAY;

/// What the history list points at.
///
/// The working tree is a selectable thing that is not a commit, so the
/// selection carries which kind it is; everything that needs an object ID
/// asks [`HistoryState::selected_commit`] and gets `None` for the tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Selection {
    /// The uncommitted working tree (the pinned top row, when dirty).
    ///
    /// Not yet produced outside tests: the row itself lands with the status
    /// backend. `allow` rather than `expect`, because tests construct it.
    #[allow(dead_code, reason = "produced by the working-tree row slice")]
    WorkingTree,
    /// A loaded commit.
    Commit(Oid),
}

/// Incrementally loaded history for one scope.
#[derive(Debug, Default)]
pub(crate) struct HistoryState {
    pub(crate) scope: Option<HistoryScope>,
    pub(crate) rows: Vec<CommitRow>,
    pub(crate) layout: Vec<GraphRow>,
    pub(crate) has_more: bool,
    pub(crate) request_in_flight: bool,
    pub(crate) selected: Option<Selection>,
    /// Bumped by every [`Self::reset`], so a batch requested before a scope
    /// change can be recognized as stale and dropped with its cursor. The
    /// session/generation envelope cannot catch this case: a same-window scope
    /// switch changes neither.
    pub(crate) epoch: u64,
    /// The §4.7 filter query, verbatim.
    pub(crate) filter: String,
    /// Loaded-row indices matching the filter; `None` when not filtering.
    visible: Option<Vec<usize>>,
    /// Graph recomputed over the filtered rows (§4.7), parallel to `visible`.
    filtered_layout: Vec<GraphRow>,
}

impl HistoryState {
    /// Appends a batch, keeping rows and graph rows the same length.
    ///
    /// The two vectors are indexed together by the renderer, so a mismatch
    /// would silently paint one commit's graph against another's text.
    pub(crate) fn extend(&mut self, rows: Vec<CommitRow>, layout: Vec<GraphRow>, has_more: bool) {
        // §6.9 requires batches to arrive aligned, and the cursor builds both
        // vectors in one loop. Truncating to the shorter one costs two words and
        // turns a hypothetical backend bug into missing rows instead of a panic
        // while scrolling.
        let usable = rows.len().min(layout.len());
        self.rows.extend(rows.into_iter().take(usable));
        self.layout.extend(layout.into_iter().take(usable));
        self.has_more = has_more;
        self.request_in_flight = false;
        if self.selected.is_none() {
            self.selected = self.rows.first().map(|row| Selection::Commit(row.oid));
        }
        if self.visible.is_some() {
            self.reapply_filter();
        }
    }

    /// Applies the §4.7 filter, deriving the visible view and its graph.
    pub(crate) fn set_filter(&mut self, query: &str) {
        query.clone_into(&mut self.filter);
        self.reapply_filter();
    }

    /// Recomputes the visible view; §4.7 allows this because loading is
    /// bounded while a filter is active.
    fn reapply_filter(&mut self) {
        let needle = self.filter.trim().to_lowercase();
        if needle.is_empty() {
            self.visible = None;
            self.filtered_layout.clear();
            return;
        }
        let visible: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches_filter(row, &needle))
            .map(|(index, _)| index)
            .collect();
        // A parent missing from the filtered view is dropped rather than drawn
        // toward: its commit's line simply stops at the node (§7.5).
        let present: HashSet<Oid> = visible.iter().map(|&index| self.rows[index].oid).collect();
        let mut state = GraphState::default();
        self.filtered_layout = visible
            .iter()
            .map(|&index| {
                let row = &self.rows[index];
                let parents: Vec<Oid> = row
                    .parents
                    .iter()
                    .copied()
                    .filter(|parent| present.contains(parent))
                    .collect();
                state.push(row.oid, &parents)
            })
            .collect();
        self.visible = Some(visible);
    }

    /// Whether a filter is currently narrowing the view.
    pub(crate) fn is_filtering(&self) -> bool {
        self.visible.is_some()
    }

    /// Rows the list should render right now.
    pub(crate) fn visible_len(&self) -> usize {
        self.visible.as_ref().map_or(self.rows.len(), Vec::len)
    }

    /// The commit shown at a display position.
    pub(crate) fn row_at(&self, index: usize) -> Option<&CommitRow> {
        match &self.visible {
            Some(visible) => self.rows.get(*visible.get(index)?),
            None => self.rows.get(index),
        }
    }

    /// The graph row shown at a display position.
    pub(crate) fn layout_at(&self, index: usize) -> Option<&GraphRow> {
        if self.visible.is_some() {
            self.filtered_layout.get(index)
        } else {
            self.layout.get(index)
        }
    }

    /// The selected commit, when the selection is one.
    pub(crate) fn selected_commit(&self) -> Option<Oid> {
        match self.selected {
            Some(Selection::Commit(oid)) => Some(oid),
            _ => None,
        }
    }

    /// The selection's position in the current view, if visible.
    pub(crate) fn selected_display_index(&self) -> Option<usize> {
        let selected = self.selected_commit()?;
        match &self.visible {
            Some(visible) => visible
                .iter()
                .position(|&index| self.rows[index].oid == selected),
            None => self.rows.iter().position(|row| row.oid == selected),
        }
    }

    /// Discards everything when the scope changes.
    ///
    /// The filter text survives — a scope change answers "where am I looking",
    /// not "what am I looking for" — and reapplies as batches arrive.
    pub(crate) fn reset(&mut self, scope: HistoryScope) {
        self.scope = Some(scope);
        self.rows.clear();
        self.layout.clear();
        self.has_more = true;
        self.request_in_flight = false;
        self.selected = None;
        self.epoch += 1;
        if self.visible.is_some() {
            self.reapply_filter();
        }
    }

    /// Whether scrolling to `visible_end` should trigger the next batch.
    ///
    /// Only one request per scope may be in flight, so a fast scroll cannot
    /// queue a pile of overlapping traversals (§6.9). While a filter is
    /// active nothing loads at all: §4.7 searches loaded history only.
    pub(crate) fn wants_more(&self, visible_end: usize) -> bool {
        !self.is_filtering()
            && self.has_more
            && !self.request_in_flight
            && visible_end + PREFETCH_ROWS >= self.rows.len()
    }

    /// Index of the selected row among the loaded rows, if it is loaded.
    pub(crate) fn selected_index(&self) -> Option<usize> {
        let selected = self.selected_commit()?;
        self.rows.iter().position(|row| row.oid == selected)
    }

    /// Moves the selection by `delta` positions in the current view.
    ///
    /// Returns the new display index when the selection moved, so the caller
    /// can scroll it into view without recomputing.
    pub(crate) fn move_selection(&mut self, delta: isize) -> Option<usize> {
        let len = self.visible_len();
        if len == 0 {
            return None;
        }
        let current = self.selected_display_index().unwrap_or(0);
        let target = current.saturating_add_signed(delta).min(len - 1);
        if target == current && self.selected_display_index().is_some() {
            return None;
        }
        self.selected = Some(Selection::Commit(self.row_at(target)?.oid));
        Some(target)
    }

    /// Selects a display position in the current view, if it exists.
    pub(crate) fn select_index(&mut self, index: usize) -> Option<usize> {
        let row = self.row_at(index)?;
        self.selected = Some(Selection::Commit(row.oid));
        Some(index)
    }
}

/// Whether a row matches the lowercased filter needle (§4.7): subject, author
/// name, object ID prefix, or any visible ref label.
fn matches_filter(row: &CommitRow, needle: &str) -> bool {
    row.summary.to_lowercase().contains(needle)
        || row.author_name.to_lowercase().contains(needle)
        || row.oid.to_hex().starts_with(needle)
        || row
            .labels
            .iter()
            .any(|label| label.name.to_lowercase().contains(needle))
}

/// The scope a click on `full_name` should switch to.
///
/// Clicking the already-selected ref returns to all refs, so the same control
/// both narrows and widens (§3.1).
pub(crate) fn toggled_scope(
    current: Option<&HistoryScope>,
    full_name: &str,
    tip: Oid,
) -> HistoryScope {
    match current {
        Some(HistoryScope::Ref {
            full_name: active, ..
        }) if active == full_name => HistoryScope::AllRefs,
        _ => HistoryScope::Ref {
            full_name: full_name.to_owned(),
            tip,
        },
    }
}

/// The scope a metadata refresh should restart history with.
///
/// The user's branch scope survives an external refresh, with its tip
/// re-resolved from the fresh snapshot so the restarted walk sees new commits.
/// A scope whose branch disappeared falls back to all refs (§6.12).
pub(crate) fn refreshed_scope(
    current: Option<&HistoryScope>,
    branches: &[BranchSnapshot],
) -> HistoryScope {
    match current {
        Some(HistoryScope::Ref { full_name, .. }) => branches
            .iter()
            .find(|branch| branch.full_name == *full_name)
            .map_or(HistoryScope::AllRefs, |branch| HistoryScope::Ref {
                full_name: full_name.clone(),
                tip: branch.tip,
            }),
        _ => HistoryScope::AllRefs,
    }
}

/// Whether `full_name` is the ref history is currently scoped to.
pub(crate) fn is_scoped_to(current: Option<&HistoryScope>, full_name: &str) -> bool {
    matches!(
        current,
        Some(HistoryScope::Ref { full_name: active, .. }) if active == full_name
    )
}

/// Human-readable age, matching the prototype's compact style.
///
/// `now` is passed in rather than read from the clock so the formatting is
/// testable and the demo fixture stays deterministic.
pub(crate) fn relative_date(now_seconds: i64, time: GitTime) -> String {
    let elapsed = now_seconds.saturating_sub(time.seconds_since_epoch);
    if elapsed < 0 {
        return String::from("in the future");
    }
    let (count, unit) = match elapsed {
        secs if secs < MINUTE => return String::from("just now"),
        secs if secs < HOUR => (secs / MINUTE, "minute"),
        secs if secs < DAY => (secs / HOUR, "hour"),
        secs if secs < MONTH => (secs / DAY, "day"),
        secs if secs < YEAR => (secs / MONTH, "month"),
        secs => (secs / YEAR, "year"),
    };
    if count == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{count} {unit}s ago")
    }
}

#[cfg(test)]
mod tests {
    use smallvec::SmallVec;
    use sourcefour_model::{
        CommitFlags, CommitRow, GitTime, GraphFlags, GraphRow, HistoryScope, Oid,
    };

    use super::{
        HistoryState, Selection, is_scoped_to, refreshed_scope, relative_date, toggled_scope,
    };

    #[test]
    fn clicking_a_branch_narrows_then_widens_again() {
        let tip = oid(7);

        let narrowed = toggled_scope(Some(&HistoryScope::AllRefs), "refs/heads/main", tip);
        assert_eq!(
            narrowed,
            HistoryScope::Ref {
                full_name: String::from("refs/heads/main"),
                tip
            }
        );

        // Clicking the same ref again returns to the all-refs scope.
        assert_eq!(
            toggled_scope(Some(&narrowed), "refs/heads/main", tip),
            HistoryScope::AllRefs
        );

        // Clicking a different ref switches scope rather than widening.
        assert_eq!(
            toggled_scope(Some(&narrowed), "refs/heads/side", tip),
            HistoryScope::Ref {
                full_name: String::from("refs/heads/side"),
                tip
            }
        );
    }

    #[test]
    fn a_refresh_keeps_the_branch_scope_and_follows_its_new_tip() {
        let scoped = HistoryScope::Ref {
            full_name: String::from("refs/heads/main"),
            tip: oid(1),
        };
        let branches = [branch_snapshot("refs/heads/main", oid(9))];

        assert_eq!(
            refreshed_scope(Some(&scoped), &branches),
            HistoryScope::Ref {
                full_name: String::from("refs/heads/main"),
                tip: oid(9),
            },
            "the restarted walk must see commits added since the last snapshot"
        );
    }

    #[test]
    fn a_refresh_widens_when_the_scoped_branch_disappeared() {
        let scoped = HistoryScope::Ref {
            full_name: String::from("refs/heads/gone"),
            tip: oid(1),
        };
        let branches = [branch_snapshot("refs/heads/main", oid(9))];

        assert_eq!(
            refreshed_scope(Some(&scoped), &branches),
            HistoryScope::AllRefs
        );
        assert_eq!(
            refreshed_scope(Some(&HistoryScope::AllRefs), &branches),
            HistoryScope::AllRefs
        );
        assert_eq!(refreshed_scope(None, &branches), HistoryScope::AllRefs);
    }

    fn branch_snapshot(full_name: &str, tip: Oid) -> sourcefour_model::BranchSnapshot {
        sourcefour_model::BranchSnapshot {
            full_name: full_name.to_owned(),
            short_name: full_name.rsplit('/').next().unwrap_or(full_name).to_owned(),
            tip,
            is_current: false,
            upstream: None,
            ahead_behind: sourcefour_model::AheadBehindState::Unavailable,
            checked_out_in: None,
        }
    }

    #[test]
    fn only_the_active_ref_reads_as_scoped() {
        let scope = HistoryScope::Ref {
            full_name: String::from("refs/heads/main"),
            tip: oid(1),
        };

        assert!(is_scoped_to(Some(&scope), "refs/heads/main"));
        assert!(!is_scoped_to(Some(&scope), "refs/heads/side"));
        assert!(!is_scoped_to(
            Some(&HistoryScope::AllRefs),
            "refs/heads/main"
        ));
        assert!(!is_scoped_to(None, "refs/heads/main"));
    }

    fn oid(name: u8) -> Oid {
        let mut bytes = [0_u8; 20];
        bytes[19] = name;
        Oid::sha1(bytes)
    }

    fn row(name: u8) -> CommitRow {
        CommitRow {
            oid: oid(name),
            parents: SmallVec::new(),
            summary: format!("commit {name}"),
            author_name: String::from("Fixture"),
            commit_time: GitTime {
                seconds_since_epoch: 0,
                offset_minutes: 0,
            },
            labels: SmallVec::new(),
            flags: CommitFlags {
                is_merge: false,
                is_shallow_boundary: false,
            },
        }
    }

    fn graph_row() -> GraphRow {
        GraphRow {
            node_lane: 0,
            node_color: 0,
            segments: SmallVec::new(),
            flags: GraphFlags::default(),
        }
    }

    fn loaded(count: u8) -> HistoryState {
        let mut state = HistoryState::default();
        state.extend(
            (0..count).map(row).collect(),
            (0..count).map(|_| graph_row()).collect(),
            true,
        );
        state
    }

    #[test]
    fn the_working_tree_selection_is_not_a_commit() {
        let mut state = loaded(3);
        state.selected = Some(Selection::WorkingTree);

        assert_eq!(state.selected_commit(), None);
        assert_eq!(
            state.selected_display_index(),
            None,
            "no display row exists for it yet; the row slice gives it index 0"
        );
        assert_eq!(state.selected_index(), None);
    }

    #[test]
    fn the_first_batch_selects_the_newest_commit() {
        let state = loaded(3);

        assert_eq!(state.selected, Some(Selection::Commit(oid(0))));
        assert_eq!(state.selected_commit(), Some(oid(0)));
        assert_eq!(state.selected_index(), Some(0));
    }

    #[test]
    fn a_later_batch_does_not_move_the_selection() {
        let mut state = loaded(3);
        state.move_selection(2);
        let chosen = state.selected;

        state.extend(vec![row(9)], vec![graph_row()], false);

        assert_eq!(state.selected, chosen, "loading more must not steal focus");
        assert_eq!(state.rows.len(), 4);
    }

    #[test]
    fn selection_clamps_at_both_ends() {
        let mut state = loaded(3);

        assert_eq!(state.move_selection(1), Some(1));
        assert_eq!(state.move_selection(10), Some(2), "clamped to the last row");
        assert_eq!(state.move_selection(1), None, "already at the end");
        assert_eq!(
            state.move_selection(-10),
            Some(0),
            "clamped to the first row"
        );
        assert_eq!(state.move_selection(-1), None, "already at the start");
    }

    #[test]
    fn selection_is_a_commit_not_an_index() {
        let mut state = loaded(3);
        state.move_selection(2);

        // A refresh that prepends newer commits must keep the same commit selected.
        let mut refreshed = HistoryState::default();
        refreshed.extend(
            vec![row(9), row(0), row(1), row(2)],
            (0..4).map(|_| graph_row()).collect(),
            false,
        );
        refreshed.selected = state.selected;

        assert_eq!(refreshed.selected_index(), Some(3));
    }

    #[test]
    fn more_is_requested_near_the_loaded_tail() {
        let mut state = loaded(100);

        assert!(!state.wants_more(10), "the tail is still far away");
        assert!(state.wants_more(75), "within the prefetch window");

        state.request_in_flight = true;
        assert!(
            !state.wants_more(75),
            "only one batch per scope may be in flight (§6.9)"
        );

        state.request_in_flight = false;
        state.has_more = false;
        assert!(!state.wants_more(75), "nothing left to load");
    }

    #[test]
    fn a_scope_change_discards_everything() {
        let mut state = loaded(5);
        state.move_selection(3);

        state.reset(HistoryScope::AllRefs);

        assert_eq!(state.rows.len(), 0);
        assert_eq!(state.selected, None);
        assert!(state.has_more);
        assert!(!state.request_in_flight);
    }

    #[test]
    fn a_scope_change_retires_batches_already_in_flight() {
        let mut state = loaded(5);
        let epoch_at_request = state.epoch;

        state.reset(HistoryScope::AllRefs);

        // A batch requested before the reset must be recognizable as stale, or
        // its rows (and its abandoned cursor) would leak into the new scope.
        assert_ne!(state.epoch, epoch_at_request);
    }

    #[test]
    fn a_misaligned_batch_never_desynchronizes_the_two_vectors() {
        let mut state = HistoryState::default();

        // A backend bug could deliver mismatched lengths; the renderer indexes
        // both by the same number, so the shorter one wins rather than panicking.
        state.extend(vec![row(0), row(1)], vec![graph_row()], false);

        assert_eq!(state.rows.len(), state.layout.len());
    }

    #[test]
    fn a_filter_matches_subject_author_hash_and_label_case_insensitively() {
        let mut state = loaded(4);
        state.rows[1].author_name = String::from("Grace Hopper");
        state.rows[2].labels.push(sourcefour_model::RefLabel {
            name: String::from("release/v1"),
            kind: sourcefour_model::RefKind::Tag,
            is_head: false,
            is_current: false,
        });

        state.set_filter("COMMIT 3");
        assert_eq!(state.visible_len(), 1, "subject matches ignore case");
        assert_eq!(state.row_at(0).map(|row| row.oid), Some(oid(3)));

        state.set_filter("grace");
        assert_eq!(state.visible_len(), 1, "author names match");

        state.set_filter("RELEASE/V1");
        assert_eq!(state.visible_len(), 1, "ref labels match");

        let hex = oid(2).to_hex().to_uppercase();
        state.set_filter(&hex);
        assert_eq!(state.visible_len(), 1, "object IDs match as hex prefixes");

        state.set_filter("  ");
        assert_eq!(state.visible_len(), 4, "a blank filter shows everything");
        assert!(!state.is_filtering());
    }

    #[test]
    fn a_filtered_row_with_an_absent_parent_becomes_a_root() {
        // Rows 0..3 form a chain (each parents the next); filtering to rows 0
        // and 2 removes row 1, so row 0's parent is absent from the view and
        // its line must stop at the node instead of curving to nothing (§7.5).
        let mut state = HistoryState::default();
        let mut rows: Vec<CommitRow> = (0..4).map(row).collect();
        for (index, row) in rows.iter_mut().take(3).enumerate() {
            row.parents.push(oid(u8::try_from(index).unwrap_or(0) + 1));
        }
        let layout = (0..4).map(|_| graph_row()).collect();
        state.extend(rows, layout, false);

        state.set_filter("commit 0");
        assert!(
            state.layout_at(0).is_some_and(|graph| graph.flags.is_root),
            "the absent parent is dropped rather than drawn toward"
        );

        state.set_filter("commit");
        assert_eq!(state.visible_len(), 4);
        assert!(
            state.layout_at(0).is_some_and(|graph| !graph.flags.is_root),
            "with every row matching, the chain connects again"
        );
    }

    #[test]
    fn selection_moves_through_the_filtered_view_only() {
        let mut state = loaded(6);
        state.rows[1].summary = String::from("special one");
        state.rows[4].summary = String::from("special two");
        state.set_filter("special");

        state.selected = Some(Selection::Commit(oid(1)));
        assert_eq!(state.selected_display_index(), Some(0));

        assert_eq!(state.move_selection(1), Some(1), "next match, not next row");
        assert_eq!(state.selected_commit(), Some(oid(4)));
        assert_eq!(state.move_selection(1), None, "clamped at the last match");
    }

    #[test]
    fn filtering_freezes_batch_loading_and_extend_keeps_matches_fresh() {
        let mut state = loaded(100);
        state.set_filter("commit 1");

        assert!(
            !state.wants_more(usize::MAX),
            "§4.7: the filter searches loaded history only"
        );

        let matches_before = state.visible_len();
        state.extend(vec![row(101)], vec![graph_row()], true);
        assert_eq!(
            state.visible_len(),
            matches_before + 1,
            "a batch that arrives while filtered joins the matches"
        );

        state.set_filter("");
        assert!(state.wants_more(95), "clearing the filter resumes loading");
    }

    #[test]
    fn relative_dates_read_the_way_the_prototype_writes_them() {
        let at = |seconds| GitTime {
            seconds_since_epoch: seconds,
            offset_minutes: 0,
        };

        assert_eq!(relative_date(100, at(100)), "just now");
        assert_eq!(relative_date(3_600, at(0)), "1 hour ago");
        assert_eq!(relative_date(7_200, at(0)), "2 hours ago");
        assert_eq!(relative_date(86_400, at(0)), "1 day ago");
        assert_eq!(relative_date(86_400 * 45, at(0)), "1 month ago");
        assert_eq!(relative_date(86_400 * 400, at(0)), "1 year ago");
        assert_eq!(
            relative_date(0, at(500)),
            "in the future",
            "clock skew must not produce a nonsense duration"
        );
    }
}
