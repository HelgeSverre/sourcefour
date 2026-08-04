//! The incremental lane algorithm from §7.3.
//!
//! Laying out one commit at a time — rather than one batch at a time — is what
//! keeps earlier rows stable when the next batch arrives: `push` returns each
//! row by value and never revisits one, so §7.1's "earlier rows do not move" is
//! a property of this signature rather than something a test here could refute.
//! The batching that could actually break it is tested against the cursor.

use smallvec::SmallVec;
use sourcefour_model::{GRAPH_MAX_LANES, GraphFlags, GraphRow, GraphSegment, Oid};

use crate::{GraphState, Lane};

impl GraphState {
    /// Lays out one commit and returns its finished row.
    ///
    /// Commits must arrive in traversal order, parents after children.
    pub fn push(&mut self, oid: Oid, parents: &[Oid]) -> GraphRow {
        let mut segments = SmallVec::new();
        let expecting = self.lanes_expecting(oid);
        let node_lane = match expecting.first() {
            Some(index) => *index,
            None => self.claim_free_lane(),
        };
        let owning_line = match self.lane(node_lane) {
            // A lane was already waiting for this commit: keep its line, and
            // therefore its color, exactly as earlier rows drew it.
            Some(lane) => lane.line,
            None => self.create_line(),
        };
        let node_color = self.color(owning_line).unwrap_or_default();

        // Every other lane waiting for this same commit converges into the node.
        for lane_index in expecting.iter().skip(1).copied() {
            let Some(lane) = self.lane(lane_index) else {
                continue;
            };
            let color = self.color(lane.line).unwrap_or_default();
            segments.push(GraphSegment::Fork {
                from: narrow(lane_index),
                to: narrow(node_lane),
                color,
            });
            self.terminate_line(lane.line);
            self.free_lane(lane_index);
        }

        // Lines untouched by this commit pass straight through.
        for (index, lane) in self.occupied_lanes() {
            if index == node_lane {
                continue;
            }
            segments.push(GraphSegment::Vertical {
                lane: narrow(index),
                color: self.color(lane.line).unwrap_or_default(),
            });
        }

        match parents.split_first() {
            None => {
                // A root: the line ends here rather than continuing off-screen.
                segments.push(GraphSegment::Terminate {
                    lane: narrow(node_lane),
                    color: node_color,
                });
                self.terminate_line(owning_line);
                self.free_lane(node_lane);
            }
            Some((first, rest)) => {
                self.occupy(
                    node_lane,
                    Lane {
                        expected: *first,
                        line: owning_line,
                    },
                );
                for parent in rest {
                    let (target, color) = self.route_merge_parent(*parent);
                    segments.push(GraphSegment::Merge {
                        from: narrow(node_lane),
                        to: narrow(target),
                        color,
                    });
                }
            }
        }

        self.compact_trailing_lanes();
        GraphRow {
            node_lane: narrow(node_lane),
            node_color,
            segments,
            flags: GraphFlags {
                is_merge: parents.len() > 1,
                is_root: parents.is_empty(),
                is_shallow_boundary: false,
            },
        }
    }

    /// Sends a merge parent to the lane already awaiting it, or to a new one.
    fn route_merge_parent(&mut self, parent: Oid) -> (usize, u8) {
        if let Some(existing) = self.lanes_expecting(parent).first().copied() {
            let color = self
                .lane(existing)
                .and_then(|lane| self.color(lane.line))
                .unwrap_or_default();
            return (existing, color);
        }
        let target = self.claim_free_lane();
        let line = self.create_line();
        let color = self.color(line).unwrap_or_default();
        self.occupy(
            target,
            Lane {
                expected: parent,
                line,
            },
        );
        (target, color)
    }
}

/// Lane indices are `u16` per §7.2 and capped per §7.6, so this never truncates.
fn narrow(lane: usize) -> u16 {
    u16::try_from(lane).unwrap_or(GRAPH_MAX_LANES - 1)
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{GraphSegment, Oid};

    use crate::GraphState;

    /// Commits are named by a single byte so topology stays readable.
    fn oid(name: u8) -> Oid {
        let mut bytes = [0_u8; 20];
        bytes[19] = name;
        Oid::sha1(bytes)
    }

    /// Lays out `(commit, parents)` pairs in order and returns the rows.
    fn layout(commits: &[(u8, &[u8])]) -> Vec<sourcefour_model::GraphRow> {
        let mut state = GraphState::default();
        commits
            .iter()
            .map(|(name, parents)| {
                let parents: Vec<Oid> = parents.iter().map(|p| oid(*p)).collect();
                state.push(oid(*name), &parents)
            })
            .collect()
    }

    fn merges(row: &sourcefour_model::GraphRow) -> Vec<(u16, u16)> {
        row.segments
            .iter()
            .filter_map(|segment| match segment {
                GraphSegment::Merge { from, to, .. } => Some((*from, *to)),
                _ => None,
            })
            .collect()
    }

    fn forks(row: &sourcefour_model::GraphRow) -> Vec<(u16, u16)> {
        row.segments
            .iter()
            .filter_map(|segment| match segment {
                GraphSegment::Fork { from, to, .. } => Some((*from, *to)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn linear_history_stays_in_one_lane() {
        let rows = layout(&[(3, &[2]), (2, &[1]), (1, &[])]);

        assert!(rows.iter().all(|row| row.node_lane == 0));
        assert!(rows.iter().all(|row| row.node_color == rows[0].node_color));
        assert!(rows[2].flags.is_root);
        assert!(rows.iter().take(2).all(|row| !row.flags.is_root));
    }

    #[test]
    fn a_merge_routes_its_second_parent_to_another_lane() {
        // 3 merges 2 and 1; both descend from root 0.
        let rows = layout(&[(3, &[2, 1]), (2, &[0]), (1, &[0]), (0, &[])]);

        assert!(rows[0].flags.is_merge);
        assert_eq!(rows[0].node_lane, 0);
        assert_eq!(
            merges(&rows[0]),
            [(0, 1)],
            "the second parent leaves the node's lane"
        );
        assert_eq!(rows[1].node_lane, 0);
        assert_eq!(rows[2].node_lane, 1);
    }

    #[test]
    fn converging_lanes_fork_into_the_node() {
        // Both 2 and 1 have parent 0, so two lanes await 0 when it arrives.
        let rows = layout(&[(3, &[2, 1]), (2, &[0]), (1, &[0]), (0, &[])]);

        assert_eq!(
            forks(&rows[3]),
            [(1, 0)],
            "the second lane converges into the node's lane"
        );
        assert!(rows[3].flags.is_root);
    }

    #[test]
    fn an_octopus_merge_routes_every_extra_parent() {
        let rows = layout(&[(9, &[1, 2, 3]), (1, &[]), (2, &[]), (3, &[])]);

        assert!(rows[0].flags.is_merge);
        assert_eq!(merges(&rows[0]), [(0, 1), (0, 2)]);
    }

    #[test]
    fn a_line_keeps_its_color_when_it_changes_lane() {
        // 2's lane is freed when it ends, so 1 later occupies a lower lane.
        let rows = layout(&[(3, &[2, 1]), (2, &[]), (1, &[])]);

        let lane_one_color = rows[0]
            .segments
            .iter()
            .find_map(|segment| match segment {
                GraphSegment::Merge { to: 1, color, .. } => Some(*color),
                _ => None,
            })
            .expect("the merge parent has a color");
        assert_eq!(
            rows[2].node_color, lane_one_color,
            "the line carries its color even after lanes shift (§7.3)"
        );
    }

    #[test]
    fn disconnected_roots_each_get_their_own_lane() {
        let rows = layout(&[(1, &[]), (2, &[]), (3, &[])]);

        assert!(rows.iter().all(|row| row.flags.is_root));
        assert!(
            rows.iter().all(|row| row.node_lane == 0),
            "a terminated lane is reused rather than growing the graph"
        );
    }

    #[test]
    fn passing_lines_are_drawn_as_vertical_segments() {
        let rows = layout(&[(3, &[2, 1]), (2, &[0]), (1, &[0]), (0, &[])]);

        let verticals: Vec<u16> = rows[1]
            .segments
            .iter()
            .filter_map(|segment| match segment {
                GraphSegment::Vertical { lane, .. } => Some(*lane),
                _ => None,
            })
            .collect();
        assert_eq!(
            verticals,
            [1],
            "the waiting second parent's lane passes through this row"
        );
    }

    #[test]
    fn a_criss_cross_merge_is_laid_out_deterministically() {
        let topology: &[(u8, &[u8])] = &[
            (6, &[4, 5]),
            (5, &[2, 3]),
            (4, &[3, 2]),
            (3, &[1]),
            (2, &[1]),
            (1, &[]),
        ];

        assert_eq!(
            layout(topology),
            layout(topology),
            "identical input must produce byte-identical layout (§12.1)"
        );
    }

    #[test]
    fn a_duplicated_parent_claims_only_one_lane() {
        // A malformed commit listing the same parent twice must not fan out.
        let rows = layout(&[(2, &[1, 1]), (1, &[])]);

        assert_eq!(
            merges(&rows[0]),
            [(0, 0)],
            "the duplicate parent resolves to the lane already awaiting it"
        );
        assert_eq!(rows[1].node_lane, 0);
    }

    #[test]
    fn many_simultaneous_lanes_stay_within_the_safety_cap() {
        // One commit with 200 parents exceeds the §7.6 visual lane cap.
        let parents: Vec<u8> = (1..=200).collect();
        let mut state = GraphState::default();
        let parent_oids: Vec<Oid> = parents.iter().map(|p| oid(*p)).collect();

        let row = state.push(oid(0), &parent_oids);

        assert!(
            row.segments.iter().all(|segment| match segment {
                GraphSegment::Merge { to, .. } => *to < sourcefour_model::GRAPH_MAX_LANES,
                _ => true,
            }),
            "no lane index may exceed the safety cap (§7.6)"
        );
    }
}
