# Sourcefour v1 — Architecture and Implementation Specification

**Status:** implementation-ready draft  
**Target:** Rust + GPUI desktop application  
**Primary platform:** macOS; Linux supported where GPUI permits  
**Product shape:** a single-repository Git history browser launched from the shell  
**Working name:** `sourcefour`

---

## 1. Executive decision

Build Sourcefour as a **single-window, single-repository, read-optimized Git client** with:

- **GPUI** for the native GPU-rendered interface.
- **gix/gitoxide** for repository discovery, references, worktrees, commit objects, history traversal, and tree diffs.
- The installed **`git` executable** for network and mutation operations in v1, beginning with `fetch` and optionally branch creation/checkout.
- A **pure Rust graph-layout crate** that incrementally converts a stream of commits and parent IDs into stable lane geometry.
- A **small unidirectional application state model**: UI actions produce commands, background work produces versioned events, and the UI only applies events belonging to the current repository generation.
- **Virtualized fixed-height history rows**, with the graph painted only for the visible range.
- **Progressive loading**: metadata and the first history batch appear quickly; further history, ahead/behind counts, commit details, file lists, and diffs load lazily.

Do not build bookmarks, a repository browser, tabs, staging, commit composition, pull, push, merge, stash, interactive rebase, or a project database in v1.

The result should feel like “press `s` inside a repository and immediately get the useful half of SourceTree.”

---

## 2. Product definition

### 2.1 Core user story

From any directory inside a Git repository or linked worktree:

```sh
s
```

opens Sourcefour focused on that repository. The window shows:

- all repository worktrees and their filesystem locations;
- local branches;
- remote-tracking branches grouped by remote;
- a graphical commit history;
- reference labels on commits;
- selected commit metadata;
- changed files and a lazy unified diff;
- fetch status and compact performance/status information.

The application starts from the invocation path. It has no “home screen.” Closing the repository closes the application.

### 2.2 Launch contract

The binary interface is:

```text
sourcefour [PATH]
```

Rules:

1. `PATH` defaults to the process current directory.
2. If `PATH` is a file, use its parent directory as the discovery start.
3. Discover upward until a repository, bare repository, submodule, or linked worktree is found.
4. Canonicalize enough to identify the repository, but preserve a display-friendly invocation/worktree path.
5. If opened from a linked worktree, mark that worktree as current.
6. If no repository is found, open a small error window or native alert and exit non-zero after dismissal.
7. Unknown command-line arguments are errors. Do not silently interpret them as revision specifications in v1.

Recommended shell setup:

```sh
alias s='sourcefour'
```

The default-current-directory behavior means the alias needs no `.` argument.

### 2.3 Product principles

1. **The first useful pixels win.** Do not wait for full history, all ahead/behind counts, or a status scan before showing the window.
2. **Read operations are native and incremental.** Avoid spawning `git log` for ordinary browsing.
3. **Mutation behavior follows Git.** In v1, delegate network and repository-changing actions to the installed Git executable rather than reimplementing every credential, hook, config, and edge-case behavior.
4. **The graph is data, not decoration.** History ordering and graph lane state must be deterministic and testable without GPUI.
5. **No row-per-entity architecture.** Thousands of commits remain plain data; the viewport renders only what is visible.
6. **No accidental project manager.** One process/window represents one invocation and one active worktree context.

---

## 3. Scope

### 3.1 Required v1 functionality

#### Repository/session

- Open from repository root, nested directory, `.git` directory, linked worktree, or bare repository.
- Display the repository name and active worktree path in the title bar.
- Refresh when refs or worktree metadata change externally.
- Preserve the selected commit across a safe refresh when its object still exists.

#### Worktrees

- List the main worktree and all linked worktrees.
- Show worktree display name, branch or detached state, and shortened filesystem path.
- Mark the invocation worktree as `CURRENT`.
- Show inaccessible, locked, or stale/prunable worktrees without crashing.
- Clicking a worktree changes the **active worktree context** used for HEAD, status, branch actions, and command working directory. Shared repository history remains available because objects and shared refs live in the common repository.
- Do not automatically launch a second process when switching worktrees.

#### Branches and remotes

- List local branches, current branch, upstream association, and lazily computed ahead/behind counts.
- Group remote-tracking branches by remote.
- Show remote URL in truncated secondary text.
- Omit symbolic aliases such as `origin/HEAD` from the ordinary remote branch list; they may be used to mark a remote’s default branch.
- Clicking a branch or remote branch changes the history scope to that ref.
- Clicking the already-selected scope returns to “all refs” history.

#### History

- Default scope: all unique tips from local branches, remote-tracking branches, tags, and active HEAD.
- Fixed row height of 30 logical pixels.
- Columns: graph, description, author, relative date, abbreviated hash.
- Labels: `HEAD`, local branches, remote branches, and tags.
- Merge commits visually de-emphasize the subject slightly, matching the prototype.
- Infinite/progressive loading rather than loading the entire repository.
- Keyboard selection with up/down arrows.
- Scrolling near the end requests another batch.
- Filtering searches loaded commits instantly by summary, author, and hash. A full-history indexed search is explicitly post-v1.

#### Commit detail

- Full object ID and copy action.
- Subject, body, author, author email, authored time, committed time, committer, and parent IDs.
- List changed paths and insertion/deletion counts.
- Lazy unified diff for the selected file.
- Binary files show a binary-change placeholder.
- Root commits diff against the empty tree.
- Merge commits default to diff against first parent, with a compact parent selector if implemented within schedule; otherwise clearly label “diff vs first parent.”

#### Toolbar/actions

- `Fetch` is functional.
- `Branch` may be included in v1 only after the read-only core is complete. At minimum it opens a branch creation/checkout dialog and delegates execution to Git.
- Pull, Push, Commit, Merge, and Stash remain visible but disabled only if visual fidelity benefits; their tooltip must say “Planned after v1.” Alternatively omit them from shipping builds and retain them in the visual test fixture.
- Filter commits field with `Cmd+F` / `Ctrl+F`.

### 3.2 Explicit non-goals

- Bookmarks, recent repositories, repository discovery UI, or project listing.
- Multiple repository tabs or windows managed by one process.
- Working-copy file browser.
- Stage/unstage, commit editor, amend, discard, reset, cherry-pick, rebase, or conflict resolution.
- Pull/push/merge/stash.
- Hosting-provider integrations.
- Commit signature verification.
- Blame.
- Submodule management.
- Full-text indexing of every historical commit.
- Windows support in v1.
- Pixel-perfect replication of SourceTree branding or proprietary assets. Reproduce structure and interaction, not protected artwork.

---

## 4. Visual and interaction specification

The supplied prototype is the visual source of truth. Translate its CSS tokens and dimensions into Rust constants and GPUI styles rather than “approximately recreating” them component by component.

### 4.1 Baseline geometry

Use these logical-pixel values as the 1× design baseline:

| Token | Value |
|---|---:|
| Initial window | 1280 × 800 |
| Minimum window | 900 × 560 |
| Title bar | 38 |
| Toolbar | 46 |
| Sidebar | 236 default, 180 min, 420 max |
| Column header | 26 |
| History row | 30 |
| Graph column | 76 minimum |
| Detail pane | 268 default, 140 min |
| Status bar | 26 |
| Corner radius | native window; 10 in screenshot fixture |

The main pane columns at the baseline width are:

```text
graph: 76
subject: flexible
content author: 148
date: 96
hash: 74
right padding: 14
```

Responsive collapse order below 1000 logical pixels:

1. Hide author column.
2. Hide hash column.
3. Keep date visible as long as possible.
4. Never shrink graph below the width required by visible lanes; permit horizontal graph/content scrolling if a pathological history requires it.

### 4.2 Theme tokens

Create a `Theme` struct with named semantic fields, initialized to the prototype values:

```rust
pub struct Theme {
    pub bg_page: Hsla,
    pub bg_chrome: Hsla,
    pub bg_panel: Hsla,
    pub bg_list: Hsla,
    pub bg_hover: Hsla,
    pub bg_selected: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_faint: Hsla,
    pub accent: Hsla,
    pub green: Hsla,
    pub orange: Hsla,
    pub purple: Hsla,
    pub red: Hsla,
    pub cyan: Hsla,
    pub graph_lanes: [Hsla; 6],
}
```

Keep all view code semantic: `theme.text_secondary`, never scattered hex literals.

Initial dark palette:

```text
page        #0b0c0f
chrome      #15171c
panel       #191c22
list        #1d2129
hover       #222734
selected    #263042
border      #262a33
border+     #2f3542
text        #dfe3ec
text2       #9aa2b3
text3       #5f677a
accent      #5b9dff
green       #7ec96f
orange      #e0a458
purple      #b392f0
red         #e5655e
cyan        #64c7d6
```

Do not add blur, transparency, gradients, or animated chrome in v1. The mockup’s strength is restraint and density.

### 4.3 Typography

- UI: system UI font (`-apple-system`/SF Pro on macOS, system sans elsewhere).
- Code/hash/path/diff: system monospace (`SF Mono`/Menlo on macOS).
- History subject: 12.5 px equivalent.
- Sidebar and row secondary text: 10–11.5 px.
- Section headings: uppercase, 10 px, 0.08 em letter spacing, weight 700.
- Render text with platform antialiasing; do not bundle Apple fonts.

### 4.4 Native title bar

On macOS use a transparent/custom-drawn title bar while keeping native traffic-light controls. Position controls to match the prototype and render the centered title:

```text
sourcefour — <repo-name> — <short-path>
```

The entire title bar background is draggable except interactive controls. Double-click follows the system title-bar behavior.

Linux may use client-side decorations or the platform title bar depending on GPUI support, but content geometry below it must remain consistent.

### 4.5 Sidebar behavior

Sections are always ordered:

1. Worktrees
2. Branches
3. Remotes

Section headers remain visible in natural scroll; sticky headers are optional.

#### Worktree row

Two lines:

```text
● sourcefour  CURRENT                         main
  ~/code/sourcefour
```

- Green filled dot: active/current worktree and accessible.
- Hollow faint dot: non-current accessible worktree.
- Warning glyph: inaccessible or prunable worktree.
- Lock glyph: locked worktree.
- The path is monospaced and middle-ellipsized when possible, preserving the leaf directory.
- Tooltip contains full path and any lock/prune reason.

#### Branch row

- Branch glyph, branch name, optional ahead/behind indicator.
- Current branch name is semibold.
- Ahead green; behind orange.
- Selected history scope gets selected background and accent left rule.
- Ahead/behind absence during lazy calculation is empty, not a spinner.

#### Remote group

- Remote name and URL on a group row.
- Branch children are indented.
- Groups may collapse only if it does not complicate v1; default expanded.

### 4.6 History list behavior

- Hover changes only background.
- Selection uses `bg_selected` plus a 2 px accent rule at the left edge of the main list.
- Selection follows keyboard navigation and remains visible.
- Up/down moves one row; PageUp/PageDown moves approximately one viewport; Home/End moves to first/last loaded row.
- `Enter` focuses the details pane; `Space` toggles details pane collapsed/expanded.
- Double-clicking a commit copies its short SHA only if this is documented in a tooltip; otherwise no double-click action in v1.
- Relative dates update at most once per minute, not every frame.

### 4.7 Filter behavior

`Cmd+F` focuses the filter field.

Matching fields:

- commit subject;
- author display name;
- short or full object ID;
- visible ref label.

Rules:

- Case-insensitive ASCII/simple Unicode lowercase matching is adequate in v1.
- Filtering applies only to loaded commits and displays a subtle “searching loaded history” label if the user could mistake it for repository-wide search.
- Escape clears the query; a second Escape returns focus to history.
- Graph geometry is recomputed for the filtered list. This is acceptable because the loaded set is bounded.
- No spinner for sub-50 ms filtering.

### 4.8 Detail pane

The detail pane is vertically resizable and may be collapsed. It contains:

1. Commit header and actions.
2. Subject/body.
3. Metadata grid.
4. Changed-file list.
5. Diff viewer for selected file.

The file list and diff are independent scroll regions only when necessary; prefer one detail-pane scroll region to avoid nested-scroll frustration.

A selection change immediately updates the header with already-known row data, then fills in metadata and files asynchronously. Never blank the entire pane while loading.

### 4.9 Status bar

Left side:

- active branch or detached SHA;
- ahead/behind for active branch when known;
- active worktree path;
- worktree count;
- local and remote branch counts.

Right side, debug/performance build and optionally release builds:

```text
256 commits · first batch 19 ms · gix
```

Do not turn this into a noisy telemetry ticker. It should update only when a meaningful operation completes.

---

## 5. Technical architecture

### 5.1 Workspace layout

```text
sourcefour/
├── Cargo.toml
├── Cargo.lock
├── apps/
│   └── sourcefour/
│       ├── Cargo.toml
│       ├── assets/
│       │   └── icons/
│       └── src/
│           ├── main.rs
│           ├── app.rs
│           ├── actions.rs
│           ├── state.rs
│           ├── theme.rs
│           ├── command_bus.rs
│           ├── views/
│           │   ├── root.rs
│           │   ├── titlebar.rs
│           │   ├── toolbar.rs
│           │   ├── sidebar.rs
│           │   ├── history.rs
│           │   ├── history_row.rs
│           │   ├── graph_element.rs
│           │   ├── details.rs
│           │   ├── diff_view.rs
│           │   └── statusbar.rs
│           └── platform/
│               ├── mod.rs
│               ├── macos.rs
│               └── linux.rs
├── crates/
│   ├── sourcefour-model/
│   │   └── src/lib.rs
│   ├── sourcefour-git/
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── discover.rs
│   │   │   ├── session.rs
│   │   │   ├── refs.rs
│   │   │   ├── worktrees.rs
│   │   │   ├── history.rs
│   │   │   ├── details.rs
│   │   │   ├── diff.rs
│   │   │   ├── operations.rs
│   │   │   ├── watch.rs
│   │   │   └── error.rs
│   │   └── tests/fixtures.rs
│   ├── sourcefour-graph/
│   │   ├── src/lib.rs
│   │   └── tests/snapshots/
│   └── sourcefour-test-support/
│       └── src/lib.rs
├── fixtures/
│   ├── linear.bundle
│   ├── merges.bundle
│   ├── octopus.bundle
│   ├── worktrees/
│   └── weird-refs.bundle
├── benches/
│   ├── history.rs
│   ├── graph.rs
│   └── diff.rs
└── docs/
    ├── architecture.md
    ├── performance.md
    └── adr/
        ├── 0001-gpui-pin.md
        ├── 0002-hybrid-git-backend.md
        └── 0003-history-order.md
```

Keep the number of crates low. The split is justified as follows:

- `sourcefour-model`: transport-safe domain snapshots with no GPUI or gix lifetimes.
- `sourcefour-git`: all repository access and command execution.
- `sourcefour-graph`: deterministic, pure layout code.
- `apps/sourcefour`: GPUI state, rendering, input, and orchestration.
- `sourcefour-test-support`: temporary repositories and reusable test builders.

### 5.2 Dependency policy

- Pin GPUI exactly. Never depend on `main`, `*`, or an unpinned Git branch.
- Prefer the current crates.io GPUI release. If a needed API exists only in the Zed repository, pin a full commit SHA and record the reason in ADR 0001.
- Pin gix to a tested minor release and let `Cargo.lock` fix the full graph.
- Keep `gix` feature selection intentional. Start with default features for implementation velocity, then benchmark a reduced feature set before release rather than guessing.
- Use `smallvec` for parent IDs and labels, `thiserror` for typed errors, `tracing` for structured diagnostics, and a small bounded LRU implementation for details/diff caches.
- Do not introduce Tokio solely for this application. Use GPUI’s executor plus blocking background tasks/threads where appropriate.

### 5.3 High-level component diagram

```text
┌──────────────────────────────── GPUI UI thread ───────────────────────────────┐
│                                                                              │
│  SourcefourWindow Entity                                                     │
│  ├─ ViewState: selection, focus, split sizes, filter, scroll handles          │
│  ├─ RepoSnapshot: worktrees, refs, remotes, labels                            │
│  ├─ HistoryState: rows, graph rows, cursor state, load status                 │
│  ├─ DetailState: commit detail, files, selected diff                          │
│  └─ OperationState: fetch/branch operation progress and errors                │
│          │ commands                                      ▲ versioned events    │
└──────────┼───────────────────────────────────────────────┼────────────────────┘
           ▼                                               │
┌──────────────────────────── orchestration ───────────────┴────────────────────┐
│ RepoController                                                                │
│ - owns RepoSessionId and monotonically increasing Generation                  │
│ - schedules jobs on GPUI/background workers                                   │
│ - cancels/invalidates stale jobs                                               │
│ - coalesces filesystem refreshes                                               │
└───────────────┬──────────────────────────────┬────────────────────────────────┘
                ▼                              ▼
┌──────────────────────────────┐  ┌─────────────────────────────────────────────┐
│ gix read workers             │  │ Git operation runner                        │
│ - metadata/ref snapshots     │  │ - `git fetch --porcelain --progress`        │
│ - history cursor             │  │ - branch create/checkout                    │
│ - commit detail and diff     │  │ - no shell, explicit argv and cwd           │
└──────────────────────────────┘  └─────────────────────────────────────────────┘
```

### 5.4 State ownership

The root GPUI entity owns UI-facing state. Background jobs must never hold a mutable reference to it.

```rust
pub struct SourcefourWindow {
    pub session_id: RepoSessionId,
    pub generation: Generation,

    pub repo: LoadState<RepoSnapshot>,
    pub history: HistoryState,
    pub details: DetailState,
    pub operations: OperationState,
    pub view: ViewState,

    pub history_scroll: UniformListScrollHandle,
    pub focus: FocusHandle,
    pub subscriptions: Vec<Subscription>,
}
```

Use plain structs and enums. Do not create a GPUI `Entity` for every sidebar row, commit, label, changed file, or diff line.

### 5.5 Command/event model

UI code emits typed commands:

```rust
pub enum RepoCommand {
    Open { path: PathBuf },
    RefreshMetadata { cause: RefreshCause },
    SetActiveWorktree { id: WorktreeId },
    SetHistoryScope { scope: HistoryScope },
    LoadNextHistoryBatch,
    LoadCommitDetail { oid: ObjectId },
    LoadCommitFiles { oid: ObjectId, parent: DiffParent },
    LoadFileDiff { oid: ObjectId, parent: DiffParent, path: RepoPath },
    Fetch { remote: FetchTarget },
    CreateBranch { name: String, start: ObjectId, checkout: bool },
}
```

Workers return events carrying identity and generation:

```rust
pub struct RepoEnvelope<T> {
    pub session: RepoSessionId,
    pub generation: Generation,
    pub payload: T,
}

pub enum RepoEvent {
    Opened(RepoSnapshot),
    MetadataRefreshed(RepoSnapshot),
    HistoryBatch(HistoryBatch),
    HistoryFinished,
    CommitDetailLoaded(CommitDetail),
    CommitFilesLoaded(CommitFiles),
    FileDiffLoaded(FileDiff),
    AheadBehindLoaded(Vec<AheadBehindUpdate>),
    OperationProgress(OperationProgress),
    OperationFinished(OperationOutcome),
    Failed(RepoFailure),
}
```

An event is ignored if its session or generation no longer matches. This is the primary cancellation mechanism for cheap reads. Long-running operations also receive an atomic cancellation flag.

### 5.6 Load-state model

Avoid a swarm of booleans.

```rust
pub enum LoadState<T> {
    Idle,
    Loading { started_at: Instant },
    Ready(T),
    Refreshing { current: T, started_at: Instant },
    Failed { error: UserFacingError, previous: Option<T> },
}
```

History uses a specialized state because it is incremental:

```rust
pub struct HistoryState {
    pub scope: HistoryScope,
    pub rows: Vec<CommitRow>,
    pub layout: Vec<GraphRow>,
    pub status: HistoryLoadStatus,
    pub request_in_flight: bool,
    pub has_more: bool,
    pub generation: u64,
    pub selected: Option<ObjectId>,
}
```

---

## 6. Git backend design

### 6.1 Hybrid backend rationale

Use gix for reads because the application needs direct object access, incremental traversal, reusable caches, and data structures that are independent of subprocess formatting.

Use the installed Git executable for fetch and v1 mutations because it naturally inherits the user’s credential helpers, SSH setup, URL rewriting, hooks, safe-directory behavior, and configuration semantics. A network fetch is already much larger than process startup overhead, so avoiding one subprocess provides little user-visible benefit while greatly increasing integration risk.

Represent both behind traits so a later all-gix backend remains possible:

```rust
pub trait RepoReader: Send + Sync {
    fn snapshot(&self, active_worktree: &WorktreeId) -> Result<RepoSnapshot>;
    fn start_history(&self, query: HistoryQuery) -> Result<Box<dyn HistoryCursor>>;
    fn commit_detail(&self, oid: ObjectId) -> Result<CommitDetail>;
    fn commit_files(&self, oid: ObjectId, parent: DiffParent) -> Result<CommitFiles>;
    fn file_diff(&self, request: FileDiffRequest) -> Result<FileDiff>;
}

pub trait RepoOperator: Send + Sync {
    fn fetch(&self, request: FetchRequest, sink: &dyn OperationSink) -> Result<OperationOutcome>;
    fn create_branch(&self, request: CreateBranchRequest) -> Result<OperationOutcome>;
}
```

### 6.2 Repository discovery and identity

Open with gix repository discovery from the supplied path. Store:

```rust
pub struct RepoLocation {
    pub invocation_path: PathBuf,
    pub active_worktree_path: Option<PathBuf>,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub is_bare: bool,
    pub kind: RepoKind,
}
```

`RepoSessionId` is a random ID for the current process session, not a hash of the path. `RepoIdentity` for refresh/caches may be derived from canonical `common_dir` plus device/inode where available.

Do not “re-open the common dir” and lose linked-worktree context. Retain both:

- a thread-safe repository representing the common/shared object and ref store;
- the selected worktree’s private git directory and worktree path.

### 6.3 Threading and gix handles

A gix `Repository` is thread-local rather than `Sync`; store a cheap cloneable `ThreadSafeRepository` in the session and create thread-local handles on background workers.

Use two categories of worker state:

1. **History worker** — a long-lived blocking task/thread that owns a thread-local repository, object cache, optional commit graph, traversal cursor, and graph continuation state.
2. **Point-read jobs** — commit details and diffs create a thread-local repository from the shared handle, configure an object cache, perform one request, and exit. A small worker pool prevents unbounded thread creation.

Do not use one global mutex around a single gix repository. It would serialize scrolling, details, and refreshes and would make cancellation poor.

### 6.4 Object caching

Configure caches only after measuring, but provide explicit hooks from the beginning:

```rust
pub struct GitPerformanceConfig {
    pub history_object_cache_bytes: usize, // start around 16–32 MiB
    pub detail_object_cache_bytes: usize,  // start around 8–16 MiB
    pub diff_cache_bytes: usize,           // application-level cap
}
```

Use `object_cache_size_if_unset`. Keep an application-level LRU for already-decoded `CommitDetail`, `CommitFiles`, and `FileDiff`; avoiding object access entirely is better than repeatedly relying on the ODB cache.

Cache keys include object IDs, selected parent, path, and relevant diff options.

### 6.5 Worktree enumeration

Build one `WorktreeSnapshot` for the main worktree, then append linked worktree proxies.

```rust
pub struct WorktreeSnapshot {
    pub id: WorktreeId,
    pub display_name: String,
    pub path: PathBuf,
    pub git_dir: PathBuf,
    pub head: HeadSnapshot,
    pub is_current: bool,
    pub is_main: bool,
    pub is_locked: bool,
    pub lock_reason: Option<String>,
    pub accessibility: WorktreeAccessibility,
}
```

For each linked worktree:

1. Read its base path from the proxy.
2. Open it with “possibly inaccessible worktree” semantics so HEAD can still be read if the checkout path is unavailable.
3. Read HEAD as branch, detached, or unborn.
4. Compare canonical/private git-dir identity to mark the invocation worktree current.
5. Never assume the worktree directory name equals its checked-out branch.

If gix worktree enumeration proves incomplete for a Git edge case, permit a narrowly isolated fallback parser for:

```sh
git worktree list --porcelain -z
```

The fallback belongs in `worktrees.rs`, is covered by fixtures, and does not leak subprocess output into UI code.

### 6.6 References, labels, branches, and remotes

Create one metadata pass that collects all relevant refs and builds a label index:

```rust
pub struct RefSnapshot {
    pub full_name: String,
    pub short_name: String,
    pub target: ObjectId,
    pub peeled_target: ObjectId,
    pub kind: RefKind,
    pub symbolic_target: Option<String>,
}

pub type LabelsByObject = HashMap<ObjectId, SmallVec<[RefLabel; 3]>>;
```

Rules:

- Local branches: `refs/heads/*`.
- Remote-tracking branches: `refs/remotes/<remote>/*`, excluding symbolic `<remote>/HEAD` from the normal child list.
- Tags: `refs/tags/*`; peel annotated tags so the label appears on the commit.
- `HEAD`: add a special high-priority label to its peeled commit.
- Stable label order: HEAD, current local branch, other local branches, remotes, tags; alphabetical within each group.

Remote URLs come from repository configuration. Display fetch URL; push URL is post-v1.

### 6.7 Ahead/behind calculation

Do not block first render on ahead/behind counts.

Initial branch snapshots contain the upstream ref and `AheadBehindState::Pending` where applicable. After metadata is shown:

1. Queue branches with valid local and upstream commit tips.
2. Compute ahead/behind with a reusable commit graph/merge-base structure where supported.
3. Limit work to one or a small number of background jobs.
4. Emit partial updates.
5. Cache by `(local_tip, upstream_tip)`.
6. Invalidate only when either tip changes.

Branches without upstream show no indicator. Unrelated histories show a subtle `↕`/tooltip rather than a fake numeric count.

### 6.8 History roots and ordering

#### Default all-refs scope

Roots are the unique peeled commit IDs of:

- active HEAD;
- all local branches;
- all remote-tracking branches;
- all tags.

Ignore refs that do not peel to commits. Deduplicate roots.

#### Selected branch scope

When a branch is selected, use that branch tip as the sole visible root. The default recommendation is to show only commits reachable from that root rather than keep unrelated rows dimmed. This is cheaper, more predictable, and makes branch selection useful on enormous repositories.

If preserving the prototype’s dimmed-unreachable treatment is important, implement it only for the currently loaded all-refs rows: compute reachability for loaded object IDs and dim non-members. Do not walk the entire repository merely to calculate dimming.

#### Ordering

Prefer topological date order equivalent to Git’s `--date-order`: no parent appears before its children, but recent commits remain prioritized. Implement with gix’s topological traversal plumbing if the pinned version supports it cleanly. If that API is too unstable, use the high-level gix revision walk’s topological breadth-first order and record the deviation in ADR 0003.

Do not silently sort decoded rows by timestamp after traversal; that breaks graph validity.

### 6.9 History cursor and batching

```rust
pub trait HistoryCursor: Send {
    fn next_batch(&mut self, max_rows: usize, cancel: &AtomicBool) -> Result<HistoryBatch>;
}

pub struct HistoryBatch {
    pub rows: Vec<CommitRow>,
    pub graph_rows: Vec<GraphRow>,
    pub has_more: bool,
    pub elapsed: Duration,
}
```

Batch policy:

- First batch: 256 rows.
- Subsequent batches: 512 rows.
- Request next batch when the viewport is within roughly 800 logical pixels or 30 rows of the loaded tail.
- Only one batch request per history generation may be in flight.
- Keep the traversal cursor alive; do not restart from roots and skip `N` commits for every page.
- Emit rows and graph layout together so the UI never has mismatched lengths.

`CommitRow` should contain only list-required data:

```rust
pub struct CommitRow {
    pub oid: ObjectId,
    pub parents: SmallVec<[ObjectId; 2]>,
    pub summary: String,
    pub author_name: String,
    pub commit_time: GitTime,
    pub labels: SmallVec<[RefLabel; 3]>,
    pub flags: CommitFlags,
}
```

Decode a commit once per traversal item and extract all row fields from that decode.

### 6.10 Commit details and files

Selection triggers detail loading after a short debounce of about 40–60 ms so rapid arrow-key navigation does not decode every passed commit.

Load detail and changed files separately:

- `CommitDetail`: cheap commit-object metadata.
- `CommitFiles`: tree-to-tree change summary, potentially more expensive.
- `FileDiff`: blob diff for one selected path, most expensive.

The header can be populated from `CommitRow` immediately. The metadata request may replace/fill exact values.

Changed-file records:

```rust
pub struct ChangedFile {
    pub old_path: Option<RepoPath>,
    pub new_path: Option<RepoPath>,
    pub status: ChangeKind,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub is_binary: bool,
}
```

If computing exact line statistics for every file materially delays the file list, emit path/status first and line counts as a second update. Measure before adding this complexity.

### 6.11 Diff generation

Use gix tree changes and blob diff support for read-only diffs.

Defaults:

- Unified context: 3 lines.
- No syntax highlighting in v1.
- Respect text/binary detection.
- Limit rendered diff to configurable safety thresholds, e.g. 20,000 lines or 5 MiB of formatted text.
- Show “Diff too large — open with external tool” rather than freezing.
- Rename detection may be disabled initially if it causes blob-heavy latency; present add/delete until performance is proven.
- Normalize line endings only for display, never mutate content.

The diff view should virtualize lines if large diffs are common. A simple scroll container is acceptable for the initial milestone with the safety cap.

### 6.12 Fetch operation

Run without a shell:

```text
git -C <active-worktree> fetch --all --prune --porcelain --progress
```

Whether `--prune` is default should be configurable; recommended v1 default is **off** unless the user explicitly chooses “Fetch and prune.” Safer default command:

```text
git -C <active-worktree> fetch --all --porcelain --progress
```

Requirements:

- Spawn on a blocking background task.
- Capture stdout and stderr incrementally.
- Parse porcelain stdout for ref update outcomes.
- Treat stderr progress as human-readable operation progress, rate-limited before UI updates.
- Provide cancellation by terminating the child process, then waiting/reaping it.
- Disable a second fetch while one is active.
- On success or partial success, refresh refs, branches, labels, active HEAD, and history roots.
- Preserve the selected commit if possible.
- On auth that requires a terminal prompt, allow Git’s configured credential helper/askpass path. If no graphical/askpass helper is available, show the error output and guidance rather than embedding a terminal in v1.

### 6.13 Branch create/checkout operation

Optional late-v1 feature:

- Dialog fields: branch name, start point (defaults to selected commit or HEAD), checkout checkbox.
- Validate branch name with Git-compatible check before enabling create.
- Execute one of:

```text
git -C <active-worktree> branch <name> <start>
git -C <active-worktree> switch -c <name> <start>
```

- Never construct a command string.
- If the branch is checked out in another worktree, surface Git’s exact error clearly.
- Refresh metadata/history after success.

### 6.14 Filesystem watching and refresh

Watch only metadata paths necessary for the visible model:

- active/private `HEAD`;
- common `refs/`;
- `packed-refs`;
- common `worktrees/`;
- `config` and optionally `config.worktree`;
- active index only if worktree dirty state is shown;
- `FETCH_HEAD` as an operation hint, not source of truth.

Coalesce events for 75–150 ms. A refresh re-reads metadata and compares snapshots before causing a render.

Do not watch the entire working directory in v1. The product is a history browser, and recursive worktree watching can be expensive in large repositories.

After a self-initiated operation, perform an explicit refresh; filesystem events are backup, not the operation completion mechanism.

---

## 7. Commit graph architecture

### 7.1 Requirements

The graph layout must be:

- deterministic;
- independent of GPUI;
- incremental across history batches;
- valid for merges, convergences, octopus merges, multiple roots, shallow boundaries, and filtered loaded data;
- compact but stable enough that loading the next batch does not move earlier rows;
- cheap enough to process hundreds of rows in substantially less than one frame budget.

### 7.2 Model

```rust
pub struct GraphState {
    lanes: Vec<Option<ObjectId>>,
    colors: HashMap<GraphLineId, u8>,
    next_line_id: u64,
}

pub struct GraphRow {
    pub node_lane: u16,
    pub node_color: u8,
    pub segments: SmallVec<[GraphSegment; 4]>,
    pub flags: GraphFlags,
}

pub enum GraphSegment {
    Vertical { lane: u16, color: u8 },
    Fork { from: u16, to: u16, color: u8 },
    Merge { from: u16, to: u16, color: u8 },
    Terminate { lane: u16, color: u8 },
}
```

The row stores semantic segments, not absolute pixel coordinates. The GPUI graph element maps lanes and row positions to pixels.

### 7.3 Incremental lane algorithm

For each commit in valid traversal order:

1. Find the lane currently expecting this commit ID.
2. If no lane expects it, allocate the first free lane; if none exists, append a lane.
3. The commit node is drawn in that lane.
4. Replace that lane’s expected object with the first parent, or free it for a root.
5. For every additional parent:
   - reuse an existing lane already expecting that parent when present;
   - otherwise allocate a free/new lane and assign the parent;
   - emit a fork/merge curve from the node lane to the parent lane.
6. Remove duplicate lane expectations for the current commit.
7. Compact only trailing empty lanes. Never renumber live lanes within previously emitted rows.
8. Persist `GraphState` with the history cursor for the next batch.

Color belongs to a continuing graph line, not simply `lane % 6`; otherwise colors can change when a line shifts. Assign a color when a new line is created and carry it until termination. Avoid assigning the same color as immediate neighboring active lines when possible.

### 7.4 Rendering

Use a custom GPUI `Element` or `Canvas` that paints only the visible row range.

Geometry:

```text
lane x = left_padding + lane_index × 18
node y = row_top + row_height / 2
node radius = 4
stroke width = 2
HEAD halo radius = 7.5, stroke width 1, 55% opacity
```

Curves use cubic Bézier segments with vertical tangents. Antialias at device scale. Clip painting to the graph column viewport.

Do not create one SVG per row or one GPUI div per segment. Paint all visible segments in one graph element per frame.

### 7.5 Filtering and graph continuity

Filtering the loaded rows creates a derived list. A parent may no longer be present in the filtered list. Render a short continuation stub rather than drawing a curve to a nonexistent row. Recompute graph state from the start of the loaded filtered list; with a few thousand loaded rows this is cheap and deterministic.

### 7.6 Pathological width

If more than the visually supported lane count is active:

- graph width grows in 18 px increments;
- subject column shifts right;
- permit horizontal scrolling of history content only if necessary;
- cap visual lane count at a high safety value such as 128 and show a degradation marker rather than allocating unbounded geometry from malformed input.

---

## 8. GPUI implementation design

### 8.1 Why GPUI fits

Use GPUI at three levels:

- entities for coarse application state;
- declarative `div`-based layout for chrome, sidebar, rows, and details;
- low-level custom painting for the commit graph and, if needed, large diff content.

### 8.2 Root render tree

```text
Root
├── CustomTitlebar
├── Toolbar
├── Body (horizontal)
│   ├── Sidebar
│   ├── VerticalDivider
│   └── Main (vertical)
│       ├── HistoryColumnHeader
│       ├── HistoryViewport
│       ├── HorizontalDivider
│       └── DetailsPane
└── Statusbar
```

The root view computes responsive column visibility once per render from viewport width and split sizes.

### 8.3 History virtualization

Use GPUI’s `uniform_list`/`UniformListScrollHandle` because rows have a fixed 30 px height.

The list renderer receives a visible range and produces only those rows. The graph custom element receives the same range plus a small overscan. The graph and text rows share one scroll handle and must never be separate independently scrolling layers.

Pseudo-structure:

```rust
uniform_list(
    "history",
    row_count,
    move |visible_range, window, cx| {
        visible_range.map(|index| render_history_row(index, window, cx))
    },
)
.track_scroll(&self.history_scroll)
```

Exact API names may change with the pinned GPUI version; preserve the architectural intent.

### 8.4 Graph overlay

Render history row backgrounds/text and graph geometry as siblings inside one clipped viewport:

```text
relative viewport
├── uniform list rows
└── absolute graph canvas, pointer-events disabled
```

The graph canvas computes its vertical offset from the list scroll state. Alternatively, include the graph cell in each row but draw cross-row segments through a custom element spanning the viewport. Prefer the single overlay because it minimizes element count and produces smoother curves.

### 8.5 Focus and actions

Define GPUI actions instead of ad-hoc keydown matching:

```rust
actions!(sourcefour, [
    FocusFilter,
    ClearFilter,
    SelectNextCommit,
    SelectPreviousCommit,
    PageUp,
    PageDown,
    SelectFirstCommit,
    SelectLastLoadedCommit,
    ToggleDetails,
    Fetch,
    CopyCommitSha,
    Refresh,
]);
```

Bind macOS and Linux shortcuts in one keymap module. Views declare key contexts such as `History`, `FilterInput`, and `Details`.

### 8.6 Split panes

Sidebar width and detail height are in `ViewState` and update during drag. Clamp to specified min/max. Persist only in a tiny settings file after v1 core; memory-only sizing is acceptable for the first milestone.

Do not continuously trigger expensive repository work during a resize. Renders may occur, but data does not change.

### 8.7 Icons

Use original minimal line icons stored as SVG assets or painted paths. Keep stroke width, size, and optical alignment consistent. Do not import SourceTree icons.

### 8.8 Accessibility

At minimum:

- logical labels for toolbar buttons;
- focusable history list and filter input;
- selected-state semantics for commit, worktree, and branch rows;
- accessible full text where visual text is truncated;
- sufficient contrast for primary content;
- keyboard access to every functional v1 action.

---

## 9. Performance specification

### 9.1 Performance budgets

Measure on a documented Apple Silicon reference machine and a Linux reference machine. Treat cold filesystem caches separately from warm runs.

Target budgets for a representative repository with 50,000 commits, 100 branches, and a commit graph file:

| Metric | Target |
|---|---:|
| Process start to first window frame, warm | p50 < 100 ms, p95 < 180 ms |
| Repository discovery + initial metadata | p50 < 30 ms, p95 < 80 ms |
| First 256 history rows after open | p50 < 50 ms, p95 < 150 ms |
| History batch of 512 | p50 < 35 ms warm |
| Filter loaded 5,000 rows | < 16 ms |
| Arrow selection to immediate header update | < 8 ms |
| Commit detail metadata | p50 < 40 ms warm |
| File list for ordinary commit | p50 < 100 ms warm |
| Scroll rendering | no sustained frame over 16.7 ms at 60 Hz |
| Idle CPU | approximately 0% absent watchers/timers |

These are engineering targets, not promises under cold network filesystems, giant pathological commits, or corrupted repositories.

### 9.2 Rules that protect the budgets

- Open the window before all secondary data is ready.
- Fixed-height virtualized rows.
- Graph paints only visible range plus overscan.
- History cursor remains alive across batches.
- No full history retained solely for filtering; retain only loaded rows.
- Ahead/behind is lazy.
- Details/diff is lazy and cancellable by generation.
- No recursive worktree watcher.
- No per-row entities or subscriptions.
- No allocation-heavy formatting on every frame; precompute display strings when data arrives.
- Relative dates are cached and periodically refreshed in one pass.
- No logging at debug/trace level in release builds unless enabled.

### 9.3 Instrumentation

Use `tracing` spans around:

- startup;
- repository discovery;
- metadata snapshot;
- worktree enumeration;
- ref scan;
- first history batch and subsequent batches;
- graph layout;
- commit detail;
- file summary;
- diff generation;
- fetch.

Expose a `SOURCEFOUR_PROFILE=1` mode that writes Chrome/Perfetto-compatible or structured timing output if practical. At minimum, write newline-delimited JSON spans to a temporary file.

### 9.4 Benchmarks

Create Criterion or custom wall-clock benchmarks for:

- graph layout: linear, merge-heavy, octopus, 100k rows;
- history decode: warm and cold-ish fixture runs;
- label-index construction with 10k refs;
- ahead/behind across many branches;
- tree diff and line-stat calculation;
- filter across 1k/5k/20k loaded rows.

Performance regressions greater than 15% in stable fixtures require explicit review.

---

## 10. Domain model

Use owned, UI-safe domain types. Do not expose gix references or borrowed byte strings outside `sourcefour-git`.

```rust
pub type Generation = u64;

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct RepoSessionId(uuid::Uuid);

#[derive(Clone, Eq, PartialEq, Hash)]
pub struct WorktreeId(pub String);

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct ObjectId(pub gix_hash::ObjectId);

pub struct RepoSnapshot {
    pub location: RepoLocation,
    pub display_name: String,
    pub active_worktree: WorktreeId,
    pub worktrees: Vec<WorktreeSnapshot>,
    pub local_branches: Vec<BranchSnapshot>,
    pub remotes: Vec<RemoteSnapshot>,
    pub tags_count: usize,
    pub labels_by_object: Arc<LabelsByObject>,
    pub head: HeadSnapshot,
    pub refreshed_at: Instant,
}

pub enum HeadSnapshot {
    Branch { full_name: String, short_name: String, oid: ObjectId },
    Detached { oid: ObjectId },
    Unborn { intended_branch: Option<String> },
    Missing,
}

pub struct BranchSnapshot {
    pub full_name: String,
    pub short_name: String,
    pub tip: ObjectId,
    pub is_current: bool,
    pub upstream: Option<UpstreamSnapshot>,
    pub ahead_behind: AheadBehindState,
    pub checked_out_in: Option<WorktreeId>,
}

pub enum HistoryScope {
    AllRefs,
    Ref { full_name: String, tip: ObjectId },
}
```

Git paths and names are byte sequences. Convert to display strings lossily at the backend boundary, but preserve raw path bytes internally where needed for diff lookup and command execution. On Unix, use `OsString`/byte-safe path types rather than round-tripping through UTF-8 strings.

---

## 11. Error handling and safety

### 11.1 Error classes

```rust
pub enum RepoFailureKind {
    NotARepository,
    PermissionDenied,
    UntrustedRepository,
    CorruptRepository,
    MissingObject,
    UnsupportedHash,
    InaccessibleWorktree,
    OperationConflict,
    Authentication,
    Network,
    Cancelled,
    Internal,
}
```

Each failure has:

- concise user-facing title;
- actionable message;
- optional technical details/cause chain;
- retryability;
- operation/session/generation context.

### 11.2 Degraded states

The application should still open when possible:

- A broken remote URL does not prevent local history.
- One inaccessible worktree does not hide the others.
- Ahead/behind failure leaves branch list usable.
- A missing blob makes one diff fail, not the window.
- A corrupt object encountered deep in history ends loading with a visible error row after already loaded commits.
- An unborn repository shows a clean empty-history state.
- A bare repository shows history and refs but disables worktree-dependent actions.

### 11.3 Command safety

- Never pass repository-controlled strings through a shell.
- Use `Command` with explicit arguments.
- Use `--` before path arguments where supported.
- Do not execute hooks or filters during read-only browsing.
- Respect gix trust semantics; never opt into executing repository-defined programs merely to display history.
- Git operations are user-initiated and may invoke their configured helpers/hooks as normal Git behavior. Make this boundary explicit in code and ADR 0002.
- Escape all commit messages, author names, ref names, paths, and diff text as text elements; they are never markup.

---

## 12. Testing strategy

### 12.1 Pure graph tests

Snapshot the semantic `GraphRow` output for:

- linear history;
- simple branch and merge;
- nested merges;
- criss-cross merge;
- octopus merge;
- multiple disconnected roots;
- duplicate roots;
- shallow boundary;
- batch split immediately before/after merge;
- filtering that removes parents;
- 100+ simultaneous lanes.

Assert determinism by running the same input repeatedly and comparing serialized output bytes.

### 12.2 Git backend integration fixtures

Build temporary repositories using the Git executable during test setup or restore bundle fixtures. Cover:

- ordinary repository;
- repository opened from nested directory;
- main plus multiple linked worktrees;
- detached linked worktree;
- locked worktree;
- inaccessible/stale worktree;
- branch checked out in another worktree;
- packed refs;
- annotated and lightweight tags;
- remote symbolic HEAD;
- unusual valid branch names;
- unborn repository;
- bare repository;
- shallow repository;
- replace refs if supported;
- SHA-256 repository if the pinned gix configuration supports it;
- invalid UTF-8 path/name on Unix;
- large commit message and binary file.

Compare selected backend results against canonical Git commands in tests, not in production.

### 12.3 UI tests

Use GPUI test support for:

- initial loading state;
- first history selection;
- keyboard navigation;
- filter focus/clear;
- sidebar branch scope selection;
- worktree context selection;
- details debounce and stale-response rejection;
- split-pane clamping;
- responsive column hiding;
- fetch action disabled while running;
- error banners and retry.

### 12.4 Visual regression

Create a deterministic demo repository fixture matching the supplied mockup’s shape. Render screenshots at:

- 1280 × 800 @1×;
- 1280 × 800 @2×;
- 1000 × 700;
- 900 × 560;
- selected/hover/focused states;
- long paths and long branch names;
- 8+ graph lanes;
- details collapsed and expanded.

Use a small pixel-difference tolerance for antialiasing. Visual regression is a release gate because fidelity is a primary requirement.

### 12.5 Operation tests

Wrap the Git executable in a fake/test runner and verify exact argv, cwd, cancellation, stdout/stderr parsing, and refresh behavior. Do not require network access in unit/CI tests.

---

## 13. Implementation plan

Each milestone must leave the application runnable and demonstrable.

### Milestone 0 — Workspace and pinned foundations

Deliver:

- workspace and crates;
- pinned GPUI and gix dependencies;
- ADRs 0001–0003;
- tracing setup;
- empty custom-titlebar window at correct geometry;
- CI for macOS and Linux compilation/tests;
- formatting, Clippy, and test commands.

Acceptance:

- `cargo run -p sourcefour -- .` opens a window.
- No wildcard or branch dependencies.
- Screenshot dimensions and base palette match the prototype shell.

### Milestone 1 — Repository discovery and static shell

Deliver:

- command-line path parsing;
- repository discovery;
- window title/repo path;
- static toolbar/sidebar/history/detail/status layout;
- no fake data in shipping path; an explicit `--demo` flag may load deterministic demo data.

Acceptance:

- Works from root, nested folder, and linked worktree.
- Clear non-repository error.
- Layout visual diff is within agreed tolerance.

### Milestone 2 — Metadata sidebar

Deliver:

- worktrees including main and current marker;
- local branches;
- remotes and remote branches;
- HEAD/ref/tag label index;
- metadata refresh watcher;
- lazy ahead/behind updates.

Acceptance:

- Fixtures pass for detached/unborn/locked/inaccessible worktrees.
- Sidebar never blocks first window on ahead/behind.
- External branch creation/deletion refreshes UI.

### Milestone 3 — Incremental history and graph

Deliver:

- history cursor;
- 256/512 batching;
- graph layout crate;
- virtualized GPUI list;
- custom visible-range graph painter;
- selection and keyboard navigation;
- branch scope.

Acceptance:

- 50k fixture starts progressively rather than loading all rows.
- Earlier graph rows do not move when next batch arrives.
- Scrolling remains within frame budget.
- Graph snapshots pass all merge cases.

### Milestone 4 — Filtering and details

Deliver:

- loaded-history filter;
- immediate detail header;
- debounced commit metadata;
- changed files;
- lazy file diff;
- copy SHA;
- detail splitter/collapse.

Acceptance:

- Rapid arrow navigation applies only latest detail response.
- Binary/large/root/merge commit cases degrade correctly.
- Filtering 5k loaded rows meets target.

### Milestone 5 — Fetch and operational refresh

Deliver:

- Git operation runner;
- fetch button/action;
- progress display;
- cancellation;
- post-fetch metadata/history refresh;
- robust operation errors.

Acceptance:

- Exact argv tested.
- No concurrent duplicate fetch.
- Selection preserved when object remains reachable/available.
- Auth/network errors do not destroy the browsing session.

### Milestone 6 — Fidelity, performance, packaging

Deliver:

- final visual regression suite;
- benchmarks and profiling notes;
- macOS app bundle/icon;
- Linux packaging notes/binary;
- release build stripping/LTO decision based on measured startup;
- README and `s` alias instructions.

Acceptance:

- Meets agreed p50/p95 budgets on reference hardware or documents measured exceptions.
- No known high-severity correctness issue on required fixtures.
- First release is useful as the author’s daily `s` command.

### Optional Milestone 7 — Branch creation

Only after Milestones 0–6 are stable.

---

## 14. Definition of done for v1

Sourcefour v1 is done when all of the following are true:

1. Running `sourcefour` inside a normal or linked-worktree repository opens directly to a useful history view.
2. There is no project/bookmark/recent-repository screen.
3. Worktrees and their paths are correct, including the main worktree and current invocation worktree.
4. Local and remote branches are grouped and selectable.
5. History loads incrementally, graph lanes are correct across batch boundaries, and the UI remains responsive on the large fixture.
6. The supplied 1280 × 800 visual fixture is recognizably matched in spacing, hierarchy, typography, palette, and density.
7. Commit details, changed files, and one-file diffs are lazy and cancellable/stale-safe.
8. Fetch uses the installed Git executable safely and refreshes the model after completion.
9. External ref/worktree changes are reflected without restarting.
10. Empty, unborn, bare, detached, shallow, inaccessible-worktree, corrupt-object, binary-diff, and huge-diff cases have intentional behavior.
11. No GPUI entity exists per commit row.
12. No full-history load is required for ordinary startup or scrolling.
13. Visual, graph, backend integration, and operation-runner tests pass in CI.
14. Performance measurements are stored in `docs/performance.md` with hardware and fixture details.

---

## 15. Coding-agent execution instructions

Give the coding agent this document, the HTML prototype, and the screenshot. Then instruct it to work milestone by milestone under these constraints:

1. Do not implement future toolbar operations opportunistically.
2. Do not replace GPUI with another framework.
3. Do not use `git log` as the production history backend unless an ADR demonstrates a measured blocker in the pinned gix version.
4. Do not make network calls in tests.
5. Do not add a project database, bookmarks, recent repositories, tabs, or global repository registry.
6. Keep gix types inside `sourcefour-git`; UI-facing types must be owned domain snapshots.
7. Every asynchronous result must carry session and generation identity.
8. Every potentially expensive operation must run off the GPUI UI thread.
9. Use `uniform_list` or equivalent fixed-row virtualization for history.
10. Graph layout must be pure and snapshot-tested before it is painted.
11. Match prototype constants centrally; do not eyeball per component.
12. After every milestone:
    - run unit and integration tests;
    - capture the visual fixture;
    - run the relevant benchmark;
    - update `docs/performance.md`;
    - make one cohesive commit.
13. When an API differs from this spec because the pinned GPUI/gix version changed, preserve the architecture and document the exact deviation in an ADR rather than silently redesigning.
14. Prefer a complete, tested vertical slice over broad scaffolding.

Suggested first coding-agent task:

```text
Implement Milestone 0 only. Create the Rust workspace and an executable GPUI app with the exact base window geometry, transparent native macOS title bar, centralized dark theme tokens, tracing initialization, pinned dependencies, CI, and ADRs 0001–0003. Do not implement repository access or populate fake production data. Add a --demo mode only if needed for a deterministic screenshot fixture. Run all tests and provide the screenshot plus measured process-to-first-frame timing.
```

---

## 16. Recommended defaults and deliberate decisions

### 16.1 Read with gix; operate with Git

This is the strongest tradeoff for v1. It keeps browsing fast and native while avoiding an unnecessary authentication/configuration project before the core utility is useful.

### 16.2 One active worktree context, one common history store

A linked worktree has private HEAD/index state but shares objects and most refs. Model both explicitly. Clicking a worktree changes command/status context without pretending it is a separate project.

### 16.3 Branch selection narrows history

The default recommendation is to walk from the selected branch tip rather than show all rows and dim unreachable history. It scales better and provides a clearer mental model. The prototype’s dimming can be added for loaded rows as a visual mode later.

### 16.4 Details are progressively enhanced

On selection, show known row data immediately. Then load exact metadata, files, and diff in stages. The user should never see the entire pane flash into a loading skeleton while arrowing through commits.

### 16.5 Visual fidelity is tested, not discussed

The HTML prototype becomes a token/geometry source and the deterministic screenshot fixture becomes a regression test. Without that, repeated “close enough” changes will slowly turn the application into a generic dark Rust UI.

---

## 17. Post-v1 roadmap

Ordered by likely value:

1. Working-tree status summary and changed-file view.
2. Branch creation/checkout if not included in v1.
3. Fetch one remote and fetch/prune options.
4. Open selected worktree in terminal/editor.
5. Commit search across unloaded history.
6. Tags in sidebar.
7. Pull and push through the Git operation backend.
8. Commit/staging workflow.
9. Merge/cherry-pick/rebase operations.
10. Optional recent repositories/bookmarks — only if the product intentionally stops being the `s` command replacement.

Do not let roadmap items distort the v1 state model. The command/event architecture and `RepoOperator` abstraction are sufficient extension points.

---

## 18. Research notes for implementers

At the time this specification was drafted:

- GPUI is a hybrid immediate/retained GPU-accelerated Rust UI framework, with entities, declarative views, custom low-level elements, an integrated executor, and a uniform-list primitive suitable for fixed-height virtualization. It is pre-1.0 and should be pinned exactly.
- GPUI window options support a transparent/custom title bar and macOS traffic-light positioning.
- gix exposes repository discovery, shared/thread-local repository handles, refs, worktrees, revision traversal, commit graphs, object caches, status, and tree/blob diffs. Its thread-safe repository is intended to be converted to thread-local handles on workers.
- gix object caching is opt-in/configurable and should be benchmarked rather than enabled blindly.
- Git documents `git worktree list --porcelain -z` as a stable machine-readable fallback format.
- Git fetch provides porcelain output for machine parsing; progress is reported separately.

Primary references:

- GPUI crate documentation: https://docs.rs/gpui/latest/gpui/
- GPUI source/README in Zed: https://github.com/zed-industries/zed/tree/main/crates/gpui
- gix crate documentation: https://docs.rs/gix/latest/gix/
- gix-traverse topological walker: https://docs.rs/gix-traverse/latest/gix_traverse/commit/
- Git worktree manual: https://git-scm.com/docs/git-worktree
- Git fetch manual: https://git-scm.com/docs/git-fetch

