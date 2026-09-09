# Context-menu surface inventory

Implemented 2026-09-09, following [ADR 0007](adr/0007-context-menus.md).
One host provides consistent gesture handling, navigation, focus, and dismissal
in both application windows and their overlays. Entries vary with the target
and already loaded data.

## Implemented surfaces

| Surface | Entries and behavior |
| --- | --- |
| Commit rows, commit details and collapsed detail strip, parent hashes | Copy full SHA; Copy subject where cached; Copy commit message where loaded; Create branch here when a repository is available. Parents target their own full ID. Opening does not change the history selection. |
| Local/remote branch rows; branch, tag and HEAD chips | Copy full branch/tag name, including folder prefixes; Copy full ref where known; Copy commit SHA; Create branch here. An annotated tag's SHA is its peeled commit. Nested chips claim their own menu before the commit row. |
| Changed-file rows, diff file-switcher results, diff header | Open diff on rows; Copy relative/absolute path; Reveal working copy; Copy old relative path for renames. Working-tree entries additionally offer Stage file or Unstage file. Comparison parent, revision, and staged side remain explicit. |
| STAGED / UNSTAGED section headers | Unstage all / Stage all, using the existing eligible-path filter and literal-path bulk transport. Conflict entries are excluded. |
| Worktree rows, repository title/path and status path | Copy native path; Reveal in file manager. Unavailable worktrees disable reveal. Bare repositories target the Git directory. Exact path copying is disabled when it cannot produce UTF-8 text. |
| Remote headers and fetch URLs | Copy remote name; Copy configured fetch URL; Fetch from this remote through the shared network controller. An SSH Git URL is not treated as a browser link. |
| Every TextInput: history filter, diff switcher, branch name, commit message, token, repository picker | Undo, Redo, Cut, Copy, Paste, Select all. These invoke the same editor methods as shortcuts. A context click inside a selection preserves it; outside moves the caret without dragging. Token-field behavior follows its existing keyboard policy. |
| Unified, split and wrapped text-diff cells | Copy selection when it includes the clicked side/line; Copy line; Copy relative path for that side. Copied content excludes UI line numbers and diff markers. Header menus also cover image, SVG and video previews' file paths. |
| Markdown paragraphs/headings/table text and code blocks | Copy selection within the clicked block/side; Copy text; Copy code. Code copies source without Markdown fences. Read-only context clicks preserve selection. |
| PR/check/workflow links; Actions run header and job rows | Open on GitHub (or in browser for external CI); Copy link; Copy run/job name where applicable. The run SHA chip copies the full available SHA. Control-click never triggers the workflow row's left-press prefetch. |
| About/settings links | Open in browser; Copy link through the same link helper. |
| Actions steps, log lines and log footer | Copy step name; Copy line; Copy step log; Copy job log when loaded. Copy scope is explicit and ANSI escapes are stripped. No synthetic text-selection action is offered for logs. |
| Repository-picker errors and operation messages | Copy message, using displayed text without a diagnostic fetch. |

Create branch here captures the clicked commit and repository in the existing
dialog, including at submission. Fetch from this remote uses the existing
controller's progress, cancellation, and mutual exclusion. Stage/unstage calls
the same controller as the buttons and rechecks that the captured path is still
eligible. Opening any of these menus performs no operation.

## Consistent interaction

Right-click opens a menu; Control-click does the same on macOS. Shift-F10 opens
the focused text field's menu or the selected visible commit's menu. Arrows and
Home/End navigate enabled entries, Enter invokes, Escape or Tab dismisses.
Menu keyboard input does not navigate history, submit a picker/dialog, or close
an underlying overlay. Other targets currently expose their menu by pointer.

An outside primary click dismisses without activating the underlying control.
A second context click replaces the menu. Focus returns to the original owner
unless another surface has already taken it. Scroll, resize/move, deactivation,
focus loss, and relevant owner changes close the menu. Blank areas, loading
placeholders, branch folders, splitters, and ordinary buttons have no invented
menu of their own; a button inside a meaningful surface can inherit that
surface's menu.

## Performance contract

- One optional popup per window, with a fixed set of host subscriptions.
  Closed menus build no entries or per-command callbacks. Virtualized rows
  register only lightweight target handlers.
- Opening consults cached identities and capabilities. It does not start Git,
  network requests, SVG rendering, or content reads. Ref names are projected
  during the existing metadata pass.
- Diff and preview selections retain source/range handles. Logs retain an
  immutable `Arc` revision; ANSI stripping and joining run on a worker only
  after Copy. Step ranges are cached instead of rescanned during each render.
- All application Copy/Cut paths share a sequence so an older log-copy worker
  cannot overwrite a newer copy. Weak owners prevent delivery to a closed
  view. This does not track other applications' clipboard writes.

## Verification

The nextest suite includes GPUI dispatch tests for right-click, Control-click,
menu replacement, deepest-target precedence, enabled-entry navigation, all
four window corners, outside-click consumption, focus restoration, scroll,
resize, deactivation, and focus loss. App-level tests cover:

- Right-click B with A selected: full 40/64-character SHA, unchanged selection
  and backend request tokens; menu keys cannot reach history shortcuts.
- Ref-chip identity, filter invalidation, rename paths, and conflict staging
  availability.
- Input Cut with preserved selection and Undo; Control-click caret placement;
  repository-picker Paste without submission.
- Create branch here retaining its target after selection changes and giving
  the dialog focus.
- Diff copy using the clicked source side and only its eligible selection.
- Job-log copy retaining the loaded revision after cache replacement and
  stripping ANSI; a newer synchronous copy superseding a pending ticket.
- Rendering/scrolling 100,000 staged/unstaged or committed files and copying
  the last changed-file row without exhausting the GPUI arena.

A local debug GPUI harness sample on 2026-09-09 measured five draws of the
100,000-file fixture at **37.9 ms closed / 43.1 ms open**. These are aggregate
harness draw timings from one run, not release-native latency or clipboard
benchmarks. `RUSTUP_TOOLCHAIN=1.97.1 just check` passed formatting, Clippy,
**407 tests**, and debug/release builds on the final implementation.
Nextest marked the existing
`app::tests::a_repository_resolves_to_its_name_and_worktree_path` test as leaky
on the last run; it passed, and the gate exited successfully. The preceding
full run had no leak flag.

The final release binary also stayed running for a 90-second smoke check on
`/Users/helge/code/crescat-website-fontawesome`, beyond the original crash's
roughly 49-second startup interval. It emitted no error output; sampled RSS
settled around 80–100 MB. The check ended by terminating only the process
started for verification. This establishes startup liveness, not native menu
interaction coverage.

Native screenshot/pointer verification remains limited by this environment's
Accessibility/Screen Recording permissions. GPUI event tests run without those
permissions. For manual checking, include menus near window edges, inside
Settings/diff/Actions overlays, and after switching applications. The
`SOURCEFOUR_STARTUP_LOG` probe exits after one frame, so it must not be used for
interaction or liveness checks.

## Explicit follow-ups

| Candidate | Required decision or integration |
| --- | --- |
| Open worktree in Sourcefour | Extract reusable repository opening and define behavior for an already open worktree. |
| Scope history to a ref | Add explicit Show history / Show all history commands rather than reusing a selection-dependent toggle. |
| Markdown span links | Resolve the span under the pointer and relative links against the previewed revision. |
| Copy/save image, SVG or video frame | Define original bytes versus rendered PNG versus poster/frame, including destination behavior. |
| Copy author email | Use loaded structured author data, not parsing formatted labels. |
| Sidebar section Collapse / Move / Reorder | Add section targets and keyboard behavior; folders are distinct from branch targets. |
| Additional keyboard menu anchors | Define focus/selection models for sidebar, file rows, and read-only content before adding a general keyboard opener. |

Existing-branch checkout, deletion, reset, rebase, cherry-pick, discard,
workflow rerun/cancel, hunk operations, and column preferences require separate
product and operation support. They are not implied by the menu host.
