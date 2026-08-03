//! Pure semantic graph-layout state.
//!
//! GPUI coordinates intentionally do not enter this crate. The M3 layout
//! implementation will evolve `GraphState` while preserving its persistent
//! lane-state contract.

use sourcefour_model::{GraphRow, Oid};

pub use sourcefour_model::{GraphSegment, GraphSegmentKind};

/// Persistent state retained across incremental history batches.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GraphState {
    lanes: Vec<Option<Oid>>,
}

impl GraphState {
    /// Returns semantic lane occupancy from the preceding batch.
    #[must_use]
    pub fn lanes(&self) -> &[Option<Oid>] {
        &self.lanes
    }

    /// Replaces lane occupancy after a completed semantic layout step.
    ///
    /// This deliberately accepts owned object IDs and performs no coordinate
    /// calculation, keeping the M0 scaffold honest and reusable by M3.
    pub fn replace_lanes(&mut self, lanes: Vec<Option<Oid>>) {
        self.lanes = lanes;
    }

    /// Clears all retained topology before a new history scope begins.
    pub fn reset(&mut self) {
        self.lanes.clear();
    }
}

/// Semantic output row shared with history transport.
pub type SemanticGraphRow = GraphRow;

#[cfg(test)]
mod tests {
    use sourcefour_model::Oid;

    use super::GraphState;

    #[test]
    fn reset_discards_prior_batch_lane_state() -> Result<(), sourcefour_model::OidParseError> {
        let oid = Oid::from_hex("0123456789abcdef0123456789abcdef01234567")?;
        let mut state = GraphState::default();
        state.replace_lanes(vec![Some(oid), None]);
        assert_eq!(state.lanes().len(), 2);

        state.reset();
        assert!(state.lanes().is_empty());
        Ok(())
    }
}
