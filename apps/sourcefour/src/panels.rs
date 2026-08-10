//! User-resizable panel geometry.
//!
//! Plain math, no GPUI: the splitter handles set `dragging`, the window routes
//! mouse positions here, and rendering reads the clamped sizes back.

use crate::theme::{DETAILS_HEIGHT, GRAPH_WIDTH, SIDEBAR_WIDTH, SPLITTER_WIDTH, STATUS_HEIGHT};

/// Which divider a drag is currently moving.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Splitter {
    /// Between the sidebar and the history pane.
    Sidebar,
    /// Between the graph column and the commit text columns.
    Graph,
    /// Between the history list and the details panel.
    Details,
    /// Above the optional Actions timeline footer.
    ActionsTimeline,
}

/// Current, user-adjusted panel sizes in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PanelSizes {
    pub(crate) sidebar: f32,
    pub(crate) graph: f32,
    pub(crate) details: f32,
    pub(crate) actions_timeline: f32,
}

impl Default for PanelSizes {
    fn default() -> Self {
        Self {
            sidebar: SIDEBAR_WIDTH,
            graph: GRAPH_WIDTH,
            details: DETAILS_HEIGHT,
            actions_timeline: 180.0,
        }
    }
}

impl PanelSizes {
    pub(crate) fn actions_timeline_for(self, window_height: f32) -> f32 {
        self.actions_timeline
            .min((window_height * 0.45).clamp(96.0, 480.0))
    }

    /// Applies persisted sizes, clamped exactly like a live drag so a stale
    /// or hand-edited state file cannot produce an unusable layout.
    pub(crate) fn apply(&mut self, state: &crate::ui_state::UiState) {
        if let Some(sidebar) = state.sidebar_width {
            self.sidebar = sidebar.clamp(170.0, 480.0);
        }
        if let Some(graph) = state.graph_width {
            self.graph = graph.clamp(40.0, 720.0);
        }
        if let Some(details) = state.details_height {
            self.details = details.clamp(100.0, 560.0);
        }
        if let Some(height) = state.actions_timeline_height {
            self.actions_timeline = height.clamp(96.0, 480.0);
        }
    }

    /// Applies a pointer position to the panel a splitter controls.
    ///
    /// Every size is clamped so no panel can vanish or swallow the window;
    /// the graph maximum is generous because wide histories need dozens of
    /// 18px lanes on screen at once.
    pub(crate) fn drag(&mut self, splitter: Splitter, x: f32, y: f32, window_height: f32) {
        match splitter {
            Splitter::Sidebar => self.sidebar = x.clamp(170.0, 480.0),
            Splitter::Graph => {
                self.graph = (x - self.sidebar - SPLITTER_WIDTH).clamp(40.0, 720.0);
            }
            Splitter::Details => {
                self.details = (window_height - STATUS_HEIGHT - y).clamp(100.0, 560.0);
            }
            Splitter::ActionsTimeline => {
                let maximum = (window_height * 0.45).clamp(96.0, 480.0);
                self.actions_timeline = (window_height - 26.0 - y).clamp(96.0, maximum);
            }
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "sizes are set from the exact constants the tests compare against"
)]
mod tests {
    use crate::theme::{DETAILS_HEIGHT, GRAPH_WIDTH, SIDEBAR_WIDTH, STATUS_HEIGHT};

    use super::{PanelSizes, Splitter};

    #[test]
    fn panels_start_at_the_visual_contract_defaults() {
        let sizes = PanelSizes::default();

        assert_eq!(
            (
                sizes.sidebar,
                sizes.graph,
                sizes.details,
                sizes.actions_timeline,
            ),
            (SIDEBAR_WIDTH, GRAPH_WIDTH, DETAILS_HEIGHT, 180.0)
        );
    }

    #[test]
    fn the_sidebar_follows_the_pointer_within_bounds() {
        let mut sizes = PanelSizes::default();

        sizes.drag(Splitter::Sidebar, 300.0, 0.0, 800.0);
        assert_eq!(sizes.sidebar, 300.0);

        sizes.drag(Splitter::Sidebar, 10.0, 0.0, 800.0);
        assert!(
            sizes.sidebar > 100.0,
            "a sidebar cannot collapse to nothing"
        );

        sizes.drag(Splitter::Sidebar, 2000.0, 0.0, 800.0);
        assert!(sizes.sidebar < 600.0, "a sidebar cannot swallow the window");
    }

    #[test]
    fn the_graph_column_is_measured_from_the_sidebar_edge() {
        let mut sizes = PanelSizes::default();
        sizes.drag(Splitter::Sidebar, 300.0, 0.0, 800.0);

        sizes.drag(Splitter::Graph, 500.0, 0.0, 800.0);

        assert!(
            (sizes.graph - 200.0).abs() <= 4.0,
            "pointer at x=500 with a 300px sidebar leaves ~200px of graph, got {}",
            sizes.graph
        );
    }

    #[test]
    fn the_graph_column_can_grow_far_enough_for_many_lanes() {
        let mut sizes = PanelSizes::default();

        sizes.drag(Splitter::Graph, 5000.0, 0.0, 800.0);

        // §7.4: 18px per lane. Wide history needs dozens of visible lanes.
        assert!(sizes.graph >= 30.0 * 18.0);

        sizes.drag(Splitter::Graph, 0.0, 0.0, 800.0);
        assert!(
            sizes.graph >= 40.0,
            "the graph column keeps a usable minimum"
        );
    }

    #[test]
    fn the_details_panel_resizes_from_the_bottom_edge() {
        let mut sizes = PanelSizes::default();

        sizes.drag(Splitter::Details, 0.0, 500.0, 800.0);
        assert_eq!(sizes.details, 800.0 - STATUS_HEIGHT - 500.0);

        sizes.drag(Splitter::Details, 0.0, 790.0, 800.0);
        assert!(sizes.details >= 100.0, "details cannot collapse entirely");

        sizes.drag(Splitter::Details, 0.0, 0.0, 800.0);
        assert!(sizes.details <= 560.0, "details cannot swallow the history");
    }

    #[test]
    fn the_actions_timeline_resizes_without_swallowing_the_overlay() {
        let mut sizes = PanelSizes::default();

        sizes.drag(Splitter::ActionsTimeline, 0.0, 600.0, 900.0);
        assert_eq!(sizes.actions_timeline, 274.0);

        sizes.drag(Splitter::ActionsTimeline, 0.0, 890.0, 900.0);
        assert_eq!(sizes.actions_timeline, 96.0);

        sizes.drag(Splitter::ActionsTimeline, 0.0, 0.0, 900.0);
        assert_eq!(sizes.actions_timeline, 405.0);
    }
}
