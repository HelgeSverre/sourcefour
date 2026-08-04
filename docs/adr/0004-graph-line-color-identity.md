# ADR 0004: Color belongs to a graph line, not a lane

## Status

Accepted.

## Context

Specification §7.2 defines `GraphRow` with `node_color`, `GraphSegment` variants that each carry a color, and a `GraphState` holding `colors: HashMap<GraphLineId, u8>` and `next_line_id`. §7.3 requires that a color be assigned when a line is created and carried until the line terminates, precisely so that a line keeps its color when it shifts lanes.

The Milestone 0 model instead defined `GraphRow { oid, lane: u32, segments }` with a flat `GraphSegment { from_lane, to_lane, kind }` and no color anywhere, and `GraphState` retained only `lanes: Vec<Option<Oid>>`. That shape cannot express §7.3 at all: with no line identity and no color store, the only available rule is `lane % 6`, which is the exact behavior §7.3 forbids. Because `HistoryBatch` carries `GraphRow`, the shape would have been frozen into the Milestone 2 transport before Milestone 3 discovered it could not be implemented.

## Decision

Adopt the §7.2 shapes. `GraphRow` becomes `{ node_lane: u16, node_color: u8, segments, flags }` and `GraphSegment` becomes an enum of `Vertical`, `Fork`, `Merge`, and `Terminate`, each carrying its line's color.

Three points where §7 is silent or where this repository must differ are resolved as follows.

1. **`GraphFlags` is undefined by the specification.** Define it as the row properties a painter needs but cannot infer from segments: `is_merge`, `is_root`, and `is_shallow_boundary`. §3 requires merge commits to be de-emphasized, §7.1 requires validity across multiple roots and shallow boundaries, and none of those are derivable from lane geometry.

2. **§7.2 shows `colors` keyed by `GraphLineId` but never states how a lane maps to a line.** A lane therefore stores `Lane { expected: Oid, line: GraphLineId }` rather than a bare object ID. Without this the color store is unreachable from the lane algorithm and the field is decorative.

3. **`GraphRow` drops the `oid` field the Milestone 0 model carried.** `HistoryBatch` already specifies that `graph_rows` parallel `rows` in the same order, so commit identity lives on the commit row. Duplicating it invites the two vectors to disagree.

Lane indices narrow from `u32` to `u16` per §7.2. `GRAPH_MAX_LANES` records the §7.6 safety cap of 128, and `GRAPH_COLOR_COUNT` is the single source of the palette size — the theme's lane palette is declared as `[Hsla; GRAPH_COLOR_COUNT as usize]` so a color index cannot outrun the colors that exist.

Color selection prefers a color no live line currently carries, probing from the new line's own index. Exhausting the palette reuses a color rather than failing; §7.3 asks for distinct neighbors only "when possible".

## Consequences

Milestone 3 can implement §7.3 without reshaping the history transport, and the compiler now rejects a palette that disagrees with the model. The lane algorithm itself — which lane receives which line, and when forks and merges are emitted — remains Milestone 3 work; this ADR settles only the types it will produce.

`GraphSegmentKind` is removed. Nothing outside `sourcefour-graph` consumed it, so no migration is required.
