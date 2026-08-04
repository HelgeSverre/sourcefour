//! Pure semantic graph-layout state.
//!
//! GPUI coordinates intentionally do not enter this crate. The M3 layout
//! implementation will evolve `GraphState` while preserving its persistent
//! lane-state contract.

use std::collections::HashMap;

use sourcefour_model::{GRAPH_COLOR_COUNT, Oid};

pub use sourcefour_model::{GraphFlags, GraphRow, GraphSegment};

/// Identity of one continuing graph line, independent of the lane it occupies.
///
/// A line keeps its identity — and therefore its color — while it moves between
/// lanes, which is what stops colors from changing as the graph shifts.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GraphLineId(pub u64);

/// One occupied lane: the commit it expects next, and the line that owns it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lane {
    /// Commit that will be drawn in this lane when traversal reaches it.
    pub expected: Oid,
    /// Line owning the lane.
    pub line: GraphLineId,
}

/// Persistent state retained across incremental history batches.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GraphState {
    lanes: Vec<Option<Lane>>,
    colors: HashMap<GraphLineId, u8>,
    next_line_id: u64,
}

impl GraphState {
    /// Starts a new line and assigns the color it carries until termination.
    ///
    /// Prefers a color no live line is using, so adjacent lines differ whenever
    /// the palette allows it.
    pub fn create_line(&mut self) -> GraphLineId {
        let line = GraphLineId(self.next_line_id);
        // Probe from this line's own index, before it advances, so the first
        // line of a scope starts at color 0.
        let color = self.unused_color();
        self.next_line_id += 1;
        self.colors.insert(line, color);
        line
    }

    /// Color carried by a live line, or `None` once the line has terminated.
    #[must_use]
    pub fn color(&self, line: GraphLineId) -> Option<u8> {
        self.colors.get(&line).copied()
    }

    /// Forgets a terminated line so its color returns to the palette.
    pub fn terminate_line(&mut self, line: GraphLineId) {
        self.colors.remove(&line);
    }

    /// Returns semantic lane occupancy from the preceding batch.
    #[must_use]
    pub fn lanes(&self) -> &[Option<Lane>] {
        &self.lanes
    }

    /// Replaces lane occupancy after a completed semantic layout step.
    ///
    /// This deliberately accepts owned lane state and performs no coordinate
    /// calculation, keeping the crate reusable by the M3 layout algorithm.
    pub fn replace_lanes(&mut self, lanes: Vec<Option<Lane>>) {
        self.lanes = lanes;
    }

    /// Clears all retained topology before a new history scope begins.
    pub fn reset(&mut self) {
        self.lanes.clear();
        self.colors.clear();
        self.next_line_id = 0;
    }

    /// First color not carried by a live line, else the next in rotation.
    fn unused_color(&self) -> u8 {
        let start = u8::try_from(self.next_line_id % u64::from(GRAPH_COLOR_COUNT))
            .expect("a remainder below GRAPH_COLOR_COUNT always fits in u8");
        (0..GRAPH_COLOR_COUNT)
            .map(|offset| (start + offset) % GRAPH_COLOR_COUNT)
            .find(|color| !self.colors.values().any(|live| live == color))
            .unwrap_or(start)
    }
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{GRAPH_COLOR_COUNT, Oid};

    use super::{GraphState, Lane};

    fn oid(last: u8) -> Oid {
        let mut bytes = [0_u8; 20];
        bytes[19] = last;
        Oid::sha1(bytes)
    }

    #[test]
    fn a_line_carries_one_color_until_it_terminates() {
        let mut state = GraphState::default();

        let line = state.create_line();
        let color = state.color(line).expect("a new line is colored");

        assert_eq!(state.color(line), Some(color));
        state.terminate_line(line);
        assert_eq!(state.color(line), None);
    }

    #[test]
    fn concurrent_lines_receive_distinct_colors_while_the_palette_allows() {
        let mut state = GraphState::default();

        let colors: Vec<u8> = (0..GRAPH_COLOR_COUNT)
            .map(|_| {
                let line = state.create_line();
                state.color(line).expect("a new line is colored")
            })
            .collect();

        let mut distinct = colors.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), colors.len());
        assert!(colors.iter().all(|color| *color < GRAPH_COLOR_COUNT));
    }

    #[test]
    fn a_terminated_color_returns_to_the_palette() {
        let mut state = GraphState::default();
        let lines: Vec<_> = (0..GRAPH_COLOR_COUNT)
            .map(|_| state.create_line())
            .collect();
        // A saturated palette assigns 0..GRAPH_COLOR_COUNT in order, so
        // terminating the third line is the only way color 2 becomes free.
        assert_eq!(state.color(lines[2]), Some(2));

        state.terminate_line(lines[2]);
        let reused = state.create_line();

        assert_eq!(state.color(reused), Some(2));
    }

    #[test]
    fn exhausting_the_palette_still_yields_a_usable_color() {
        let mut state = GraphState::default();
        for _ in 0..GRAPH_COLOR_COUNT {
            state.create_line();
        }

        let extra = state.create_line();

        let color = state.color(extra).expect("a new line is always colored");
        assert!(color < GRAPH_COLOR_COUNT);
    }

    #[test]
    fn line_identity_is_never_reused_after_termination() {
        let mut state = GraphState::default();

        let first = state.create_line();
        state.terminate_line(first);
        let second = state.create_line();

        assert_ne!(first, second);
    }

    #[test]
    fn reset_discards_prior_batch_lane_state_and_colors() {
        let mut state = GraphState::default();
        let line = state.create_line();
        state.replace_lanes(vec![
            Some(Lane {
                expected: oid(1),
                line,
            }),
            None,
        ]);
        assert_eq!(state.lanes().len(), 2);

        state.reset();

        assert!(state.lanes().is_empty());
        assert_eq!(state.color(line), None);
    }
}
