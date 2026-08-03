# ADR 0001: Pin GPUI exactly

## Status

Accepted. The exact proven GPUI revision is required before the foundation dependency configuration is accepted.

## Decision

GPUI must be pinned to an exact, proven dependency revision. The foundation owner will fill in the concrete revision after compatibility and build verification:

```text
GPUI revision: REQUIRED — not yet selected or proven
```

Do not depend on GPUI `main`, an unpinned Git branch, or a wildcard version. Keep GPUI consumption in `apps/sourcefour/` and isolate version-sensitive UI APIs behind a small internal compatibility boundary.

## Consequences

GPUI is pre-1.0, so an exact pin makes the supported API surface reproducible. This ADR intentionally does not claim that any revision has been benchmarked or proven until foundation records that evidence.
