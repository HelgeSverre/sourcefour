//! Typed REST calls, each one URL and one serde shape.

use serde::Deserialize;
use std::collections::HashMap;

use sourcefour_model::{
    CheckConclusion, CheckRun, CheckStatus, GithubAccount, Oid, PrSummary, WorkflowRun,
};

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
    #[expect(
        clippy::struct_field_names,
        reason = "field names mirror GitHub's JSON exactly"
    )]
    struct Run {
        name: String,
        run_number: u64,
        head_branch: String,
        head_sha: String,
        status: String,
        conclusion: Option<String>,
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
        .map(|run| WorkflowRun {
            name: run.name,
            run_number: run.run_number,
            branch: run.head_branch,
            sha: run.head_sha,
            status: status(&run.status, run.conclusion.as_deref()),
            html_url: run.html_url,
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
    fn workflow_runs_carry_branch_and_number() -> Result<(), String> {
        let fake = Fake::new(
            r#"{
                "total_count": 1,
                "workflow_runs": [
                    {"name": "CI", "run_number": 128, "head_branch": "main",
                     "head_sha": "abc123", "status": "completed", "conclusion": "failure",
                     "html_url": "https://github.com/HelgeSverre/sourcefour/actions/runs/9"}
                ]
            }"#,
        );

        let runs = workflow_runs(&fake, &remote(), "ghp_token", 25)?;

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].name, "CI");
        assert_eq!(runs[0].run_number, 128);
        assert_eq!(runs[0].branch, "main");
        assert_eq!(
            runs[0].status,
            CheckStatus::Completed(CheckConclusion::Failure)
        );
        assert!(fake.seen.borrow()[0].0.ends_with("per_page=25"));
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
    fn a_foreign_response_shape_is_a_parse_failure() {
        let fake = Fake::new(r#"{"message": "Bad credentials"}"#);

        let result = open_pulls(&fake, &remote(), "ghp_token");

        assert!(result.is_err(), "an object where a list belongs must fail");
    }
}
