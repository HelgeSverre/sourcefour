# Performance measurement ledger

This ledger records measured Sourcefour performance. It contains no estimated, projected, or fabricated results. Leave result fields as `not yet measured` until a reproducible run records raw artifacts.

## Measurement contract

Run release builds on documented Apple Silicon and Linux reference machines. Record cold filesystem-cache runs separately from warm runs. The representative target fixture has 50,000 commits, 100 branches, and a commit-graph file.

| Metric | Budget |
| --- | --- |
| Process start to first window frame, warm | p50 < 100 ms; p95 < 180 ms |
| Repository discovery + initial metadata | p50 < 30 ms; p95 < 80 ms |
| First 256 history rows after open | p50 < 50 ms; p95 < 150 ms |
| History batch of 512 | p50 < 35 ms warm |
| Filter 5,000 loaded rows | < 16 ms |
| Arrow selection to immediate header update | < 8 ms |
| Commit detail metadata | p50 < 40 ms warm |
| File list for ordinary commit | p50 < 100 ms warm |
| Scroll rendering | no sustained frame over 16.7 ms at 60 Hz |
| Idle CPU | approximately 0% absent watchers/timers |

Stable-fixture regressions greater than 15% require explicit review.

## Required run record

Create one entry per machine, fixture, revision, and cache condition.

| Field | Value |
| --- | --- |
| Date/time (UTC) | 2026-08-04 |
| Sourcefour revision | 2bb78bd |
| Build profile and Rust toolchain | release, rustc 1.97.0 (2d8144b78 2026-07-07) |
| Operating system/version | macOS 15.6 |
| Hardware model, CPU/GPU, RAM | MacBook Pro (Mac14,6), Apple M2 Max, 32 GB |
| Storage/filesystem | internal Apple SSD, APFS |
| Power and thermal state | not controlled (interactive session) |
| Fixture name and manifest | 18 real repositories; names and shapes in the raw file |
| Fixture commit/branch/tag counts and commit-graph state | 176 – 1,464,430 commits per repository; no commit-graph files generated |
| Cache condition (cold/warm) | warm |
| Exact command | `cargo run --release -p sourcefour-git --example stress -- <repos>` |
| Iteration count and sampling method | 1 iteration per repository (single sample, not a percentile) |
| Raw samples/results path or link | `fixtures/stress/2026-08-04-warm.txt` |
| Profiling trace path or link | none |
| Notes, deviations, and failures | Headless harness, not the windowed app: covers discovery, metadata snapshot, history cursor, and §7.3 layout, not rendering. Real repositories replace the not-yet-built 50k reference fixture. git.git and linux.git exceed the §7.6 lane cap; overflow-lane degradations are counted in the raw file and confined to the parked lane. Cross-row graph continuity validated on every row: zero violations in 1,730,184 rows. |

## Result table

Single-sample values from the run record above; p95 requires repeated runs and stays unmeasured.

| Run record | Metric | Cache condition | p50 | p95 | Budget | Raw result link/path |
| --- | --- | --- | --- | --- | --- | --- |
| 2026-08-04 | Repository discovery + initial metadata | warm | 0–5 ms + 3–38 ms across 18 repositories | not yet measured | p50 < 30 ms; p95 < 80 ms | `fixtures/stress/2026-08-04-warm.txt` |
| 2026-08-04 | First 256 history rows | warm | 2–55 ms for repositories up to 85k commits; 218 ms for linux (1.46M commits, 29× the reference fixture) | not yet measured | p50 < 50 ms; p95 < 150 ms | `fixtures/stress/2026-08-04-warm.txt` |
| 2026-08-04 | Full history walk + layout throughput | warm | 41,000–260,000 rows/s per repository | not yet measured | no budget defined | `fixtures/stress/2026-08-04-warm.txt` |

## Commands to record verbatim

The final command set is owned by the package and performance work; record the exact commands used, including environment variables and fixture paths. Candidate categories are:

| Measurement | Exact command | Raw-result path/link |
| --- | --- | --- |
| Release build | not yet measured | not yet measured |
| App startup/first frame | not yet measured | not yet measured |
| Discovery and metadata | not yet measured | not yet measured |
| Initial history and subsequent batch | not yet measured | not yet measured |
| Filter and selection responsiveness | not yet measured | not yet measured |
| Details/file list/diff | not yet measured | not yet measured |
| Graph benchmark | not yet measured | not yet measured |
| Profile trace (`SOURCEFOUR_PROFILE=1`, if implemented) | not yet measured | not yet measured |

Do not infer a result from a budget, an implementation change, or an unarchived terminal session. Link raw samples under `fixtures/`, a benchmark artifact directory, or another durable repository-relative path.
