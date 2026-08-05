//! Owned, transport-safe domain values shared by Sourcefour crates.
//!
//! This crate deliberately has no dependency on GPUI or gix. Repository-facing
//! crates convert borrowed backend values into these snapshots before crossing a
//! thread or UI boundary.

use std::{
    collections::BTreeMap,
    fmt,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use smallvec::SmallVec;
use thiserror::Error;
use uuid::Uuid;

/// A random identity for one repository session in one Sourcefour process.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct RepoSessionId(Uuid);

impl RepoSessionId {
    /// Creates a fresh session identity.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Returns the underlying UUID for diagnostic correlation.
    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for RepoSessionId {
    fn default() -> Self {
        Self::new()
    }
}

/// Monotonically increasing version of UI-visible repository state.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct Generation(pub u64);

/// Identity of an independently cancellable request within a session.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct RequestId(pub u64);

/// A background payload tagged with the state it was computed for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoEnvelope<T> {
    /// Repository session that owns the request.
    pub session: RepoSessionId,
    /// Repository generation that owns the request.
    pub generation: Generation,
    /// Request identity, useful for point reads and diagnostics.
    pub request: RequestId,
    /// Owned event payload.
    pub payload: T,
}

impl<T> RepoEnvelope<T> {
    /// Maps the payload while retaining request identity.
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> RepoEnvelope<U> {
        RepoEnvelope {
            session: self.session,
            generation: self.generation,
            request: self.request,
            payload: map(self.payload),
        }
    }
}

/// UI load state that retains useful stale content across refresh failures.
#[derive(Clone, Debug)]
pub enum LoadState<T> {
    /// No request has been started.
    Idle,
    /// The first request is running.
    Loading { started_at: Instant },
    /// A current value is available.
    Ready(T),
    /// A refresh is running while a prior value remains usable.
    Refreshing { current: T, started_at: Instant },
    /// A request failed, optionally preserving a prior value.
    Failed {
        /// User-displayable error.
        error: UserFacingError,
        /// Content that remains safe to display.
        previous: Option<T>,
    },
}

impl<T> LoadState<T> {
    /// Returns the current or stale value when one exists.
    #[must_use]
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Ready(value)
            | Self::Refreshing { current: value, .. }
            | Self::Failed {
                previous: Some(value),
                ..
            } => Some(value),
            Self::Idle | Self::Loading { .. } | Self::Failed { previous: None, .. } => None,
        }
    }

    /// Returns the current or stale value mutably, for in-place enrichment.
    ///
    /// Later-arriving detail such as ahead/behind counts is merged into the
    /// loaded snapshot rather than replacing it.
    #[must_use]
    pub fn value_mut(&mut self) -> Option<&mut T> {
        match self {
            Self::Ready(value)
            | Self::Refreshing { current: value, .. }
            | Self::Failed {
                previous: Some(value),
                ..
            } => Some(value),
            Self::Idle | Self::Loading { .. } | Self::Failed { previous: None, .. } => None,
        }
    }

    /// Reports whether a request is in flight.
    #[must_use]
    pub const fn is_loading(&self) -> bool {
        matches!(self, Self::Loading { .. } | Self::Refreshing { .. })
    }
}

/// Owned object identity supporting SHA-1 and SHA-256 repositories.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Oid {
    bytes: [u8; 32],
    length: u8,
}

impl Oid {
    /// Creates an object ID from a 20-byte SHA-1 digest.
    #[must_use]
    pub const fn sha1(bytes: [u8; 20]) -> Self {
        let mut storage = [0; 32];
        let mut index = 0;
        while index < 20 {
            storage[index] = bytes[index];
            index += 1;
        }
        Self {
            bytes: storage,
            length: 20,
        }
    }

    /// Creates an object ID from a SHA-256 digest.
    #[must_use]
    pub const fn sha256(bytes: [u8; 32]) -> Self {
        Self { bytes, length: 32 }
    }

    /// Parses a full 40- or 64-character hexadecimal object ID.
    ///
    /// # Errors
    ///
    /// Returns an error when the input is not a full SHA-1 or SHA-256 digest.
    pub fn from_hex(value: &str) -> Result<Self, OidParseError> {
        let byte_length = match value.len() {
            40 => 20,
            64 => 32,
            length => return Err(OidParseError::InvalidLength { length }),
        };
        let mut bytes = [0; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            let high = hex_value(pair[0]).ok_or(OidParseError::InvalidHex { index: index * 2 })?;
            let low = hex_value(pair[1]).ok_or(OidParseError::InvalidHex {
                index: index * 2 + 1,
            })?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self {
            bytes,
            length: byte_length,
        })
    }

    /// Returns the underlying digest bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    /// Returns an abbreviated hexadecimal object ID, never longer than the ID.
    #[must_use]
    pub fn abbreviated(&self, requested_length: usize) -> String {
        self.to_hex().chars().take(requested_length).collect()
    }

    /// Returns the full lowercase hexadecimal object ID.
    #[must_use]
    pub fn to_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(usize::from(self.length) * 2);
        for byte in self.as_bytes() {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        output
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Oid").field(&self.to_hex()).finish()
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

/// An invalid serialized object ID.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum OidParseError {
    /// The input does not contain a full supported object ID.
    #[error("object ID has unsupported hexadecimal length {length}; expected 40 or 64")]
    InvalidLength { length: usize },
    /// One character is not hexadecimal.
    #[error("object ID contains non-hexadecimal data at character {index}")]
    InvalidHex { index: usize },
}

/// Stable identity of a configured worktree.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct WorktreeId(pub String);

/// A byte-preserving Git path for diff lookup and display conversion.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct RepoPath(pub Vec<u8>);

impl RepoPath {
    /// Returns a lossy, display-safe representation.
    #[must_use]
    pub fn display_lossy(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }
}

/// Repository location and discovery context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoLocation {
    /// Path provided to the executable.
    pub invocation_path: PathBuf,
    /// Selected worktree path, omitted for bare repositories.
    pub active_worktree_path: Option<PathBuf>,
    /// Repository-specific Git directory.
    pub git_dir: PathBuf,
    /// Shared common Git directory.
    pub common_dir: PathBuf,
    /// Whether this repository has no working tree.
    pub is_bare: bool,
    /// Repository shape discovered by the backend.
    pub kind: RepoKind,
}

/// Supported repository shapes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepoKind {
    /// A normal main worktree.
    Worktree,
    /// A linked worktree with private HEAD/index state.
    LinkedWorktree,
    /// A bare repository.
    Bare,
}

/// Complete metadata snapshot for one active repository context.
#[derive(Clone, Debug)]
pub struct RepoSnapshot {
    /// Location and repository form.
    pub location: RepoLocation,
    /// Display title selected by the backend.
    pub display_name: String,
    /// Active worktree context, absent for bare repositories.
    pub active_worktree: Option<WorktreeId>,
    /// Main and linked worktrees.
    pub worktrees: Vec<WorktreeSnapshot>,
    /// Local branches.
    pub local_branches: Vec<BranchSnapshot>,
    /// Remotes and their non-symbolic tracking branches.
    pub remotes: Vec<RemoteSnapshot>,
    /// Number of tags known to the metadata scan.
    pub tags_count: usize,
    /// Ref labels grouped by peeled target object.
    pub labels_by_object: LabelsByObject,
    /// HEAD from the active context.
    pub head: HeadSnapshot,
    /// Instant at which the snapshot completed.
    pub refreshed_at: Instant,
}

/// Labels keyed by object ID. `BTreeMap` makes transport order deterministic.
pub type LabelsByObject = BTreeMap<Oid, SmallVec<[RefLabel; 3]>>;

/// Current HEAD state for a worktree or repository.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeadSnapshot {
    /// HEAD points at a local branch.
    Branch {
        /// Fully qualified ref name.
        full_name: String,
        /// User-facing short branch name.
        short_name: String,
        /// Branch tip.
        oid: Oid,
    },
    /// HEAD points directly at an object.
    Detached { oid: Oid },
    /// The repository has no commit yet.
    Unborn { intended_branch: Option<String> },
    /// HEAD could not be read.
    Missing,
}

/// Worktree metadata safe to show even when the path is unavailable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeSnapshot {
    /// Stable private Git-directory identity.
    pub id: WorktreeId,
    /// User-facing worktree name.
    pub display_name: String,
    /// Checkout location.
    pub path: PathBuf,
    /// Private Git directory.
    pub git_dir: PathBuf,
    /// Worktree HEAD state.
    pub head: HeadSnapshot,
    /// Whether this is the invocation worktree.
    pub is_current: bool,
    /// Whether this is the primary worktree.
    pub is_main: bool,
    /// Whether Git reports this worktree locked.
    pub is_locked: bool,
    /// Optional Git lock explanation.
    pub lock_reason: Option<String>,
    /// Accessibility state without attempting repair.
    pub accessibility: WorktreeAccessibility,
}

/// Availability of a worktree path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorktreeAccessibility {
    /// Path is readable.
    Accessible,
    /// Path cannot currently be read.
    Inaccessible { reason: String },
    /// Git marks this worktree as stale or prunable.
    Prunable { reason: Option<String> },
}

/// Local branch metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchSnapshot {
    /// Fully qualified ref name.
    pub full_name: String,
    /// User-facing branch name.
    pub short_name: String,
    /// Branch tip.
    pub tip: Oid,
    /// Whether this branch is active in the selected context.
    pub is_current: bool,
    /// Optional upstream relationship.
    pub upstream: Option<UpstreamSnapshot>,
    /// Lazily computed divergence state.
    pub ahead_behind: AheadBehindState,
    /// Worktree using this branch, if any.
    pub checked_out_in: Option<WorktreeId>,
}

/// A branch upstream target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamSnapshot {
    /// Fully qualified upstream ref name.
    pub full_name: String,
    /// User-facing upstream name.
    pub short_name: String,
    /// Upstream tip at snapshot time.
    pub tip: Oid,
}

/// Lazy branch divergence state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AheadBehindState {
    /// No calculation is applicable or configured.
    Unavailable,
    /// A calculation is queued or in progress.
    Pending,
    /// Divergence is known.
    Known { ahead: u32, behind: u32 },
    /// Histories cannot be compared meaningfully.
    Unrelated,
    /// Calculation failed while leaving the branch usable.
    Failed,
}

/// One configured remote and its visible tracking branches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteSnapshot {
    /// Remote name.
    pub name: String,
    /// Fetch URL suitable for display.
    pub fetch_url: Option<String>,
    /// Remote's advertised default branch, if known.
    pub default_branch: Option<String>,
    /// Ordinary remote-tracking branches; symbolic `HEAD` is excluded.
    pub branches: Vec<RemoteBranchSnapshot>,
}

/// Remote-tracking branch metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteBranchSnapshot {
    /// Fully qualified ref name.
    pub full_name: String,
    /// User-facing remote branch name.
    pub short_name: String,
    /// Branch tip.
    pub tip: Oid,
}

/// Reference snapshot and peeled target information.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefSnapshot {
    /// Fully qualified reference name.
    pub full_name: String,
    /// User-facing ref name.
    pub short_name: String,
    /// Direct target.
    pub target: Oid,
    /// Tag-peeled or direct object target.
    pub peeled_target: Oid,
    /// Ref namespace classification.
    pub kind: RefKind,
    /// Symbolic target for aliases such as `origin/HEAD`.
    pub symbolic_target: Option<String>,
}

/// Reference namespace classification.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RefKind {
    /// `HEAD`.
    Head,
    /// `refs/heads/*`.
    LocalBranch,
    /// `refs/remotes/*`.
    RemoteBranch,
    /// `refs/tags/*`.
    Tag,
    /// Any other ref namespace.
    Other,
}

/// A display label attached to a commit row.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RefLabel {
    /// Visible label text.
    pub name: String,
    /// Reference type, used for stable ordering and color.
    pub kind: RefKind,
    /// Whether this label represents active HEAD.
    pub is_head: bool,
    /// Whether this local branch is active in the selected worktree.
    pub is_current: bool,
}

/// History scope selected by the user.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HistoryScope {
    /// Unique roots from HEAD, local branches, remotes, and tags.
    AllRefs,
    /// A single ref tip.
    Ref { full_name: String, tip: Oid },
}

/// Query used to start a persistent history cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryQuery {
    /// Root-selection policy.
    pub scope: HistoryScope,
}

/// One row's lightweight, list-ready commit data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitRow {
    /// Commit object ID.
    pub oid: Oid,
    /// Parent object IDs in commit order.
    pub parents: SmallVec<[Oid; 2]>,
    /// First line of the commit message.
    pub summary: String,
    /// Display author name.
    pub author_name: String,
    /// Author timestamp and offset.
    pub commit_time: GitTime,
    /// Labels currently known for this object.
    pub labels: SmallVec<[RefLabel; 3]>,
    /// Rendering-relevant commit flags.
    pub flags: CommitFlags,
}

/// Git timestamp preserved without backend lifetime data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GitTime {
    /// Seconds since the Unix epoch.
    pub seconds_since_epoch: i64,
    /// Signed UTC offset in minutes.
    pub offset_minutes: i32,
}

/// Rendering-relevant commit traits.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CommitFlags {
    /// Whether the commit has more than one parent.
    pub is_merge: bool,
    /// Whether traversal reached a shallow boundary.
    pub is_shallow_boundary: bool,
}

/// Commit metadata loaded after a row is selected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitDetail {
    /// Commit ID.
    pub oid: Oid,
    /// Complete subject line.
    pub subject: String,
    /// Commit body without the subject.
    pub body: String,
    /// Author identity and timestamp.
    pub author: Signature,
    /// Committer identity and timestamp.
    pub committer: Signature,
    /// Parent IDs in commit order.
    pub parents: SmallVec<[Oid; 2]>,
}

/// Owned Git signature.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Signature {
    /// Display name.
    pub name: String,
    /// Email address supplied by the commit object.
    pub email: String,
    /// Commit timestamp and offset.
    pub time: GitTime,
}

/// Selected parent comparison for changed files and diffs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffParent {
    /// First parent of a non-root commit.
    FirstParent,
    /// An explicitly selected merge parent.
    Parent(Oid),
    /// Empty tree used by root commits.
    EmptyTree,
}

/// Changed-file summary for one commit/parent pair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitFiles {
    /// Commit being compared.
    pub oid: Oid,
    /// Parent-selection policy used for this comparison.
    pub parent: DiffParent,
    /// File summaries.
    pub files: Vec<ChangedFile>,
}

/// One changed path and its optional line statistics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangedFile {
    /// Path before the change.
    pub old_path: Option<RepoPath>,
    /// Path after the change.
    pub new_path: Option<RepoPath>,
    /// File-change classification.
    pub status: ChangeKind,
    /// Added lines when computed.
    pub additions: Option<u32>,
    /// Deleted lines when computed.
    pub deletions: Option<u32>,
    /// Whether either side is binary.
    pub is_binary: bool,
}

/// File-change classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeKind {
    /// A path was added.
    Added,
    /// A path was modified.
    Modified,
    /// A path was deleted.
    Deleted,
    /// A path was renamed.
    Renamed,
    /// A path was copied.
    Copied,
    /// Backend could not classify the change more precisely.
    Unknown,
}

/// The uncommitted state of the working tree, split the way Git splits it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkingTreeStatus {
    /// Changes in the index, ready to commit.
    pub staged: Vec<ChangedFile>,
    /// Worktree changes not yet staged. Untracked files fold in as
    /// additions; conflicted paths as [`ChangeKind::Unknown`].
    pub unstaged: Vec<ChangedFile>,
}

impl WorkingTreeStatus {
    /// The counts the working-tree row label shows.
    #[must_use]
    pub fn summary(&self) -> WorkingTreeSummary {
        WorkingTreeSummary {
            staged: self.staged.len(),
            unstaged: self.unstaged.len(),
        }
    }

    /// Whether there is nothing to commit and nothing to stage.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.staged.is_empty() && self.unstaged.is_empty()
    }
}

/// Counts for the working-tree row label.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkingTreeSummary {
    /// Files with staged changes.
    pub staged: usize,
    /// Files with unstaged changes, including untracked.
    pub unstaged: usize,
}

/// Request to lazily load a one-file diff.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileDiffRequest {
    /// Commit being compared.
    pub oid: Oid,
    /// Parent-selection policy.
    pub parent: DiffParent,
    /// Selected path in the commit side of the comparison.
    pub path: RepoPath,
}

/// Bounded display representation of one file diff.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileDiff {
    /// Request that produced this output.
    pub request: FileDiffRequest,
    /// Bounded outcome for rendering.
    pub content: DiffContent,
}

/// Safe outcome of formatting a diff.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiffContent {
    /// Textual unified diff lines.
    Text { lines: Vec<DiffLine> },
    /// The compared path contains binary content.
    Binary { message: String },
    /// The compared path is an image: encoded bytes for each present side.
    Image {
        /// Old-side encoded bytes; absent when the file was added.
        before: Option<Vec<u8>>,
        /// New-side encoded bytes; absent when the file was deleted.
        after: Option<Vec<u8>>,
        /// Lowercased extension that identified the format ("png", "jpg", …).
        format: String,
    },
    /// The formatted result exceeded configured safety limits.
    TooLarge {
        /// Formatted output byte count observed before stopping.
        bytes: usize,
        /// Formatted output line count observed before stopping.
        lines: usize,
        /// Maximum formatted bytes allowed.
        byte_limit: usize,
        /// Maximum formatted lines allowed.
        line_limit: usize,
    },
    /// The requested content could not be read without failing the whole window.
    Unavailable { message: String },
}

/// Classified display line in a unified diff.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffLine {
    /// Semantic line class.
    pub kind: DiffLineKind,
    /// Text excluding the line ending.
    pub text: String,
    /// Old-side line number where applicable.
    pub old_line: Option<u32>,
    /// New-side line number where applicable.
    pub new_line: Option<u32>,
}

/// Unified-diff line class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffLineKind {
    /// File header or metadata.
    Meta,
    /// Hunk boundary.
    Hunk,
    /// Context line.
    Context,
    /// Added line.
    Addition,
    /// Removed line.
    Deletion,
    /// Marker such as no-newline-at-end-of-file.
    Marker,
}

/// Explicit Git operation supported by the v1 command boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationKind {
    /// Fetch remote refs and objects.
    Fetch,
    /// Push the current branch to its upstream.
    Push,
    /// Pull the current branch from its upstream.
    Pull,
    /// Create a branch.
    CreateBranch,
    /// Create and check out a branch.
    CreateAndCheckoutBranch,
    /// Record the staged changes as a commit.
    Commit,
}

/// Progress emitted while an explicit Git operation is alive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationProgress {
    /// Kind of operation in progress.
    pub kind: OperationKind,
    /// Human-readable Git progress text.
    pub message: String,
    /// Optional completed units reported by Git.
    pub completed: Option<u64>,
    /// Optional total units reported by Git.
    pub total: Option<u64>,
}

/// User-requested fetch parameters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchRequest {
    /// Worktree used as the Git command context.
    pub worktree: WorktreeId,
    /// Optional remote name; absent means all remotes.
    pub remote: Option<String>,
    /// Whether stale tracking refs may be removed.
    pub prune: bool,
}

/// User-requested branch creation parameters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateBranchRequest {
    /// Worktree used as the Git command context.
    pub worktree: WorktreeId,
    /// New branch name.
    pub name: String,
    /// Commit from which the branch starts.
    pub start: Oid,
    /// Whether the new branch should become active.
    pub checkout: bool,
}

/// Result of an explicit Git operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationOutcome {
    /// Operation completed successfully.
    Succeeded {
        /// Operation type.
        kind: OperationKind,
        /// User-displayable summary.
        summary: String,
    },
    /// Operation was cancelled and its child process reaped.
    Cancelled { kind: OperationKind },
    /// Operation failed without discarding currently loaded repository data.
    Failed {
        /// Operation type.
        kind: OperationKind,
        /// Typed user-facing failure.
        error: RepoFailure,
    },
}

/// Number of distinct graph line colors a renderer must provide.
///
/// Colors are carried by a continuing graph line rather than derived from a
/// lane index, so a line keeps its color when it shifts lanes.
pub const GRAPH_COLOR_COUNT: u8 = 6;

/// Highest lane index the renderer will lay out before degrading.
///
/// Malformed or pathological input must not translate into unbounded geometry.
pub const GRAPH_MAX_LANES: u16 = 128;

/// Semantic graph row emitted atomically with its commit row.
///
/// Rows are positional: a batch's `graph_rows` parallel its `rows`, so the
/// commit identity lives on the commit row rather than being duplicated here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphRow {
    /// Stable lane containing the commit node.
    pub node_lane: u16,
    /// Color index of the line owning the node, below `GRAPH_COLOR_COUNT`.
    pub node_color: u8,
    /// Per-row lane segments without UI coordinates.
    pub segments: SmallVec<[GraphSegment; 4]>,
    /// Row properties a painter needs but cannot infer from segments alone.
    pub flags: GraphFlags,
}

/// A graph primitive expressed in stable lane indices, never pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphSegment {
    /// A line passing straight through the row.
    Vertical { lane: u16, color: u8 },
    /// A line leaving the node's lane toward an additional parent.
    Fork { from: u16, to: u16, color: u8 },
    /// A merge-parent edge arriving in the node's lane.
    Merge { from: u16, to: u16, color: u8 },
    /// A line that ends at this row, including a filtered-out parent stub.
    Terminate { lane: u16, color: u8 },
}

/// Row properties that affect painting but are not edges.
#[expect(
    clippy::struct_excessive_bools,
    reason = "a set of independent row flags is exactly what §7.2 defines"
)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GraphFlags {
    /// The commit has more than one parent.
    pub is_merge: bool,
    /// The commit has no parents.
    pub is_root: bool,
    /// Traversal stopped here because the repository is shallow.
    pub is_shallow_boundary: bool,
    /// The node's line was already drawn in the row above.
    ///
    /// False for branch tips and disconnected roots, whose lines start at the
    /// node itself; a painter drawing rows in isolation (§7.4) cannot infer
    /// this from segments alone.
    pub continues_above: bool,
}

/// Incremental output from a persistent history cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryBatch {
    /// Rows decoded during this batch.
    pub rows: Vec<CommitRow>,
    /// Matching graph rows in exactly the same order as `rows`.
    pub graph_rows: Vec<GraphRow>,
    /// Whether more rows may be requested from the cursor.
    pub has_more: bool,
    /// Time spent producing this batch.
    pub elapsed: Duration,
}

/// User-visible category of backend failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepoFailureKind {
    /// Discovery found no repository.
    NotARepository,
    /// Filesystem permissions prevent required reads.
    PermissionDenied,
    /// Git trust policy rejected the repository.
    UntrustedRepository,
    /// Repository data is malformed.
    CorruptRepository,
    /// A requested object cannot be found.
    MissingObject,
    /// Object hash format is unsupported by this build.
    UnsupportedHash,
    /// One linked worktree is inaccessible.
    InaccessibleWorktree,
    /// Another operation prevents this one from running safely.
    OperationConflict,
    /// Git authentication could not complete.
    Authentication,
    /// A network operation failed.
    Network,
    /// A user or generation cancellation stopped the operation.
    Cancelled,
    /// A non-classified implementation failure.
    Internal,
}

/// Actionable error information suitable for a banner or error window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserFacingError {
    /// Concise heading.
    pub title: String,
    /// User-actionable explanation.
    pub message: String,
    /// Optional diagnostics safe to reveal on request.
    pub details: Option<String>,
    /// Whether retrying may reasonably succeed.
    pub retryable: bool,
}

/// A versioned repository failure with both display and diagnostic context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoFailure {
    /// Stable matching category.
    pub kind: RepoFailureKind,
    /// User-facing content.
    pub user: UserFacingError,
    /// Optional operation that failed.
    pub operation: Option<OperationKind>,
    /// Session for stale-event rejection when applicable.
    pub session: Option<RepoSessionId>,
    /// Generation for stale-event rejection when applicable.
    pub generation: Option<Generation>,
}

impl RepoFailure {
    /// Creates a failure with no request context.
    #[must_use]
    pub fn new(
        kind: RepoFailureKind,
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            user: UserFacingError {
                title: title.into(),
                message: message.into(),
                details: None,
                retryable: false,
            },
            operation: None,
            session: None,
            generation: None,
        }
    }

    /// Attaches diagnostics that are safe to reveal on request.
    #[must_use]
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.user.details = Some(details.into());
        self
    }
}

impl fmt::Display for RepoFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.user.title, self.user.message)
    }
}

impl std::error::Error for RepoFailure {}

/// Semantic update to one branch's lazy ahead/behind state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AheadBehindUpdate {
    /// Updated local branch ref.
    pub full_name: String,
    /// Newly calculated state.
    pub state: AheadBehindState,
}

/// A time captured at an external-system boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapturedAt(pub SystemTime);

/// The GitHub account a token authenticates as.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GithubAccount {
    /// The login name, e.g. `octocat`.
    pub login: String,
}

/// One open pull request, as much as branch decoration needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrSummary {
    pub number: u64,
    pub draft: bool,
    /// The branch the PR merges from (`head.ref`).
    pub head_branch: String,
    /// The PR page, for click-through.
    pub html_url: String,
}

/// Where one check or workflow run stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckStatus {
    Queued,
    InProgress,
    Completed(CheckConclusion),
}

/// How a completed check ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckConclusion {
    Success,
    Failure,
    Neutral,
    Cancelled,
    Skipped,
    TimedOut,
    ActionRequired,
    /// Backend reported a conclusion this build does not know.
    Unknown,
}

/// One check run attached to a commit; GitHub Actions jobs appear here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckRun {
    pub name: String,
    pub status: CheckStatus,
    /// The check's page, for click-through.
    pub html_url: String,
}

/// One GitHub Actions workflow run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowRun {
    /// The run's API identity, for fetching its jobs.
    pub id: u64,
    /// The workflow's name, e.g. `CI`.
    pub name: String,
    /// The commit subject or PR title GitHub displays for the run.
    pub display_title: String,
    pub run_number: u64,
    /// What triggered the run, e.g. `push`.
    pub event: String,
    /// Who triggered the run.
    pub actor: String,
    /// The branch the run was triggered on.
    pub branch: String,
    pub sha: String,
    pub status: CheckStatus,
    /// When the run started, epoch seconds.
    pub started_at: Option<i64>,
    /// When the run finished, epoch seconds; `None` while it runs.
    pub completed_at: Option<i64>,
    /// The run's page, for click-through.
    pub html_url: String,
}

/// One job of a workflow run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowJob {
    /// The job's API identity, for fetching its log.
    pub id: u64,
    pub name: String,
    pub status: CheckStatus,
    /// When the job started, epoch seconds.
    pub started_at: Option<i64>,
    /// When the job finished, epoch seconds; `None` while it runs.
    pub completed_at: Option<i64>,
    pub steps: Vec<WorkflowStep>,
    /// The job's page, for click-through.
    pub html_url: String,
}

/// One step of a job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowStep {
    pub name: String,
    pub status: CheckStatus,
    /// When the step started, epoch seconds.
    pub started_at: Option<i64>,
    /// When the step finished, epoch seconds.
    pub completed_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oid_round_trips_sha1_and_sha256() -> Result<(), OidParseError> {
        let sha1 = Oid::from_hex("0123456789abcdef0123456789abcdef01234567")?;
        let sha256 =
            Oid::from_hex("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")?;

        assert_eq!(sha1.to_hex(), "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(sha1.abbreviated(7), "0123456");
        assert_eq!(sha256.as_bytes().len(), 32);
        Ok(())
    }

    #[test]
    fn load_state_exposes_retained_values() {
        let error = UserFacingError {
            title: String::from("Refresh failed"),
            message: String::from("Try again"),
            details: None,
            retryable: true,
        };
        let state = LoadState::Failed {
            error,
            previous: Some(42_u8),
        };

        assert_eq!(state.value(), Some(&42));
        assert!(!state.is_loading());
    }

    #[test]
    fn envelopes_retain_identity_when_mapped() {
        let session = RepoSessionId::new();
        let envelope = RepoEnvelope {
            session,
            generation: Generation(4),
            request: RequestId(9),
            payload: 3_u8,
        };
        let mapped = envelope.map(u16::from);

        assert_eq!(mapped.session, session);
        assert_eq!(mapped.generation, Generation(4));
        assert_eq!(mapped.request, RequestId(9));
        assert_eq!(mapped.payload, 3);
    }
}
