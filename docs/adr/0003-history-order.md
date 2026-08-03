# ADR 0003: Default to all-refs topological-date history

## Status

Accepted.

## Decision

The default history scope starts from the unique tips of active `HEAD`, local branches, remote-tracking branches, and tags. Traverse that combined scope in topological-date order. History loading is incremental: initially 256 rows, then approximately 512 rows when the consumer approaches the loaded tail.

Selecting a local or remote ref replaces the scope with that ref. Selecting the already-selected ref returns to the all-refs scope. Filtering is a case-insensitive substring projection over loaded rows only; it does not change traversal or perform a full-history search.

## Consequences

This presents reachable history across the repository by default while preserving an explicit ref-focused view. The ordering and scope behavior are normative; implementation-specific traversal details must preserve them and be covered by fixture tests.
