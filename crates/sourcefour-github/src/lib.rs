//! A small GitHub REST client for the read surfaces Sourcefour shows:
//! pull requests, commit checks, and Actions runs.
//!
//! All HTTP goes through the [`GithubTransport`] trait so tests never touch
//! the network (§12); the shipping transport is `ureq` on a background
//! executor, matching how git subprocesses run.

mod api;
mod credentials;
mod remote;
mod transport;

pub use api::{check_runs, open_pulls, whoami, workflow_runs};
pub use credentials::{delete_token, load_token, store_token};
pub use remote::GithubRemote;
pub use transport::{ApiFailure, ApiFailureKind, GithubTransport, UreqTransport};
