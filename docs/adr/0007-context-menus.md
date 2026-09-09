# ADR 0007: Shared context menus across application surfaces

## Status

Accepted and implemented, 2026-09-09. The API was checked against the pinned
GPUI revision, `d637307bd75389f63adbb7d86bc66174ee817d0d`. See the
[surface inventory](../context-menu-surfaces.md) for coverage and follow-ups.

## Decision

Use one application-local `MenuHost` per window. Build entries only when a
context gesture occurs. Keep commands with the existing view, editor, or
operation controller; the host handles presentation, focus, navigation,
invocation, and dismissal without understanding Git or editor commands.

Both `SourcefourWindow` and the repository picker's `ErrorWindow` own a host.
Inputs and overlays use that host through a weak entity handle. The host paints
last at the root, above dialogs, without adding per-row views or subscriptions.
There is no new dependency or GPUI upgrade.

## Why this boundary

History, changed files, diff lines, and Actions logs are virtualized. Their
visible indices and elements are transient. Commands must capture semantic
identity: full `Oid`, ref name, repository session, comparison parent, staged
side, native path, job ID, or immutable text plus range. Right-clicking commit
B while A is selected must act on B without loading B's details or files.

The pinned GPUI provides anchoring, deferred painting, occlusion, focus handles,
and event subscriptions. Its `Menu`/`MenuItem` API describes application menus,
not a portable popup-at-pointer interface. A native popup would require a new
platform boundary. Zed's higher-level menu lives outside core GPUI and depends
on its UI infrastructure. Adopting a component suite would also require a
separate compatibility and theming decision. The small local host fits the
existing app and can use its GPUI test harness.

## Implementation

[`context_menu.rs`](../../apps/sourcefour/src/context_menu.rs) contains:

- `MenuEntry`: an enabled callback, disabled label, or separator.
  `MenuEntry::command` binds a callback to a weak typed owner. Copy and link
  helpers cover commands that need no view state.
- `MenuHost`: one optional popup, its highlighted entry and pointer anchor,
  previous focus, a serial number, theme, and three lifetime subscriptions.
- `ContextMenuExt`: right-click and macOS Control-click recognition, consuming
  the opening press so the deepest meaningful target wins.
- `PrimaryClickExt`: guards the matching release. GPUI records a pending normal
  click for Control-left-press even when the press is consumed; every app click
  handler uses the guard to prevent incidental activation.
- The app-wide clipboard sequence: synchronous Copy and Cut advance it;
  background log copies write only if their ticket is still current.

[`views/menus.rs`](../../apps/sourcefour/src/views/menus.rs) builds commit, ref,
remote, file, and section entries from cached state. Other adapters stay beside
their owners: TextInput editing, diff coordinates, Markdown blocks, Actions
logs, and settings links. An adapter adds a gesture handler carrying a small
target; it constructs the entry vector inside that handler.

Entries are dismissed before invocation. Invocation is deferred to the next
window update so a command can open a dialog or invalidate the host without a
reentrant entity borrow. Weak owners prevent callbacks retaining closed views.
Repository operations recheck their session and applicable target before using
the existing controllers. The branch dialog retains its start commit and
repository independently of later selection changes. Remote fetch shares the
toolbar's progress, cancellation, and mutual exclusion.

## Interaction and lifecycle

Menus support arrows, Home/End, Enter, Escape, and Tab. Disabled entries and
separators are skipped. Shift-F10 opens the selected visible commit's menu when
history owns focus, or the focused TextInput's edit menu. Other keys are consumed.
The containing root switches to `MenuRoot` while a menu is open, keeping
history/picker action bindings from running before the menu's raw key handler.

An outside primary press dismisses and is consumed. An outside context press
can replace the popup with the newly clicked target. Replacement retains the
original focus destination. Dismissal restores it only while the host still
owns focus, so a new dialog or input cannot lose focus to a stale callback.
Serial checks protect delayed dismissal, highlighting, and invocation from
operating on a replacement.

Window bounds changes, deactivation, focus loss, and scroll gestures dismiss
the popup. Repository reloads, history/filter changes, selected-file changes,
and relevant overlay transitions invalidate it through the owner. Updating an
input invalidates only its own open menu. Unrelated CI updates need not close
a menu whose captured copy target remains valid.

GPUI-specific details matter here:

- The popup uses `anchored`, `snap_to_window_with_margin`, `deferred` priority
  100, and `occlude`. Clamp the origin's minimum coordinates as well as using
  overflow snapping, preserving an eight-pixel margin at all four corners.
- Register the window-wide scroll listener during paint via a canvas; GPUI
  rejects registration during element construction.
- Focus notifications arrive after paint. The host also checks current focus
  before rendering, preventing a frame with an already unfocused popup.

## Data and performance

Ref metadata now retains canonical full names during the existing metadata
pass. No Git query is added when opening a chip menu. Parent hashes carry their
actual IDs instead of reparsing display abbreviations. SHA-1 and SHA-256 copy
in full. Native filesystem paths preserve Git bytes; exact text-copy entries
are disabled for paths that cannot be represented as UTF-8.

Existing virtualized lists remain virtualized. Only visible targets gain
lightweight handlers; menus allocate no entry vectors when closed. Opening a
menu consults loaded state and performs no Git/network/file-content work.
Copy selections capture immutable source handles and ranges rather than
cloning complete documents or diffs.

Actions logs use `Arc<[String]>`; step ranges are cached per loaded log and
cleared when that data changes. Copy job/step log strips ANSI and assembles text
on a worker only after invocation. The existing footer button uses the same
path, removing whole-log joining from render. A later application Copy wins
over a pending background copy, and a closed owner cannot write its result.
The sequence does not monitor clipboard writes made by other applications.

## Verification and extension

The GPUI tests exercise real pointer/key dispatch, nested targets, Control-click,
focus restoration, outside-click consumption, window lifecycle, corner clamping,
keyboard isolation, picker editing, and captured branch/log targets. The suite
also renders and scrolls 100,000 changed files with menus available and copies
the final row. Detailed evidence and remaining native visual checks are in the
[surface inventory](../context-menu-surfaces.md#verification).

To add a menu, use the existing host, capture an immutable semantic target,
build entries on demand, and delegate operations to their owner. Add owner
invalidation when a new surface can disappear without moving focus or scrolling.
Do not introduce I/O just to populate a menu. Submenus, alternate destructive
commands, native accessibility integration, and additional keyboard anchors
need their own behavior and tests when introduced.
