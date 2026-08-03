# ADR 0001: Pin GPUI exactly

## Status

Accepted. The exact proven GPUI revision is required before the foundation dependency configuration is accepted.

## Decision

The application depends on GPUI from the Zed repository at exactly
`d637307bd75389f63adbb7d86bc66174ee817d0d` (the `v0.179.4` tag). Cargo records
the full transitive source graph in `Cargo.lock`; release and CI builds use
`--locked`.

The pin is limited to `apps/sourcefour/`, which is the workspace's sole GPUI
consumer. Domain, Git-backend, graph, and test-support crates remain independent
of GPUI.

Do not depend on GPUI `main`, an unpinned Git branch, or a wildcard version. Keep GPUI consumption in `apps/sourcefour/` and isolate version-sensitive UI APIs behind a small internal compatibility boundary.

## Consequences

GPUI is pre-1.0, so an exact pin makes the supported API surface reproducible.
At foundation time, `git ls-remote` resolved the SHA to Zed's `v0.179.4` tag and
the workspace's locked debug and release builds compile a minimal `gpui::App`
type proof. The API evidence used for the next visual-shell milestone is the
GPUI documentation for `App::open_window(WindowOptions, builder)`, background
spawning/entity APIs, and `#[gpui::test]`. This is dependency compatibility
evidence, not a performance measurement.
