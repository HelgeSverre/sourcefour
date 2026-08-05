//! A small GitHub REST client for the read surfaces Sourcefour shows:
//! pull requests, commit checks, and Actions runs.
//!
//! All HTTP goes through the [`GithubTransport`] trait so tests never touch
//! the network (§12); the shipping transport is `ureq` on a background
//! executor, matching how git subprocesses run.

mod api;
mod credentials;
mod log;
mod remote;
mod time;
mod transport;

pub use api::{
    actions_run_id_in_url, check_runs, commit_states, job_log, open_pulls, run_jobs, whoami,
    workflow_run, workflow_runs,
};
pub use credentials::{delete_token, gh_cli_token, load_token, store_token};
pub use log::{first_error, split_timestamp, step_slice};
pub use remote::GithubRemote;
pub use transport::{GithubTransport, UreqTransport};

/// The one host this build integrates with; also the credentials-file key.
pub const GITHUB_HOST: &str = "github.com";
