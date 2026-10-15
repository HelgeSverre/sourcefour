# Sourcefour — agent notes

## Commands

Use the justfile. `just --list` shows everything, grouped: run, build, check, dist.

- `just run [PATH]` / `just demo` — launch the app
- `just test [NAME-FILTER]`, `just test-crate <crate>` — tests
- `just check` — the full pre-push gate, same as CI

Drop to `cargo nextest run …` directly when you need flags no recipe covers
(`-E` filter expressions, `--no-capture`). Never `cargo test` — nextest is the
runner here, and it does not run doctests. We don't write doctests; put
examples in `#[test]` functions instead.

## Visual evidence

When a task changes or verifies a visual result, capture a screenshot that
clearly shows the final state. Attach that screenshot to the relevant pull
request, issue, release follow-up, or other user-facing report when one exists;
do not consider the visual work complete with only logs or programmatic checks.
If a screenshot cannot be produced, state the reason explicitly in the final
report.
