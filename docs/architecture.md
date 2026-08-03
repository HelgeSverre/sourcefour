# Sourcefour architecture

This document records the architectural rationale for Sourcefour. Normative implementation requirements are defined by the [Sourcefour v1 implementation specification](spec.md).

I would build this as a **read-optimized, shell-launched Git repository inspector**, rather than starting from the architecture of a general-purpose Git client and cutting features away.

```text
sourcefour [path]
        │
        ▼
 Repository session
 ├── GPUI application state
 ├── gix read backend
 ├── git subprocess backend
 ├── incremental history worker
 ├── pure commit-graph layout engine
 └── metadata watcher
```

The full implementation-ready specification is [spec.md](spec.md).

## Core architectural decisions

### 1. One invocation, one repository, one window

The launch contract should simply be:

```sh
sourcefour [PATH]
```

`PATH` defaults to the current working directory, so your alias remains:

```sh
alias s='sourcefour'
```

Sourcefour discovers upward from that directory, preserving both:

* the shared repository identity and common Git directory;
* the particular worktree from which it was opened.

There is no home screen, project database, recent-repository list, bookmark system, tab model, or daemon. Closing the repository closes the application.

That makes the product extremely focused:

> Press `s`; get the useful half of SourceTree immediately.

### 2. GPUI for presentation, but not for domain modelling

Use one top-level GPUI entity containing the application snapshot and interaction state. Do not make every commit, file, branch, or label its own entity.

A practical component split would be:

```text
SourcefourApp
├── TitleBar
├── Toolbar
├── Sidebar
│   ├── WorktreeSection
│   ├── LocalBranchSection
│   └── RemoteSection
├── HistoryView
│   ├── HistoryHeader
│   ├── VirtualizedCommitRows
│   └── CommitGraphOverlay
├── CommitDetailsPane
└── StatusBar
```

The commit list should use GPUI’s virtualized uniform-list mechanism, with fixed-height rows. GPUI combines retained application state with immediate-style element construction and provides custom `Element` implementations and `UniformList`, which fits this interface well. It is still pre-1.0, so the GPUI dependency should be pinned to an exact revision and isolated behind a small internal UI compatibility layer. ([GitHub][1])

### 3. Use `gix` for reading and the installed `git` binary for operations

For v1:

| Responsibility           | Backend                        |
| ------------------------ | ------------------------------ |
| Repository discovery     | `gix`                          |
| Worktrees                | `gix`, with porcelain fallback |
| Local and remote refs    | `gix`                          |
| Commit object decoding   | `gix`                          |
| Revision traversal       | `gix`                          |
| Trees and changed files  | `gix`                          |
| Diff generation          | `gix`                          |
| Fetch                    | `git` subprocess               |
| Branch creation/checkout | `git` subprocess, optional v1  |
| Pull, push, merge, stash | Out of scope                   |

This hybrid boundary is deliberate. The read path benefits from an in-process object database, caches, commit graph access, and direct data structures. Network operations must deal with credential helpers, SSH configuration, proxy settings, remote helpers, and existing user configuration. Letting the installed Git executable perform those operations gives Sourcefour the same operational environment as the user’s terminal. `gix` does support fetching, but its receive path is blocking and introduces additional transport and credential integration concerns. ([Docs.rs][2])

Execute Git directly without a shell:

```rust
Command::new("git")
    .arg("-C")
    .arg(&active_worktree)
    .args(["fetch", "--all", "--porcelain", "--progress"]);
```

This avoids quoting problems and permits streamed progress and cancellation.

### 4. Never put the repository behind one global mutex

Store a cheap, cloneable `gix::ThreadSafeRepository` in the repository session. Each background worker converts it into its own thread-local repository handle.

`gix::Repository` is `Send` but not `Sync`, while `ThreadSafeRepository` is the shareable container intended for this handoff. ([Docs.rs][3])

Use two kinds of workers:

* A **long-lived history worker** owning the revision cursor, object cache, commit-graph access, decoded-object buffers, and graph continuation state.
* A small pool of **point-read workers** for commit details, changed-file lists, and diffs.

The history worker matters because throwing it away after every batch would also throw away the caches that make subsequent batches cheap.

### 5. Version every background result

Every asynchronous request should carry:

```rust
struct RequestContext {
    session_id: RepoSessionId,
    generation: u64,
    request_id: RequestId,
}
```

The UI ignores results belonging to an old generation.

Increment the generation when:

* the repository is reopened;
* active worktree context changes;
* history scope changes;
* refs are refreshed after fetch;
* the filter changes in a way that starts a new backend query.

This prevents a slow diff, old history batch, or fetch-triggered refresh from replacing newer state.

### 6. Progressive history, never full-history loading

Initial sequence:

1. Open the window.
2. Read repository identity, HEAD, worktrees, and refs.
3. Start a topological/date-ordered revision walk.
4. Decode the first 256 commits.
5. Render immediately.
6. Load later batches of roughly 512 as the viewport approaches the loaded boundary.

`gix` exposes revision traversal and common-directory/worktree information, and its traversal crates provide topological and date-order variants comparable to Git’s history ordering. ([Docs.rs][4])

Default history roots should be the unique tips of:

* active HEAD;
* local branches;
* remote-tracking branches;
* tags.

Selecting a branch changes the history scope to that ref. Selecting it again returns to all-refs history.

### 7. Make the graph engine an independent pure-Rust crate

Do not intertwine lane assignment with GPUI rendering.

Input:

```rust
struct GraphCommit {
    id: ObjectId,
    parents: SmallVec<[ObjectId; 2]>,
}
```

Output:

```rust
struct GraphRow {
    node_lane: u16,
    incoming: SmallVec<[EdgeSegment; 4]>,
    outgoing: SmallVec<[EdgeSegment; 4]>,
}
```

The engine must retain lane state between batches. Otherwise, loading the next 512 commits could cause all prior lane assignments to move, producing visible graph jumps.

The rendering layer then paints only the graph geometry intersecting the visible row range. Commit rows and graph geometry share the exact same fixed `30px` row coordinate system.

### 8. Worktrees are contexts, not separate repositories

The sidebar should include the main worktree and every linked worktree, including:

* path;
* checked-out branch;
* detached state;
* current invocation marker;
* locked state;
* stale or inaccessible state.

A worktree click changes the **active worktree context** used for HEAD, future status information, branch operations, and Git command working directory. Shared refs, objects, and repository history remain available.

Git provides a stable `--porcelain -z` worktree-listing format, which is useful as a compatibility or fixture-testing fallback. `gix` also exposes linked-worktree proxies and common-directory information. ([Git][5])

### 9. Treat the mockup as an executable visual contract

Your HTML already establishes concrete geometry rather than merely a vague visual direction:

* `1280×800` initial window;
* `236px` sidebar;
* `30px` history rows;
* `76px` graph gutter;
* `268px` details pane;
* five-column history layout;
* explicit dark palette and typography tokens.     

I would copy those values into a central Rust theme module and consider deviations to be deliberate design changes.

Visual testing should render deterministic fixtures at:

* `1280×800`;
* `1440×900`;
* narrow minimum width;
* long branch names;
* multiple graph lanes;
* detached HEAD;
* empty repository;
* many worktrees;
* selected commit with long body and many files.

Store screenshots and compare them using a perceptual threshold. That gives a coding agent an objective definition of “looks like the mockup.”

## Suggested workspace

```text
sourcefour/
├── Cargo.toml
├── apps/
│   └── sourcefour/
│       └── src/
│           ├── main.rs
│           ├── app.rs
│           ├── actions.rs
│           ├── commands.rs
│           ├── theme.rs
│           ├── window.rs
│           └── views/
├── crates/
│   ├── sourcefour-model/
│   ├── sourcefour-git/
│   ├── sourcefour-graph/
│   └── sourcefour-test-support/
├── fixtures/
└── tests/
    └── visual/
```

The app crate should not directly decode Git objects. The Git crate should not know GPUI exists. The graph crate should not know either exists.

## Initial performance gates

These are good engineering targets for an Apple Silicon reference machine:

| Operation                         |                           Target |
| --------------------------------- | -------------------------------: |
| Warm process start to first frame |                 p50 under 100 ms |
| Repository discovery and metadata |                  p50 under 30 ms |
| First 256 commits                 |                  p50 under 50 ms |
| Later 512-commit batch            |                 under 35 ms warm |
| Filter 5,000 loaded rows          |                      under 16 ms |
| Selection highlight response      |                       under 8 ms |
| Normal commit changed-file list   |                 p50 under 100 ms |
| Scrolling                         | no sustained frame above 16.7 ms |

The status bar in the mockup is useful precisely because it can expose development measurements such as:

```text
256 commits · first batch in 19 ms · gix
```

Keep that behind a debug/performance preference later, but leave it visible during implementation.

## Implementation order

1. **Window and static visual shell**
2. **Repository discovery and metadata**
3. **Progressive history list**
4. **Incremental commit graph**
5. **Commit details, file lists, and lazy diffs**
6. **Refresh watcher and fetch**
7. **Keyboard navigation, accessibility, benchmarks, and visual regression**
8. **Optional branch creation and checkout**

The [implementation specification](spec.md) expands all of this into state types, command/event contracts, backend traits, graph rules, error states, testing fixtures, milestone acceptance criteria, and a coding-agent handoff checklist.

[1]: https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md "https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md"
[2]: https://docs.rs/gix/latest/gix/remote/fetch/struct.Prepare.html "https://docs.rs/gix/latest/gix/remote/fetch/struct.Prepare.html"
[3]: https://docs.rs/gix/latest/gix/struct.ThreadSafeRepository.html "https://docs.rs/gix/latest/gix/struct.ThreadSafeRepository.html"
[4]: https://docs.rs/gix/latest/gix/struct.Repository.html "https://docs.rs/gix/latest/gix/struct.Repository.html"
[5]: https://git-scm.com/docs/git-worktree.html "https://git-scm.com/docs/git-worktree.html"
