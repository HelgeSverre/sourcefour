//! §7.4 graph geometry: one row's segments to paintable shapes.
//!
//! Everything here is plain math in graph-column coordinates, so the painting
//! decisions are testable; the canvas element maps these shapes onto GPUI
//! primitives and adds the element origin.

use sourcefour_model::{GraphRow, GraphSegment};

/// Horizontal distance between lane centers (§7.4).
pub(crate) const LANE_STEP: f32 = 18.0;
/// X of lane 0's center inside the graph column.
pub(crate) const LEFT_PADDING: f32 = 14.0;
/// Radius of a commit node (§7.4).
pub(crate) const NODE_RADIUS: f32 = 4.0;
/// Width of every line and curve (§7.4).
pub(crate) const STROKE_WIDTH: f32 = 2.0;
/// Radius of the HEAD halo ring. Its §7.4 stroke width of 1 is what
/// `gpui::outline` paints by default.
pub(crate) const HALO_RADIUS: f32 = 7.5;
/// Opacity of the HEAD halo ring (§7.4).
pub(crate) const HALO_OPACITY: f32 = 0.55;

/// One paintable primitive in graph-column coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Shape {
    /// A straight vertical line within one lane.
    Line {
        x: f32,
        top: f32,
        bottom: f32,
        color: u8,
    },
    /// An S-curve between lanes, vertical tangents at both ends (§7.4).
    Curve {
        from: (f32, f32),
        to: (f32, f32),
        color: u8,
    },
    /// The commit node, always the row's last shape so lines stay under it.
    Node {
        x: f32,
        y: f32,
        color: u8,
        halo: bool,
    },
}

/// X coordinate of a lane's center.
fn lane_x(lane: u16) -> f32 {
    LEFT_PADDING + f32::from(lane) * LANE_STEP
}

/// Appends one row's shapes, in paint order, to `shapes`.
pub(crate) fn row_shapes(
    row_top: f32,
    row_height: f32,
    graph: &GraphRow,
    is_head: bool,
) -> Vec<Shape> {
    let center = row_top + row_height / 2.0;
    let bottom = row_top + row_height;
    let node_x = lane_x(graph.node_lane);
    let mut shapes = Vec::with_capacity(graph.segments.len() + 3);
    if graph.flags.continues_above {
        shapes.push(Shape::Line {
            x: node_x,
            top: row_top,
            bottom: center,
            color: graph.node_color,
        });
    }
    if !graph.flags.is_root {
        shapes.push(Shape::Line {
            x: node_x,
            top: center,
            bottom,
            color: graph.node_color,
        });
    }
    for segment in &graph.segments {
        match *segment {
            GraphSegment::Vertical { lane, color } => shapes.push(Shape::Line {
                x: lane_x(lane),
                top: row_top,
                bottom,
                color,
            }),
            GraphSegment::Fork { from, to, color } => shapes.push(Shape::Curve {
                from: (lane_x(from), row_top),
                to: (lane_x(to), center),
                color,
            }),
            GraphSegment::Merge { from, to, color } if from != to => shapes.push(Shape::Curve {
                from: (lane_x(from), center),
                to: (lane_x(to), bottom),
                color,
            }),
            // No geometry: a duplicate merge parent routed to the node's own
            // lane is already the straight line below the node, and a
            // terminated line's ending stub above the node is already drawn by
            // `continues_above` (§7.5 downward stubs arrive with filtering).
            GraphSegment::Merge { .. } | GraphSegment::Terminate { .. } => {}
        }
    }
    shapes.push(Shape::Node {
        x: node_x,
        y: center,
        color: graph.node_color,
        halo: is_head,
    });
    shapes
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "shapes are built from the exact constants the tests compare against"
)]
mod tests {
    use smallvec::SmallVec;
    use sourcefour_model::{GraphFlags, GraphRow, GraphSegment};

    use super::{HALO_RADIUS, LANE_STEP, LEFT_PADDING, NODE_RADIUS, Shape, row_shapes};

    const ROW: f32 = 30.0;

    fn row(node_lane: u16, segments: &[GraphSegment], flags: GraphFlags) -> GraphRow {
        GraphRow {
            node_lane,
            node_color: 3,
            segments: SmallVec::from_slice(segments),
            flags,
        }
    }

    fn passes_through() -> GraphFlags {
        GraphFlags {
            continues_above: true,
            ..GraphFlags::default()
        }
    }

    #[test]
    fn a_vertical_segment_spans_the_full_row() {
        let shapes = row_shapes(
            60.0,
            ROW,
            &row(
                0,
                &[GraphSegment::Vertical { lane: 2, color: 5 }],
                passes_through(),
            ),
            false,
        );

        assert!(shapes.contains(&Shape::Line {
            x: LEFT_PADDING + 2.0 * LANE_STEP,
            top: 60.0,
            bottom: 90.0,
            color: 5,
        }));
    }

    #[test]
    fn the_node_line_connects_both_row_edges_when_history_continues() {
        let shapes = row_shapes(60.0, ROW, &row(1, &[], passes_through()), false);
        let x = LEFT_PADDING + LANE_STEP;

        assert!(
            shapes.contains(&Shape::Line {
                x,
                top: 60.0,
                bottom: 75.0,
                color: 3
            }),
            "the awaited line arrives from the row above"
        );
        assert!(
            shapes.contains(&Shape::Line {
                x,
                top: 75.0,
                bottom: 90.0,
                color: 3
            }),
            "the first parent's line leaves toward the row below"
        );
    }

    #[test]
    fn a_tip_has_no_line_above_and_a_root_none_below() {
        let tip = row_shapes(0.0, ROW, &row(0, &[], GraphFlags::default()), false);
        assert!(
            !tip.iter()
                .any(|shape| matches!(shape, Shape::Line { top, .. } if *top == 0.0)),
            "a new line starts at its node, not at the row edge"
        );

        let root = row_shapes(
            0.0,
            ROW,
            &row(
                0,
                &[],
                GraphFlags {
                    is_root: true,
                    continues_above: true,
                    ..GraphFlags::default()
                },
            ),
            false,
        );
        assert!(
            !root
                .iter()
                .any(|shape| matches!(shape, Shape::Line { bottom, .. } if *bottom == 30.0)),
            "a root's line must not continue below the node"
        );
    }

    #[test]
    fn a_merge_curve_leaves_the_node_toward_the_parent_lane_below() {
        let shapes = row_shapes(
            30.0,
            ROW,
            &row(
                0,
                &[GraphSegment::Merge {
                    from: 0,
                    to: 2,
                    color: 4,
                }],
                passes_through(),
            ),
            false,
        );

        assert!(shapes.contains(&Shape::Curve {
            from: (LEFT_PADDING, 45.0),
            to: (LEFT_PADDING + 2.0 * LANE_STEP, 60.0),
            color: 4,
        }));
    }

    #[test]
    fn a_fork_curve_arrives_at_the_node_from_the_lane_above() {
        let shapes = row_shapes(
            30.0,
            ROW,
            &row(
                0,
                &[GraphSegment::Fork {
                    from: 1,
                    to: 0,
                    color: 2,
                }],
                passes_through(),
            ),
            false,
        );

        assert!(shapes.contains(&Shape::Curve {
            from: (LEFT_PADDING + LANE_STEP, 30.0),
            to: (LEFT_PADDING, 45.0),
            color: 2,
        }));
    }

    #[test]
    fn a_merge_parent_already_in_the_node_lane_needs_no_curve() {
        // A malformed duplicate parent routes to the node's own lane (§7.3);
        // the straight line below the node already is that edge.
        let shapes = row_shapes(
            0.0,
            ROW,
            &row(
                0,
                &[GraphSegment::Merge {
                    from: 0,
                    to: 0,
                    color: 3,
                }],
                passes_through(),
            ),
            false,
        );

        assert!(
            !shapes
                .iter()
                .any(|shape| matches!(shape, Shape::Curve { .. }))
        );
    }

    #[test]
    fn the_node_is_painted_last_and_carries_the_head_halo() {
        let shapes = row_shapes(0.0, ROW, &row(1, &[], passes_through()), true);

        assert_eq!(
            shapes.last(),
            Some(&Shape::Node {
                x: LEFT_PADDING + LANE_STEP,
                y: 15.0,
                color: 3,
                halo: true,
            }),
            "lines under the node must not paint over it"
        );
    }

    #[test]
    fn the_spec_geometry_constants_hold() {
        assert_eq!((LANE_STEP, NODE_RADIUS, HALO_RADIUS), (18.0, 4.0, 7.5));
    }
}
