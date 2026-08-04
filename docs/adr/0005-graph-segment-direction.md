# ADR 0005: Fork points up the graph, Merge points down

## Status

Accepted.

## Context

Specification §7.2 names four `GraphSegment` variants — `Vertical`, `Fork`, `Merge`, and `Terminate` — but never says which direction each one runs, and §7.3 describes step 5 only as "emit a fork/merge curve from the node lane to the parent lane". Both names are ambiguous in a history drawn newest-first: the same physical curve is a fork when read from the root upward and a merge when read from the tip downward.

An implementation that guesses leaves the painter guessing too, and the two guesses need not agree.

## Decision

Direction is fixed relative to the row being drawn, with rows ordered newest-first.

- **`Vertical { lane, color }`** — a line occupying `lane` that neither starts nor ends at this row. It passes straight through.
- **`Merge { from, to, color }`** — an edge leaving the node's lane (`from`) and descending toward an additional parent's lane (`to`). Emitted once per parent after the first, so a two-parent merge emits one and an octopus merge emits `parents - 1`. `color` is the parent lane's color, not the node's, because the curve joins that line.
- **`Fork { from, to, color }`** — an edge arriving at the node's lane (`to`) from a lane above that was awaiting this same commit (`from`). Emitted when more than one lane expects the commit, which is how converging branches are drawn. `color` is the terminating lane's colour.
- **`Terminate { lane, color }`** — a line that ends at this row because the commit has no parents, or because a filtered-out parent leaves nothing to draw toward (§7.5).

The node itself is not a segment: `GraphRow` carries `node_lane` and `node_color` directly, as §7.2 specifies.

## Consequences

`Merge` counts always match `parents.len() - 1`, which makes octopus merges verifiable without inspecting geometry, and `Fork` counts match the number of redundant lane expectations resolved at that row. A painter can therefore render a row without consulting its neighbours, which is what §7.4 requires when only the visible range is painted.

The `from`/`to` fields are lane indices at the row's own boundaries; converting them to pixels stays entirely in the GPUI layer.
