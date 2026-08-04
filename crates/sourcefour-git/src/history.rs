//! Incremental history traversal on a dedicated worker thread.
//!
//! §6.9 forbids restarting from the roots and skipping `N` commits per page, so
//! the traversal must outlive a single call. A gix walk borrows its repository,
//! which cannot be stored beside it in one struct without `unsafe`; §6.3 already
//! prescribes the alternative, so the walk lives on a thread that owns its own
//! repository and answers batch requests over a channel.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread,
    time::Instant,
};

use smallvec::SmallVec;
use sourcefour_graph::GraphState;
use sourcefour_model::{
    CommitFlags, CommitRow, GitTime, HistoryBatch, HistoryQuery, HistoryScope, LabelsByObject, Oid,
    RepoFailure, RepoFailureKind, RepoLocation,
};

/// Rows in the first batch: enough to fill the viewport immediately (§6.9).
pub const FIRST_BATCH_ROWS: usize = 256;
/// Rows in every later batch (§6.9).
pub const NEXT_BATCH_ROWS: usize = 512;

/// A resumable history traversal owned by its own thread.
pub struct GixHistoryCursor {
    requests: Sender<usize>,
    batches: Receiver<Result<HistoryBatch, RepoFailure>>,
    cancelled: Arc<AtomicBool>,
    delivered: usize,
}

impl GixHistoryCursor {
    /// Starts a traversal for `query`.
    ///
    /// The repository is opened once here to surface an unusable repository
    /// immediately, and again on the worker thread, which owns its handle
    /// because a gix repository is not `Sync` (§6.3).
    ///
    /// # Errors
    ///
    /// Returns a typed failure when the repository cannot be opened.
    pub fn start(
        location: &RepoLocation,
        query: HistoryQuery,
        labels: LabelsByObject,
    ) -> Result<Self, RepoFailure> {
        let git_dir = location.git_dir.clone();
        gix::open(&git_dir).map_err(|error| open_failure(&git_dir, &error))?;

        let (requests, incoming) = channel::<usize>();
        let (outgoing, batches) = channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);

        thread::Builder::new()
            .name(String::from("sourcefour-history"))
            .spawn(move || {
                run(
                    &git_dir,
                    &query,
                    &labels,
                    &incoming,
                    &outgoing,
                    &worker_cancelled,
                );
            })
            .map_err(|error| {
                RepoFailure::new(
                    RepoFailureKind::Internal,
                    "History could not start",
                    String::from("A background thread for reading history could not be started."),
                )
                .with_details(error.to_string())
            })?;

        Ok(Self {
            requests,
            batches,
            cancelled,
            delivered: 0,
        })
    }

    /// Rows to request next: 256 to fill the window, then 512 (§6.9).
    #[must_use]
    pub const fn next_batch_size(&self) -> usize {
        if self.delivered == 0 {
            FIRST_BATCH_ROWS
        } else {
            NEXT_BATCH_ROWS
        }
    }

    /// Requests and waits for the next batch, resuming where the last stopped.
    ///
    /// # Errors
    ///
    /// Returns a typed failure when traversal cannot continue.
    fn read_next_batch(&mut self, max_rows: usize) -> Result<HistoryBatch, RepoFailure> {
        if self.requests.send(max_rows).is_err() {
            return Ok(finished());
        }
        match self.batches.recv() {
            Ok(batch) => {
                if let Ok(batch) = &batch {
                    self.delivered += batch.rows.len();
                }
                batch
            }
            // The worker ended, which for a traversal means there is no more.
            Err(_) => Ok(finished()),
        }
    }

    /// Total rows handed out so far.
    #[must_use]
    pub const fn delivered(&self) -> usize {
        self.delivered
    }

    /// Stops the traversal at the next row boundary.
    fn stop(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl crate::HistoryCursor for GixHistoryCursor {
    fn next_batch(&mut self, max_rows: usize) -> Result<HistoryBatch, RepoFailure> {
        self.read_next_batch(max_rows)
    }

    fn cancel(&self) {
        self.stop();
    }
}

impl Drop for GixHistoryCursor {
    fn drop(&mut self) {
        // Releasing the request channel ends the worker's loop.
        self.stop();
    }
}

fn finished() -> HistoryBatch {
    HistoryBatch {
        rows: Vec::new(),
        graph_rows: Vec::new(),
        has_more: false,
        elapsed: std::time::Duration::ZERO,
    }
}

/// The worker: owns a repository, a walk, and the graph continuation state.
fn run(
    git_dir: &std::path::Path,
    query: &HistoryQuery,
    labels: &LabelsByObject,
    requests: &Receiver<usize>,
    batches: &Sender<Result<HistoryBatch, RepoFailure>>,
    cancelled: &AtomicBool,
) {
    let repository = match gix::open(git_dir) {
        Ok(repository) => repository,
        Err(error) => {
            let _ = batches.send(Err(open_failure(git_dir, &error)));
            return;
        }
    };
    let roots = roots(&repository, &query.scope);
    let mut walk = (!roots.is_empty())
        .then(|| {
            repository
                .rev_walk(roots)
                .sorting(gix::revision::walk::Sorting::ByCommitTime(
                    gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
                ))
                .all()
                .ok()
        })
        .flatten();
    let mut graph = GraphState::default();

    while let Ok(max_rows) = requests.recv() {
        let started = Instant::now();
        let mut rows = Vec::new();
        let mut graph_rows = Vec::new();
        let mut exhausted = walk.is_none();

        while rows.len() < max_rows {
            if cancelled.load(Ordering::Acquire) {
                break;
            }
            let Some(active) = walk.as_mut() else {
                break;
            };
            let Some(step) = active.next() else {
                exhausted = true;
                break;
            };
            // A corrupt object ends loading with the rows already decoded (§11.2).
            let Ok(info) = step else {
                exhausted = true;
                break;
            };
            let Some(row) = decode(&repository, &info, labels) else {
                continue;
            };
            graph_rows.push(graph.push(row.oid, &row.parents));
            rows.push(row);
        }

        let batch = HistoryBatch {
            has_more: !exhausted,
            rows,
            graph_rows,
            elapsed: started.elapsed(),
        };
        if batches.send(Ok(batch)).is_err() {
            return;
        }
    }
}

/// Unique peeled roots for the scope (§6.8).
fn roots(repository: &gix::Repository, scope: &HistoryScope) -> Vec<gix::ObjectId> {
    let mut roots = Vec::new();
    match scope {
        HistoryScope::Ref { tip, .. } => {
            roots.push(gix::ObjectId::from_bytes_or_panic(tip.as_bytes()));
        }
        HistoryScope::AllRefs => {
            if let Ok(mut head) = repository.head()
                && let Ok(commit) = head.peel_to_commit()
            {
                roots.push(commit.id);
            }
            if let Ok(platform) = repository.references() {
                let groups = [
                    platform.local_branches(),
                    platform.remote_branches(),
                    platform.tags(),
                ];
                for iter in groups.into_iter().flatten() {
                    for mut reference in iter.filter_map(Result::ok) {
                        // Refs that do not peel to commits are ignored (§6.8).
                        // Peeling alone is not enough: git.git tags a blob
                        // (junio-gpg-pub), and a single non-commit root would
                        // error the walk's first step and end the whole
                        // traversal as if the repository were empty.
                        if let Ok(id) = reference.peel_to_id()
                            && repository
                                .find_header(id)
                                .is_ok_and(|header| header.kind() == gix::object::Kind::Commit)
                        {
                            roots.push(id.detach());
                        }
                    }
                }
            }
        }
    }
    roots.sort_unstable();
    roots.dedup();
    roots
}

fn decode(
    repository: &gix::Repository,
    info: &gix::revision::walk::Info<'_>,
    labels: &LabelsByObject,
) -> Option<CommitRow> {
    let commit = repository.find_commit(info.id).ok()?;
    let decoded = commit.decode().ok()?;
    let oid = convert_oid(info.id.as_ref())?;
    let parents: SmallVec<[Oid; 2]> = decoded
        .parents()
        .filter_map(|parent| convert_oid(parent.as_ref()))
        .collect();
    let time = decoded.time().ok()?;
    Some(CommitRow {
        labels: labels.get(&oid).cloned().unwrap_or_default(),
        flags: CommitFlags {
            is_merge: parents.len() > 1,
            is_shallow_boundary: false,
        },
        summary: decoded.message().summary().to_string(),
        author_name: decoded
            .author()
            .map_or_else(|_| String::new(), |author| author.name.to_string()),
        commit_time: GitTime {
            seconds_since_epoch: time.seconds,
            offset_minutes: time.offset / 60,
        },
        oid,
        parents,
    })
}

pub(crate) fn convert_oid(id: &gix::hash::oid) -> Option<Oid> {
    match id.as_bytes().len() {
        20 => Some(Oid::sha1(id.as_bytes().try_into().ok()?)),
        32 => Some(Oid::sha256(id.as_bytes().try_into().ok()?)),
        _ => None,
    }
}

pub(crate) fn open_failure(
    git_dir: &std::path::Path,
    error: &impl std::fmt::Display,
) -> RepoFailure {
    RepoFailure::new(
        RepoFailureKind::CorruptRepository,
        "Repository could not be opened",
        format!("The repository at {} could not be read.", git_dir.display()),
    )
    .with_details(error.to_string())
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{HistoryQuery, HistoryScope, LabelsByObject};
    use sourcefour_test_support::TempRepo;

    use super::{FIRST_BATCH_ROWS, GixHistoryCursor, NEXT_BATCH_ROWS};
    use crate::HistoryCursor as _;
    use crate::{discover, references, worktrees};

    fn cursor(repository: &TempRepo) -> Result<GixHistoryCursor, Box<dyn std::error::Error>> {
        let location = discover(repository.path())?;
        Ok(GixHistoryCursor::start(
            &location,
            HistoryQuery {
                scope: HistoryScope::AllRefs,
            },
            LabelsByObject::new(),
        )?)
    }

    fn commits(repository: &TempRepo, prefix: &str, count: usize) {
        for index in 0..count {
            repository.commit(&format!("{prefix} {index}"));
        }
    }

    #[test]
    fn history_arrives_newest_first() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        commits(&repository, "commit", 2);
        let mut cursor = cursor(&repository)?;

        let batch = cursor.next_batch(10)?;

        let summaries: Vec<&str> = batch.rows.iter().map(|row| row.summary.as_str()).collect();
        assert_eq!(summaries, ["commit 1", "commit 0", "initial commit"]);
        assert!(!batch.has_more);
        Ok(())
    }

    #[test]
    fn the_first_batch_is_smaller_than_the_rest() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        let mut cursor = cursor(&repository)?;

        assert_eq!(cursor.next_batch_size(), FIRST_BATCH_ROWS);
        cursor.next_batch(1)?;
        assert_eq!(cursor.next_batch_size(), NEXT_BATCH_ROWS);
        Ok(())
    }

    #[test]
    fn a_large_history_is_not_loaded_all_at_once() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        commits(&repository, "commit", 60);
        let mut cursor = cursor(&repository)?;

        let batch = cursor.next_batch(25)?;

        assert_eq!(batch.rows.len(), 25);
        assert!(batch.has_more, "the traversal stops at the batch size");
        assert_eq!(batch.graph_rows.len(), batch.rows.len());
        Ok(())
    }

    /// The batching property §6.9 actually guards: a resumed traversal must
    /// continue, not restart from the roots and skip rows.
    #[test]
    fn each_batch_continues_where_the_last_stopped() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        commits(&repository, "commit", 30);
        let mut cursor = cursor(&repository)?;

        let first = cursor.next_batch(10)?;
        let second = cursor.next_batch(10)?;
        let third = cursor.next_batch(100)?;

        let mut seen: Vec<_> = first
            .rows
            .iter()
            .chain(&second.rows)
            .chain(&third.rows)
            .map(|row| row.oid)
            .collect();
        let total = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(total, 31, "every commit is delivered exactly once");
        assert_eq!(seen.len(), total, "no commit is delivered twice");
        assert!(!third.has_more);
        Ok(())
    }

    /// Rows already handed to the interface must never be renumbered by a later
    /// batch — the §13 M3 acceptance criterion.
    #[test]
    fn a_later_batch_does_not_change_earlier_graph_rows() -> Result<(), Box<dyn std::error::Error>>
    {
        let repository = TempRepo::init();
        repository.git(&["checkout", "-q", "-b", "side"]);
        commits(&repository, "side", 3);
        repository.git(&["checkout", "-q", "main"]);
        commits(&repository, "main", 3);
        repository.git(&["merge", "-q", "--no-ff", "side", "-m", "merge side"]);

        let mut split = cursor(&repository)?;
        let first = split.next_batch(3)?;
        let second = split.next_batch(100)?;

        let mut whole = cursor(&repository)?;
        let all = whole.next_batch(1000)?;

        let combined: Vec<_> = first
            .graph_rows
            .iter()
            .chain(&second.graph_rows)
            .cloned()
            .collect();
        assert_eq!(
            combined, all.graph_rows,
            "splitting the traversal must not change any row's lane or color"
        );
        Ok(())
    }

    #[test]
    fn a_branch_scope_narrows_history() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.git(&["checkout", "-q", "-b", "side"]);
        commits(&repository, "side", 2);
        repository.git(&["checkout", "-q", "main"]);
        let location = discover(repository.path())?;
        let trees = worktrees(&location)?;
        let found = references(&location, &trees)?;
        let main = found
            .local_branches
            .iter()
            .find(|branch| branch.short_name == "main")
            .expect("main is listed");

        let mut scoped = GixHistoryCursor::start(
            &location,
            HistoryQuery {
                scope: HistoryScope::Ref {
                    full_name: main.full_name.clone(),
                    tip: main.tip,
                },
            },
            LabelsByObject::new(),
        )?;
        let batch = scoped.next_batch(100)?;

        assert_eq!(
            batch.rows.len(),
            1,
            "a branch scope walks only that branch's history"
        );
        Ok(())
    }

    /// git.git tags a blob (`junio-gpg-pub`); a root that is not a commit must
    /// not poison the whole walk (§6.8).
    #[test]
    fn a_tag_pointing_to_a_blob_does_not_break_history() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        commits(&repository, "commit", 2);
        std::fs::write(repository.path().join("key.txt"), "not a commit")?;
        let blob = repository.git(&["hash-object", "-w", "key.txt"]);
        repository.git(&["tag", "gpg-pub-key", &blob]);
        let mut cursor = cursor(&repository)?;

        let batch = cursor.next_batch(100)?;

        assert_eq!(
            batch.rows.len(),
            3,
            "every commit is still delivered despite the blob tag"
        );
        Ok(())
    }

    #[test]
    fn an_unborn_repository_yields_no_history() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init_unborn();
        let mut cursor = cursor(&repository)?;

        let batch = cursor.next_batch(10)?;

        assert!(batch.rows.is_empty());
        assert!(!batch.has_more);
        Ok(())
    }

    #[test]
    fn merges_are_flagged_and_carry_both_parents() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.git(&["checkout", "-q", "-b", "side"]);
        commits(&repository, "side", 1);
        repository.git(&["checkout", "-q", "main"]);
        commits(&repository, "main", 1);
        repository.git(&["merge", "-q", "--no-ff", "side", "-m", "merge side"]);
        let mut cursor = cursor(&repository)?;

        let batch = cursor.next_batch(100)?;

        let merge = batch
            .rows
            .iter()
            .find(|row| row.summary == "merge side")
            .expect("the merge commit is present");
        assert!(merge.flags.is_merge);
        assert_eq!(merge.parents.len(), 2);
        Ok(())
    }
}
