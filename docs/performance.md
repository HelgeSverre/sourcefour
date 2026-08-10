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

## Run record — 2026-08-04 percentiles

Same machine, toolchain, and warm-cache conditions as the single-sample record
below; 10 iterations per repository via
`cargo run --release -p sourcefour-git --example stress -- --iterations 10 <repos>`.
Raw output: `fixtures/stress/2026-08-04-percentiles.txt`.

| Repository | Commits | Snapshot p50/p95 | First 256 rows p50/p95 | Full walk p50/p95 | Budget check |
| --- | --- | --- | --- | --- | --- |
| crescat | 21,572 | 7 / 56 ms | 14 / 30 ms | 105 / 130 ms | within budgets |
| nocodb | 36,437 | 4 / 41 ms | 7 / 16 ms | 172 / 206 ms | within budgets |
| wordpress | 62,969 | 5 / 41 ms | 11 / 12 ms | 690 / 778 ms | within budgets |
| git | 85,080 | 2 / 6 ms | 9 / 68 ms | 1,240 / 1,395 ms | within budgets |

Budgets: discovery + metadata p95 < 80 ms ✓; first 256 rows p95 < 150 ms ✓.
Startup is measured in the record below; scroll frame timing is not.

## Run record — 2026-08-05 startup phase breakdown

Phase probes (`startup_phase()` in `apps/sourcefour/src/app.rs`, printed under
`SOURCEFOUR_STARTUP_LOG=1`) split the first-frame time. Sampled 3 runs of the
release `--demo` build on the 2026-08-04 machine, under interactive load — the
absolute numbers run hot, the shares are the finding. Raw:
`fixtures/stress/2026-08-05-startup-phases.txt`.

| Phase | Share of first frame | What it is |
| --- | --- | --- |
| `main` → resolve | ~0 ms | argument parse, launch resolve |
| resolve → gpui run loop | ~50 ms | `Application::new()`, AppKit/Metal init |
| keymaps, menus | ~10 ms | Sourcefour setup |
| `open_window` + view built | ~30 ms | NSWindow creation; view construction is a sliver |
| render → first present | remainder (~90 ms+) | layout, glyph/SVG raster, pipeline warmup |

Conclusion: Sourcefour's own code is roughly 10–15 ms of the total; the rest is
framework platform init and first-frame rendering. No app-side easy wins exist
without upstream gpui work. The owner accepts p50 < 150 ms (2026-08-05); the
2026-08-04 record (p50 133 ms) is within that, while the original < 100 ms
budget line remains recorded as missed.

## Run record — 2026-08-05 preview scroll cost

The diff overlay's Preview built every block of both documents every frame.
This record is the before/after of virtualizing it (`gpui::list`, one item per
top-level block). Same machine, toolchain, and warm-cache conditions as the
2026-08-04 record: MacBook Pro (Mac14,6), Apple M2 Max, 32 GB, macOS 15.6,
release build, interactive session (not a quiet machine). Raw:
`fixtures/stress/2026-08-05-preview-scroll.txt`.

The text diff reader follows the same progressive rule: blob decoding and the
Histogram line diff run off the UI thread, immutable source text is stored once,
and unified/split rows contain only source indices. Syntax and intraline styles
arrive as a second generation-checked result. Files above the enrichment
thresholds remain fully readable as virtualized plain text.

Exact command, per sample — `SOURCEFOUR_PREVIEW_STRESS=N` repeats the demo
document N times on the new side, and the app quits itself in the first
frame's callback, so one run is one process:

```
SOURCEFOUR_STARTUP_LOG=1 SOURCEFOUR_FRAME_LOG=1 SOURCEFOUR_PREVIEW_STRESS=N \
  ./target/release/sourcefour --demo --scene preview --width 1280 --height 800
```

Three runs per stress level, before and after; the table gives the range.

| Stress | Blocks (both panes) | `preview-build-ms` before | after | `first-frame-ms` before | after |
| --- | --- | --- | --- | --- | --- |
| ×50 | 556 | 0.96 – 1.40 | 0.002 – 0.003 | 198 / 207 / 306 | 164 / 164 / 181 |
| ×200 | 2,206 | 3.94 – 5.68 | 0.002 – 0.003 | 222 / 253 / 266 | 159 / 166 / 174 |

The `preview-build-ms` collapse overstates the win on its own: the probe times
the render pass, and virtualization moves the per-block work out of it into the
list's layout, which builds only the blocks on screen plus 400 px of overdraw.
`first-frame-ms` is the honest number, and it is the finding — before, it grew
with the document (×50 ≈ 200 ms, ×200 ≈ 250 ms); after, it does not (both ≈ 165
ms, against ≈ 172 ms for the 17-block fixture). A 2,206-block document now costs
what a 17-block one does.

Scroll-frame cost tracks build cost here, since nothing can script a scroll:
the frames a scroll produces run the same path a first frame does, minus window
creation, so a per-frame document cost that no longer scales with length is the
measurement available. The probe stays in the tree
(`preview::build_probe`, printed under `SOURCEFOUR_FRAME_LOG=1`) so the next
regression is one run away.

## Run record — 2026-08-04 startup

Same machine and toolchain as above, release build, warm caches;
`scripts/measure-startup.sh 10 [repo]` with `SOURCEFOUR_STARTUP_LOG=1`.
The clock starts when the process enters `main` (dynamic-loader time before
`main` is excluded) and stops in the first frame's completion callback.
Raw samples: `fixtures/stress/2026-08-04-startup.txt`.

| Target | p50 | p95 | Budget | Verdict |
| --- | --- | --- | --- | --- |
| `--demo` (no repository I/O) | 133 ms | 167 ms | p50 < 100 ms; p95 < 180 ms | **p50 over budget**; p95 within |
| `~/code/sourcefour` (real repository) | 126 ms | 149 ms | p50 < 100 ms; p95 < 180 ms | **p50 over budget**; p95 within |

The first frame paints the loading shell, so repository size does not move
this number; the cost is window creation and the first GPUI/Metal frame.
Improving p50 needs profiling inside that path, not app-level changes.

## Run record — 2026-08-10 diff rendering

The text-diff overlay was profiled with a deterministic repeated demo diff and
opt-in probes around synchronous row projection and per-frame element
construction. Release-mode raw samples and the exact command are recorded in
`fixtures/stress/2026-08-10-diff-render.txt`.

At 7,401 unified rows and 401 hunks, projection took 0.138 ms and measured
element construction took 0.047–0.088 ms. Smaller 38-row and 1,851-row cases
were correspondingly cheaper. Both paths remain far below the 16.7 ms frame
budget, so proposed caches for hunk and widest-row scans were rejected: their
invalidation state would cost more complexity than the measurements justify.

The `SOURCEFOUR_DIFF_STRESS` fixture and `diff-project-ms` / `diff-build-ms`
probes remain available under `SOURCEFOUR_FRAME_LOG=1` for future regressions.

Frame instrumentation (`SOURCEFOUR_FRAME_LOG=1`) logs element-construction
time over 16.7 ms per frame; it measures the app's share only — layout,
paint, and GPU time happen inside gpui afterwards. Scroll frame timing
under automation remains not yet measured.

## Result table

Single-sample values from the first run record; superseded for the repositories
measured above.

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
| App startup/first frame | `SOURCEFOUR_STARTUP_LOG=1 scripts/measure-startup.sh 10 [repo]` | `fixtures/stress/2026-08-04-startup.txt` |
| Discovery and metadata | not yet measured | not yet measured |
| Initial history and subsequent batch | not yet measured | not yet measured |
| Filter and selection responsiveness | not yet measured | not yet measured |
| Details/file list/diff | not yet measured | not yet measured |
| Preview document build per frame | `SOURCEFOUR_STARTUP_LOG=1 SOURCEFOUR_FRAME_LOG=1 SOURCEFOUR_PREVIEW_STRESS=200 ./target/release/sourcefour --demo --scene preview --width 1280 --height 800` | `fixtures/stress/2026-08-05-preview-scroll.txt` |
| Graph benchmark | not yet measured | not yet measured |
| Profile trace (`SOURCEFOUR_PROFILE=1`, if implemented) | not yet measured | not yet measured |

Do not infer a result from a budget, an implementation change, or an unarchived terminal session. Link raw samples under `fixtures/`, a benchmark artifact directory, or another durable repository-relative path.
