//! Disposable Git repositories for backend tests.
//!
//! Fixtures shell out to the installed `git` executable so that the shapes they
//! produce are the shapes Git actually writes, rather than shapes the reader
//! under test believes Git writes.

use std::{
    cell::Cell,
    path::{Path, PathBuf},
    process::Command,
};

use tempfile::TempDir;

/// Fixed identity and base timestamp so fixture commits hash identically on
/// every machine. Each commit advances the clock by one second.
const FIXTURE_EPOCH_SECONDS: i64 = 981_173_106;
const FIXTURE_NAME: &str = "Sourcefour Fixture";
const FIXTURE_EMAIL: &str = "fixture@sourcefour.invalid";

/// A Git repository in a temporary directory removed on drop.
#[derive(Debug)]
pub struct TempRepo {
    /// Owns the lifetime of every path below; dropping it removes them.
    _directory: TempDir,
    root: PathBuf,
    /// Seconds added to the fixture date for the next commit.
    ///
    /// Without this every commit would share one timestamp, and two commits with
    /// the same message on different branches would hash identically — which
    /// silently turns a merge fixture into a no-op.
    clock: Cell<i64>,
}

impl TempRepo {
    /// Creates a repository with a working tree and one commit on `main`.
    ///
    /// # Panics
    ///
    /// Panics when the temporary directory or any `git` invocation fails,
    /// because a fixture that cannot be built cannot yield a meaningful test.
    #[must_use]
    pub fn init() -> Self {
        let directory = temporary_directory();
        let root = directory.path().join("repository");
        std::fs::create_dir(&root).expect("could not create the fixture working tree");
        run_git(&root, &["init", "--initial-branch", "main"]);
        run_git(&root, &["commit", "--allow-empty", "-m", "initial commit"]);
        Self::new(directory, &root)
    }

    /// Creates a bare repository with one commit on `main`.
    ///
    /// # Panics
    ///
    /// Panics when the temporary directory or any `git` invocation fails.
    #[must_use]
    pub fn init_bare() -> Self {
        let source = Self::init();
        let directory = temporary_directory();
        let root = directory.path().join("repository.git");
        run_git(
            directory.path(),
            &[
                "clone",
                "--bare",
                &source.root.to_string_lossy(),
                &root.to_string_lossy(),
            ],
        );
        Self::new(directory, &root)
    }

    /// Adds a linked worktree checked out on a new branch and returns its path.
    ///
    /// The worktree is placed beside the repository rather than inside it, so
    /// discovery from the linked worktree cannot accidentally walk up into the
    /// main worktree and still appear to succeed.
    ///
    /// # Panics
    ///
    /// Panics when `git worktree add` fails.
    #[must_use]
    pub fn add_worktree(&self, name: &str) -> PathBuf {
        let path = self.sibling(name);
        self.git(&["worktree", "add", "-b", name, &path.to_string_lossy()]);
        canonical(&path)
    }

    /// Path beside the repository root, outside any of its working trees.
    fn sibling(&self, name: &str) -> PathBuf {
        self.root
            .parent()
            .expect("the fixture root always has a parent")
            .join(name)
    }

    /// Clones `origin`, producing a repository with a real remote and upstream.
    ///
    /// # Panics
    ///
    /// Panics when the temporary directory or `git clone` fails.
    #[must_use]
    pub fn clone_of(origin: &Self) -> Self {
        let directory = temporary_directory();
        let root = directory.path().join("clone");
        run_git(
            directory.path(),
            &[
                "clone",
                &origin.root.to_string_lossy(),
                &root.to_string_lossy(),
            ],
        );
        Self::new(directory, &root)
    }

    /// Creates a repository with a working tree but no commit yet.
    ///
    /// # Panics
    ///
    /// Panics when the temporary directory or `git init` fails.
    #[must_use]
    pub fn init_unborn() -> Self {
        let directory = temporary_directory();
        let root = directory.path().join("repository");
        std::fs::create_dir(&root).expect("could not create the fixture working tree");
        run_git(&root, &["init", "--initial-branch", "main"]);
        Self::new(directory, &root)
    }

    /// Adds a linked worktree checked out at a commit rather than a branch.
    ///
    /// # Panics
    ///
    /// Panics when `git worktree add` fails.
    #[must_use]
    pub fn add_detached_worktree(&self, name: &str) -> PathBuf {
        let path = self.sibling(name);
        self.git(&["worktree", "add", "--detach", &path.to_string_lossy()]);
        canonical(&path)
    }

    /// Marks a linked worktree locked, as `git worktree lock` does.
    ///
    /// # Panics
    ///
    /// Panics when `git worktree lock` fails.
    pub fn lock_worktree(&self, path: &Path, reason: &str) {
        self.git(&[
            "worktree",
            "lock",
            "--reason",
            reason,
            &path.to_string_lossy(),
        ]);
    }

    /// Deletes a linked worktree's checkout behind Git's back.
    ///
    /// This is how a worktree on an unmounted volume or a deleted directory
    /// presents itself: registered in `worktrees/`, but with no checkout.
    ///
    /// # Panics
    ///
    /// Panics when the directory cannot be removed.
    pub fn remove_worktree_checkout(path: &Path) {
        std::fs::remove_dir_all(path)
            .unwrap_or_else(|error| panic!("could not remove `{}`: {error}", path.display()));
    }

    /// Path of the repository root: the working tree, or the bare Git directory.
    ///
    /// Already canonical, so tests can compare it against paths that gix
    /// resolved through the platform's symlinked temporary directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Runs `git` inside the repository and returns its trimmed standard output.
    ///
    /// # Panics
    ///
    /// Panics when `git` cannot start or exits with a failure status.
    pub fn git(&self, arguments: &[&str]) -> String {
        run_git(&self.root, arguments)
    }

    /// Adds an empty commit with a timestamp later than every previous one.
    ///
    /// # Panics
    ///
    /// Panics when `git commit` fails.
    pub fn commit(&self, message: &str) -> String {
        self.clock.set(self.clock.get() + 1);
        run_git_at(
            &self.root,
            &["commit", "--allow-empty", "-m", message],
            self.clock.get(),
        );
        self.git(&["rev-parse", "HEAD"])
    }

    fn new(directory: TempDir, root: &Path) -> Self {
        let root = canonical(root);
        run_git(&root, &["config", "user.name", FIXTURE_NAME]);
        run_git(&root, &["config", "user.email", FIXTURE_EMAIL]);
        Self {
            _directory: directory,
            root,
            clock: Cell::new(0),
        }
    }
}

fn temporary_directory() -> TempDir {
    TempDir::new().expect("could not create a temporary directory")
}

/// Canonicalizes without the `\\?\` prefix Windows adds.
///
/// Fixtures hand these paths to the `git` executable, and Git rejects verbatim
/// paths outright: `could not create leading directories ... Invalid argument`.
fn canonical(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path)
        .unwrap_or_else(|error| panic!("could not canonicalize `{}`: {error}", path.display()));
    let Some(stripped) = canonical
        .to_str()
        .and_then(|path| path.strip_prefix(r"\\?\"))
    else {
        return canonical;
    };
    PathBuf::from(stripped)
}

fn run_git(directory: &Path, arguments: &[&str]) -> String {
    run_git_at(directory, arguments, 0)
}

fn run_git_at(directory: &Path, arguments: &[&str], clock: i64) -> String {
    let date = format!("{} +0000", FIXTURE_EPOCH_SECONDS + clock);
    let output = Command::new("git")
        .args(arguments)
        .current_dir(directory)
        // Ignore the developer's own Git configuration: a global `commit.gpgsign`,
        // `init.defaultBranch`, or hook path would otherwise change fixture shape.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", FIXTURE_NAME)
        .env("GIT_AUTHOR_EMAIL", FIXTURE_EMAIL)
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_NAME", FIXTURE_NAME)
        .env("GIT_COMMITTER_EMAIL", FIXTURE_EMAIL)
        .env("GIT_COMMITTER_DATE", &date)
        .output()
        .unwrap_or_else(|error| panic!("could not run `git {}`: {error}", arguments.join(" ")));
    assert!(
        output.status.success(),
        "`git {}` failed: {}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::TempRepo;

    #[test]
    fn init_creates_a_working_tree_repository_with_one_commit() {
        let repository = TempRepo::init();
        assert!(repository.path().join(".git").is_dir());
        assert_eq!(repository.git(&["rev-list", "--count", "HEAD"]), "1");
        assert_eq!(repository.git(&["branch", "--show-current"]), "main");
    }

    #[test]
    fn init_bare_creates_a_repository_without_a_working_tree() {
        let repository = TempRepo::init_bare();
        assert!(repository.path().join("HEAD").is_file());
        assert!(!repository.path().join(".git").exists());
        assert_eq!(repository.git(&["rev-list", "--count", "HEAD"]), "1");
    }

    #[test]
    fn add_worktree_links_a_second_checkout_to_the_same_repository() {
        let repository = TempRepo::init();
        let linked = repository.add_worktree("side");
        assert!(linked.join(".git").is_file());
        let worktrees = repository.git(&["worktree", "list", "--porcelain"]);
        let count = worktrees
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn fixture_commits_are_identical_across_repositories() {
        let first = TempRepo::init();
        let second = TempRepo::init();
        assert_eq!(
            first.git(&["rev-parse", "HEAD"]),
            second.git(&["rev-parse", "HEAD"])
        );
    }
}
