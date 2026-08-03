//! Shared test-fixture vocabulary.
//!
//! Disposable repository construction is introduced with repository discovery.
//! Keeping the M0 crate dependency-free lets test fixtures choose their own
//! temporary-directory and Git-process implementation later.

/// A named fixture category used by future reusable Git fixture builders.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixtureKind {
    /// A linear commit history.
    Linear,
    /// A repository containing merge topology.
    Merges,
    /// A repository with linked worktrees.
    Worktrees,
    /// A repository with unusual but valid ref names.
    UnusualReferences,
}

/// Immutable description of a fixture contract, not a repository implementation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureDescription {
    /// Stable fixture identifier.
    pub name: &'static str,
    /// Scenario covered by the fixture.
    pub kind: FixtureKind,
}

impl FixtureDescription {
    /// Creates a fixture description for test registration.
    #[must_use]
    pub const fn new(name: &'static str, kind: FixtureKind) -> Self {
        Self { name, kind }
    }
}

#[cfg(test)]
mod tests {
    use super::{FixtureDescription, FixtureKind};

    #[test]
    fn fixture_descriptions_are_stable_values() {
        let fixture = FixtureDescription::new("linear", FixtureKind::Linear);
        assert_eq!(fixture.name, "linear");
        assert_eq!(fixture.kind, FixtureKind::Linear);
    }
}
