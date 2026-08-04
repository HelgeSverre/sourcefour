//! Loaded history and the decisions the list makes about it.
//!
//! Everything here is plain data: no GPUI entity exists per commit (§15).

use sourcefour_model::{CommitRow, GitTime, GraphRow, HistoryScope, Oid};

/// Rows of the loaded tail within which the next batch is requested (§6.9).
const PREFETCH_ROWS: usize = 30;

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const MONTH: i64 = 30 * DAY;
const YEAR: i64 = 365 * DAY;

/// Incrementally loaded history for one scope.
#[derive(Debug, Default)]
pub(crate) struct HistoryState {
    pub(crate) scope: Option<HistoryScope>,
    pub(crate) rows: Vec<CommitRow>,
    pub(crate) layout: Vec<GraphRow>,
    pub(crate) has_more: bool,
    pub(crate) request_in_flight: bool,
    pub(crate) selected: Option<Oid>,
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
            self.selected = self.rows.first().map(|row| row.oid);
        }
    }

    /// Discards everything when the scope changes.
    pub(crate) fn reset(&mut self, scope: HistoryScope) {
        self.scope = Some(scope);
        self.rows.clear();
        self.layout.clear();
        self.has_more = true;
        self.request_in_flight = false;
        self.selected = None;
    }

    /// Number of rows the list should render.
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether scrolling to `visible_end` should trigger the next batch.
    ///
    /// Only one request per scope may be in flight, so a fast scroll cannot
    /// queue a pile of overlapping traversals (§6.9).
    pub(crate) fn wants_more(&self, visible_end: usize) -> bool {
        self.has_more && !self.request_in_flight && visible_end + PREFETCH_ROWS >= self.rows.len()
    }

    /// Index of the selected row, if it is loaded.
    pub(crate) fn selected_index(&self) -> Option<usize> {
        let selected = self.selected?;
        self.rows.iter().position(|row| row.oid == selected)
    }

    /// Moves the selection by `delta` rows, clamped to what is loaded.
    ///
    /// Returns the new index when the selection moved, so the caller can scroll
    /// it into view without recomputing.
    pub(crate) fn move_selection(&mut self, delta: isize) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }
        let current = self.selected_index().unwrap_or(0);
        let last = self.rows.len() - 1;
        let target = current.saturating_add_signed(delta).min(last);
        if target == current && self.selected.is_some() {
            return None;
        }
        self.selected = Some(self.rows[target].oid);
        Some(target)
    }

    /// Selects an absolute row index, if it is loaded.
    pub(crate) fn select_index(&mut self, index: usize) -> Option<usize> {
        let row = self.rows.get(index)?;
        self.selected = Some(row.oid);
        Some(index)
    }
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

    use super::{HistoryState, relative_date};

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
    fn the_first_batch_selects_the_newest_commit() {
        let state = loaded(3);

        assert_eq!(state.selected, Some(oid(0)));
        assert_eq!(state.selected_index(), Some(0));
    }

    #[test]
    fn a_later_batch_does_not_move_the_selection() {
        let mut state = loaded(3);
        state.move_selection(2);
        let chosen = state.selected;

        state.extend(vec![row(9)], vec![graph_row()], false);

        assert_eq!(state.selected, chosen, "loading more must not steal focus");
        assert_eq!(state.len(), 4);
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

        assert_eq!(state.len(), 0);
        assert_eq!(state.selected, None);
        assert!(state.has_more);
        assert!(!state.request_in_flight);
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
