#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DemoCommit {
    pub(crate) subject: &'static str,
    pub(crate) author: &'static str,
    pub(crate) date: &'static str,
    pub(crate) hash: &'static str,
    pub(crate) lane: usize,
    pub(crate) merge: bool,
}

pub(crate) const COMMITS: [DemoCommit; 18] = [
    DemoCommit {
        subject: "Merge branch 'feature/worktrees' into main",
        author: "Helge Sverre",
        date: "2 hours ago",
        hash: "9f3e21a",
        lane: 0,
        merge: true,
    },
    DemoCommit {
        subject: "chore(deps): pin gpui to rev d637307",
        author: "Helge Sverre",
        date: "3 hours ago",
        hash: "7c1d9b4",
        lane: 0,
        merge: false,
    },
    DemoCommit {
        subject: "List linked worktrees from common git directory",
        author: "Helge Sverre",
        date: "5 hours ago",
        hash: "4b8e0f2",
        lane: 1,
        merge: false,
    },
    DemoCommit {
        subject: "Render metadata sidebar loading state",
        author: "Helge Sverre",
        date: "Yesterday",
        hash: "e57a109",
        lane: 0,
        merge: false,
    },
    DemoCommit {
        subject: "Preserve selected commit on a safe refresh",
        author: "Helge Sverre",
        date: "Yesterday",
        hash: "6c9de83",
        lane: 2,
        merge: false,
    },
    DemoCommit {
        subject: "Add deterministic history fixture",
        author: "Helge Sverre",
        date: "Yesterday",
        hash: "0f2a49d",
        lane: 0,
        merge: false,
    },
    DemoCommit {
        subject: "Improve repository discovery failure messages",
        author: "Helge Sverre",
        date: "2 days ago",
        hash: "e6b1a95",
        lane: 3,
        merge: false,
    },
    DemoCommit {
        subject: "Document the hybrid Git backend",
        author: "Helge Sverre",
        date: "2 days ago",
        hash: "2a7ccf4",
        lane: 1,
        merge: false,
    },
    DemoCommit {
        subject: "Use all unique references as default scope",
        author: "Helge Sverre",
        date: "3 days ago",
        hash: "5cb3418",
        lane: 0,
        merge: false,
    },
    DemoCommit {
        subject: "Add graph colour continuity goldens",
        author: "Helge Sverre",
        date: "3 days ago",
        hash: "3d88c70",
        lane: 4,
        merge: false,
    },
    DemoCommit {
        subject: "Merge branch 'feature/history'",
        author: "Helge Sverre",
        date: "4 days ago",
        hash: "7f0e4f1",
        lane: 0,
        merge: true,
    },
    DemoCommit {
        subject: "Read commits through a persistent cursor",
        author: "Helge Sverre",
        date: "4 days ago",
        hash: "66ea252",
        lane: 2,
        merge: false,
    },
    DemoCommit {
        subject: "Implement typed repository sessions",
        author: "Helge Sverre",
        date: "5 days ago",
        hash: "0ceef2b",
        lane: 5,
        merge: false,
    },
    DemoCommit {
        subject: "Add sourcefour virtual workspace",
        author: "Helge Sverre",
        date: "5 days ago",
        hash: "c4f4b7a",
        lane: 0,
        merge: false,
    },
    DemoCommit {
        subject: "Promote implementation specification",
        author: "Helge Sverre",
        date: "6 days ago",
        hash: "4f8d31b",
        lane: 1,
        merge: false,
    },
    DemoCommit {
        subject: "Archive restart prototype",
        author: "Helge Sverre",
        date: "6 days ago",
        hash: "1f202f1",
        lane: 0,
        merge: false,
    },
    DemoCommit {
        subject: "Initial Sourcefour architecture",
        author: "Helge Sverre",
        date: "1 week ago",
        hash: "128b3a6",
        lane: 2,
        merge: false,
    },
    DemoCommit {
        subject: "Repository bootstrap",
        author: "Helge Sverre",
        date: "1 week ago",
        hash: "ac7d153",
        lane: 0,
        merge: false,
    },
];

/// Repository identity shown by `--demo`, matching the screenshot fixture.
pub(crate) const REPOSITORY_NAME: &str = "sourcefour";
pub(crate) const REPOSITORY_PATH: &str = "~/code/sourcefour";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_is_stable() {
        assert_eq!(COMMITS[0].hash, "9f3e21a");
        assert_eq!(COMMITS.len(), 18);
    }

    #[test]
    fn fixture_exceeds_the_m0_history_viewport() {
        const BASELINE_VISIBLE_ROWS: usize = 13;
        assert!(COMMITS.len() > BASELINE_VISIBLE_ROWS);
    }
}
