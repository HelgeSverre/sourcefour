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
| Date/time (UTC) | not yet measured |
| Sourcefour revision | not yet measured |
| Build profile and Rust toolchain | not yet measured |
| Operating system/version | not yet measured |
| Hardware model, CPU/GPU, RAM | not yet measured |
| Storage/filesystem | not yet measured |
| Power and thermal state | not yet measured |
| Fixture name and manifest | not yet measured |
| Fixture commit/branch/tag counts and commit-graph state | not yet measured |
| Cache condition (cold/warm) | not yet measured |
| Exact command | not yet measured |
| Iteration count and sampling method | not yet measured |
| Raw samples/results path or link | not yet measured |
| Profiling trace path or link | not yet measured |
| Notes, deviations, and failures | not yet measured |

## Result table

| Run record | Metric | Cache condition | p50 | p95 | Budget | Raw result link/path |
| --- | --- | --- | --- | --- | --- | --- |
| none | not yet measured | not yet measured | not yet measured | not yet measured | see measurement contract | not yet measured |

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
