//! Typed REST calls, each one URL and one serde shape.

use serde::Deserialize;
use std::collections::HashMap;

use sourcefour_model::{
    CheckConclusion, CheckRun, CheckStatus, GithubAccount, Oid, PrSummary, WorkflowJob,
    WorkflowRun, WorkflowStep,
};

use crate::time::parse_iso8601;

use crate::{remote::GithubRemote, transport::GithubTransport};

/// The github.com REST endpoint. A GitHub Enterprise host would derive its
/// own (`https://{host}/api/v3`) if support ever lands.
const API_BASE: &str = "https://api.github.com";

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, String> {
    serde_json::from_slice(body).map_err(|error| error.to_string())
}

/// Who the token authenticates as (`GET /user`), which is also the
/// connection test.
///
/// # Errors
///
/// Returns the transport's phrased failure, or the parse error when the
/// response shape is foreign.
pub fn whoami(transport: &dyn GithubTransport, token: &str) -> Result<GithubAccount, String> {
    #[derive(Deserialize)]
    struct User {
        login: String,
    }
    let body = transport.get(&format!("{API_BASE}/user"), Some(token))?;
    let user: User = parse(&body)?;
    Ok(GithubAccount { login: user.login })
}

/// Every open pull request of the repository, newest first
/// (`GET /repos/{owner}/{repo}/pulls`).
///
/// # Errors
///
/// See [`whoami`].
pub fn open_pulls(
    transport: &dyn GithubTransport,
    remote: &GithubRemote,
    token: &str,
) -> Result<Vec<PrSummary>, String> {
    #[derive(Deserialize)]
    struct Head {
        #[serde(rename = "ref")]
        branch: String,
    }
    #[derive(Deserialize)]
    struct Pull {
        number: u64,
        #[serde(default)]
        draft: bool,
        html_url: String,
        head: Head,
    }
    let url = format!(
        "{API_BASE}/repos/{}/{}/pulls?state=open&per_page=100",
        remote.owner, remote.repo
    );
    let pulls: Vec<Pull> = parse(&transport.get(&url, Some(token))?)?;
    Ok(pulls
        .into_iter()
        .map(|pull| PrSummary {
            number: pull.number,
            draft: pull.draft,
            head_branch: pull.head.branch,
            html_url: pull.html_url,
        })
        .collect())
}

/// The check runs attached to one commit — GitHub Actions jobs appear here
/// (`GET /repos/{owner}/{repo}/commits/{sha}/check-runs`).
///
/// # Errors
///
/// See [`whoami`].
pub fn check_runs(
    transport: &dyn GithubTransport,
    remote: &GithubRemote,
    token: &str,
    sha: &str,
) -> Result<Vec<CheckRun>, String> {
    #[derive(Deserialize)]
    struct Run {
        name: String,
        status: String,
        conclusion: Option<String>,
        html_url: String,
    }
    #[derive(Deserialize)]
    struct Page {
        check_runs: Vec<Run>,
    }
    let url = format!(
        "{API_BASE}/repos/{}/{}/commits/{sha}/check-runs?per_page=100",
        remote.owner, remote.repo
    );
    let page: Page = parse(&transport.get(&url, Some(token))?)?;
    Ok(page
        .check_runs
        .into_iter()
        .map(|run| CheckRun {
            name: run.name,
            status: status(&run.status, run.conclusion.as_deref()),
            html_url: run.html_url,
        })
        .collect())
}

/// Recent Actions workflow runs, newest first
/// (`GET /repos/{owner}/{repo}/actions/runs`).
///
/// # Errors
///
/// See [`whoami`].
pub fn workflow_runs(
    transport: &dyn GithubTransport,
    remote: &GithubRemote,
    token: &str,
    count: u8,
) -> Result<Vec<WorkflowRun>, String> {
    #[derive(Deserialize)]
    struct Actor {
        login: String,
    }
    #[derive(Deserialize)]
    #[expect(
        clippy::struct_field_names,
        reason = "field names mirror GitHub's JSON exactly"
    )]
    struct Run {
        id: u64,
        name: String,
        display_title: String,
        run_number: u64,
        event: String,
        actor: Option<Actor>,
        head_branch: String,
        head_sha: String,
        status: String,
        conclusion: Option<String>,
        run_started_at: Option<String>,
        updated_at: Option<String>,
        html_url: String,
    }
    #[derive(Deserialize)]
    struct Page {
        workflow_runs: Vec<Run>,
    }
    let url = format!(
        "{API_BASE}/repos/{}/{}/actions/runs?per_page={count}",
        remote.owner, remote.repo
    );
    let page: Page = parse(&transport.get(&url, Some(token))?)?;
    Ok(page
        .workflow_runs
        .into_iter()
        .map(|run| {
            let state = status(&run.status, run.conclusion.as_deref());
            WorkflowRun {
                id: run.id,
                name: run.name,
                display_title: run.display_title,
                run_number: run.run_number,
                event: run.event,
                actor: run.actor.map(|actor| actor.login).unwrap_or_default(),
                branch: run.head_branch,
                sha: run.head_sha,
                status: state,
                started_at: run.run_started_at.as_deref().and_then(parse_iso8601),
                // Runs have no completion field; the last update is it once
                // the run has completed.
                completed_at: matches!(state, CheckStatus::Completed(_))
                    .then(|| run.updated_at.as_deref().and_then(parse_iso8601))
                    .flatten(),
                html_url: run.html_url,
            }
        })
        .collect())
}

/// The rolled-up CI state of many commits in one GraphQL request
/// (`statusCheckRollup`), far cheaper than per-commit check-run reads.
/// Commits GitHub does not know, or that have no checks, are simply absent
/// from the result.
///
/// # Errors
///
/// See [`whoami`].
pub fn commit_states(
    transport: &dyn GithubTransport,
    remote: &GithubRemote,
    token: &str,
    oids: &[Oid],
) -> Result<HashMap<Oid, CheckStatus>, String> {
    use std::fmt::Write as _;

    #[derive(Deserialize)]
    struct Rollup {
        state: String,
    }
    #[derive(Deserialize)]
    struct Object {
        #[serde(rename = "statusCheckRollup")]
        rollup: Option<Rollup>,
    }
    #[derive(Deserialize)]
    struct Data {
        repository: Option<HashMap<String, Option<Object>>>,
    }
    #[derive(Deserialize)]
    struct Response {
        data: Option<Data>,
    }

    if oids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut query = format!(
        "query {{ repository(owner: \"{}\", name: \"{}\") {{",
        remote.owner, remote.repo
    );
    for (index, oid) in oids.iter().enumerate() {
        let _ = write!(
            query,
            " c{index}: object(oid: \"{}\") {{ ... on Commit {{ statusCheckRollup {{ state }} }} }}",
            oid.to_hex()
        );
    }
    query.push_str(" } }");
    let body = serde_json::json!({ "query": query }).to_string();

    let raw = transport.post(&format!("{API_BASE}/graphql"), Some(token), &body)?;
    let response: Response = parse(&raw)?;
    let objects = response
        .data
        .and_then(|data| data.repository)
        .unwrap_or_default();

    let mut states = HashMap::new();
    for (alias, object) in objects {
        let Some(rollup) = object.and_then(|object| object.rollup) else {
            continue;
        };
        let Some(index) = alias
            .strip_prefix('c')
            .and_then(|digits| digits.parse::<usize>().ok())
        else {
            continue;
        };
        let Some(oid) = oids.get(index) else {
            continue;
        };
        let state = match rollup.state.as_str() {
            "SUCCESS" => CheckStatus::Completed(CheckConclusion::Success),
            "FAILURE" | "ERROR" => CheckStatus::Completed(CheckConclusion::Failure),
            "PENDING" => CheckStatus::InProgress,
            "EXPECTED" => CheckStatus::Queued,
            _ => CheckStatus::Completed(CheckConclusion::Unknown),
        };
        states.insert(*oid, state);
    }
    Ok(states)
}

/// The jobs of one run, with their steps and timing
/// (`GET /repos/{owner}/{repo}/actions/runs/{run_id}/jobs`).
///
/// # Errors
///
/// See [`whoami`].
pub fn run_jobs(
    transport: &dyn GithubTransport,
    remote: &GithubRemote,
    token: &str,
    run_id: u64,
) -> Result<Vec<WorkflowJob>, String> {
    #[derive(Deserialize)]
    struct Step {
        name: String,
        status: String,
        conclusion: Option<String>,
        started_at: Option<String>,
        completed_at: Option<String>,
    }
    #[derive(Deserialize)]
    struct Job {
        id: u64,
        name: String,
        status: String,
        conclusion: Option<String>,
        started_at: Option<String>,
        completed_at: Option<String>,
        steps: Option<Vec<Step>>,
        html_url: String,
    }
    #[derive(Deserialize)]
    struct Page {
        jobs: Vec<Job>,
    }
    let url = format!(
        "{API_BASE}/repos/{}/{}/actions/runs/{run_id}/jobs?per_page=100",
        remote.owner, remote.repo
    );
    let page: Page = parse(&transport.get(&url, Some(token))?)?;
    Ok(page
        .jobs
        .into_iter()
        .map(|job| WorkflowJob {
            id: job.id,
            name: job.name,
            status: status(&job.status, job.conclusion.as_deref()),
            started_at: job.started_at.as_deref().and_then(parse_iso8601),
            completed_at: job.completed_at.as_deref().and_then(parse_iso8601),
            steps: job
                .steps
                .unwrap_or_default()
                .into_iter()
                .map(|step| WorkflowStep {
                    name: step.name,
                    status: status(&step.status, step.conclusion.as_deref()),
                    started_at: step.started_at.as_deref().and_then(parse_iso8601),
                    completed_at: step.completed_at.as_deref().and_then(parse_iso8601),
                })
                .collect(),
            html_url: job.html_url,
        })
        .collect())
}

/// One job's complete plaintext log, timestamps included
/// (`GET /repos/{owner}/{repo}/actions/jobs/{job_id}/logs`, via redirect).
///
/// # Errors
///
/// See [`whoami`].
pub fn job_log(
    transport: &dyn GithubTransport,
    remote: &GithubRemote,
    token: &str,
    job_id: u64,
) -> Result<String, String> {
    let url = format!(
        "{API_BASE}/repos/{}/{}/actions/jobs/{job_id}/logs",
        remote.owner, remote.repo
    );
    let bytes = transport.download(&url, Some(token))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// GitHub's two-field status/conclusion pair as one typed state.
fn status(status: &str, conclusion: Option<&str>) -> CheckStatus {
    match status {
        "queued" | "waiting" | "requested" | "pending" => CheckStatus::Queued,
        "in_progress" => CheckStatus::InProgress,
        _ => CheckStatus::Completed(match conclusion {
            Some("success") => CheckConclusion::Success,
            Some("failure") => CheckConclusion::Failure,
            Some("neutral") => CheckConclusion::Neutral,
            Some("cancelled") => CheckConclusion::Cancelled,
            Some("skipped") => CheckConclusion::Skipped,
            Some("timed_out") => CheckConclusion::TimedOut,
            Some("action_required") => CheckConclusion::ActionRequired,
            _ => CheckConclusion::Unknown,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use sourcefour_model::{CheckConclusion, CheckStatus};

    use super::{check_runs, open_pulls, whoami, workflow_runs};
    use crate::{remote::GithubRemote, transport::GithubTransport};

    /// Answers every GET with one canned body, recording the request.
    struct Fake {
        body: &'static str,
        seen: RefCell<Vec<(String, Option<String>)>>,
    }

    impl Fake {
        fn new(body: &'static str) -> Self {
            Self {
                body,
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl GithubTransport for Fake {
        fn get(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, String> {
            self.seen
                .borrow_mut()
                .push((url.to_owned(), token.map(str::to_owned)));
            Ok(self.body.as_bytes().to_vec())
        }

        fn post(&self, url: &str, token: Option<&str>, body: &str) -> Result<Vec<u8>, String> {
            self.seen
                .borrow_mut()
                .push((format!("{url} {body}"), token.map(str::to_owned)));
            Ok(self.body.as_bytes().to_vec())
        }

        fn download(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, String> {
            self.seen
                .borrow_mut()
                .push((url.to_owned(), token.map(str::to_owned)));
            Ok(self.body.as_bytes().to_vec())
        }
    }

    fn remote() -> GithubRemote {
        GithubRemote {
            owner: String::from("HelgeSverre"),
            repo: String::from("sourcefour"),
        }
    }

    #[test]
    fn whoami_reads_the_login_and_authenticates() -> Result<(), String> {
        let fake = Fake::new(r#"{"login": "HelgeSverre", "id": 1, "type": "User"}"#);

        let account = whoami(&fake, "ghp_token")?;

        assert_eq!(account.login, "HelgeSverre");
        let seen = fake.seen.borrow();
        assert_eq!(seen[0].0, "https://api.github.com/user");
        assert_eq!(seen[0].1.as_deref(), Some("ghp_token"));
        Ok(())
    }

    #[test]
    fn open_pulls_map_by_head_branch() -> Result<(), String> {
        // Trimmed from a real GET /repos/{o}/{r}/pulls response.
        let fake = Fake::new(
            r#"[
                {
                    "number": 42,
                    "title": "Add juxtapose image diffs",
                    "draft": false,
                    "html_url": "https://github.com/HelgeSverre/sourcefour/pull/42",
                    "state": "open",
                    "head": {"ref": "feature/image-diffs", "sha": "abc123", "repo": {"name": "sourcefour"}},
                    "base": {"ref": "main"}
                },
                {
                    "number": 43,
                    "title": "WIP settings",
                    "draft": true,
                    "html_url": "https://github.com/HelgeSverre/sourcefour/pull/43",
                    "state": "open",
                    "head": {"ref": "feature/settings", "sha": "def456", "repo": null},
                    "base": {"ref": "main"}
                }
            ]"#,
        );

        let pulls = open_pulls(&fake, &remote(), "ghp_token")?;

        assert_eq!(pulls.len(), 2);
        assert_eq!(pulls[0].number, 42);
        assert_eq!(pulls[0].head_branch, "feature/image-diffs");
        assert!(!pulls[0].draft);
        assert!(pulls[1].draft);
        assert!(
            fake.seen.borrow()[0].0.starts_with(
                "https://api.github.com/repos/HelgeSverre/sourcefour/pulls?state=open"
            ),
        );
        Ok(())
    }

    #[test]
    fn check_runs_fold_status_and_conclusion() -> Result<(), String> {
        let fake = Fake::new(
            r#"{
                "total_count": 3,
                "check_runs": [
                    {"name": "gate", "status": "completed", "conclusion": "success",
                     "html_url": "https://github.com/HelgeSverre/sourcefour/runs/1"},
                    {"name": "lint", "status": "in_progress", "conclusion": null,
                     "html_url": "https://github.com/HelgeSverre/sourcefour/runs/2"},
                    {"name": "future", "status": "completed", "conclusion": "galactic",
                     "html_url": "https://github.com/HelgeSverre/sourcefour/runs/3"}
                ]
            }"#,
        );

        let runs = check_runs(&fake, &remote(), "ghp_token", "abc123")?;

        assert_eq!(runs.len(), 3);
        assert_eq!(
            runs[0].status,
            CheckStatus::Completed(CheckConclusion::Success)
        );
        assert_eq!(runs[1].status, CheckStatus::InProgress);
        assert_eq!(
            runs[2].status,
            CheckStatus::Completed(CheckConclusion::Unknown),
            "future conclusions degrade, they do not fail the parse"
        );
        assert!(
            fake.seen.borrow()[0]
                .0
                .contains("/commits/abc123/check-runs")
        );
        Ok(())
    }

    #[test]
    fn workflow_runs_carry_identity_title_and_timing() -> Result<(), String> {
        let fake = Fake::new(
            r#"{
                "total_count": 1,
                "workflow_runs": [
                    {"id": 9001, "name": "CI", "display_title": "fix: Windows paths",
                     "run_number": 128, "event": "push", "actor": {"login": "HelgeSverre"},
                     "head_branch": "main", "head_sha": "abc123",
                     "status": "completed", "conclusion": "failure",
                     "run_started_at": "2026-08-04T14:30:00Z",
                     "updated_at": "2026-08-04T14:33:42Z",
                     "html_url": "https://github.com/HelgeSverre/sourcefour/actions/runs/9001"}
                ]
            }"#,
        );

        let runs = workflow_runs(&fake, &remote(), "ghp_token", 25)?;

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, 9001);
        assert_eq!(runs[0].display_title, "fix: Windows paths");
        assert_eq!(runs[0].event, "push");
        assert_eq!(runs[0].actor, "HelgeSverre");
        assert_eq!(runs[0].run_number, 128);
        assert_eq!(
            runs[0].status,
            CheckStatus::Completed(CheckConclusion::Failure)
        );
        assert_eq!(
            runs[0].completed_at.unwrap() - runs[0].started_at.unwrap(),
            222,
            "run wall time comes from started/updated"
        );
        assert!(fake.seen.borrow()[0].0.ends_with("per_page=25"));
        Ok(())
    }

    #[test]
    fn run_jobs_carry_steps_and_timing() -> Result<(), String> {
        let fake = Fake::new(
            r#"{
                "total_count": 2,
                "jobs": [
                    {"id": 71, "name": "quality (windows-latest)", "status": "completed",
                     "conclusion": "failure",
                     "started_at": "2026-08-04T14:30:03Z", "completed_at": "2026-08-04T14:32:01Z",
                     "html_url": "https://github.com/HelgeSverre/sourcefour/runs/71",
                     "steps": [
                        {"name": "Checkout", "status": "completed", "conclusion": "success",
                         "started_at": "2026-08-04T14:30:04Z", "completed_at": "2026-08-04T14:30:08Z"},
                        {"name": "cargo clippy", "status": "completed", "conclusion": "failure",
                         "started_at": "2026-08-04T14:30:59Z", "completed_at": "2026-08-04T14:32:01Z"}
                     ]},
                    {"id": 72, "name": "package", "status": "completed", "conclusion": "skipped",
                     "started_at": null, "completed_at": null,
                     "html_url": "https://github.com/HelgeSverre/sourcefour/runs/72",
                     "steps": null}
                ]
            }"#,
        );

        let jobs = super::run_jobs(&fake, &remote(), "ghp_token", 9001)?;

        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].name, "quality (windows-latest)");
        assert_eq!(
            jobs[0].status,
            CheckStatus::Completed(CheckConclusion::Failure)
        );
        assert_eq!(jobs[0].steps.len(), 2);
        assert_eq!(
            jobs[0].steps[1].completed_at.unwrap() - jobs[0].steps[1].started_at.unwrap(),
            62
        );
        assert!(jobs[1].steps.is_empty(), "null steps read as none");
        assert!(
            fake.seen.borrow()[0].0.contains("/actions/runs/9001/jobs"),
            "the run id names the jobs endpoint"
        );
        Ok(())
    }

    #[test]
    fn commit_states_batch_and_map_by_alias() -> Result<(), String> {
        let fake = Fake::new(
            r#"{"data": {"repository": {
                "c0": {"statusCheckRollup": {"state": "SUCCESS"}},
                "c1": {"statusCheckRollup": {"state": "PENDING"}},
                "c2": {"statusCheckRollup": null},
                "c3": null
            }}}"#,
        );
        let oids = [
            sourcefour_model::Oid::sha1([0x11; 20]),
            sourcefour_model::Oid::sha1([0x22; 20]),
            sourcefour_model::Oid::sha1([0x33; 20]),
            sourcefour_model::Oid::sha1([0x44; 20]),
        ];

        let states = super::commit_states(&fake, &remote(), "ghp_token", &oids)?;

        assert_eq!(
            states.get(&oids[0]),
            Some(&CheckStatus::Completed(CheckConclusion::Success))
        );
        assert_eq!(states.get(&oids[1]), Some(&CheckStatus::InProgress));
        assert_eq!(states.get(&oids[2]), None, "no checks means no dot");
        assert_eq!(states.get(&oids[3]), None, "unknown commits are absent");
        let seen = fake.seen.borrow();
        assert!(seen[0].0.starts_with("https://api.github.com/graphql"));
        assert!(
            seen[0].0.contains(&oids[3].to_hex()),
            "every oid is asked for"
        );
        Ok(())
    }

    #[test]
    fn job_logs_download_from_the_jobs_endpoint() -> Result<(), String> {
        let fake = Fake::new("2026-08-04T14:32:41.0000000Z error: it broke\n");

        let log = super::job_log(&fake, &remote(), "ghp_token", 71)?;

        assert!(log.contains("error: it broke"));
        assert!(
            fake.seen.borrow()[0]
                .0
                .ends_with("/repos/HelgeSverre/sourcefour/actions/jobs/71/logs")
        );
        Ok(())
    }

    #[test]
    fn a_foreign_response_shape_is_a_parse_failure() {
        let fake = Fake::new(r#"{"message": "Bad credentials"}"#);

        let result = open_pulls(&fake, &remote(), "ghp_token");

        assert!(result.is_err(), "an object where a list belongs must fail");
    }
}
