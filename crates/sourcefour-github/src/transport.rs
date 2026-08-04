//! The HTTP boundary, behind a trait so tests never open a socket (§12).
//!
//! Failures are the words the interface shows; no caller branches on a
//! failure kind today, so there is none to carry.

/// Authenticated requests returning the raw response body.
pub trait GithubTransport {
    /// Fetches `url` with the standard GitHub headers.
    ///
    /// # Errors
    ///
    /// Returns the user-facing words for transport errors and non-2xx
    /// statuses.
    fn get(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, String>;

    /// Posts a JSON `body` to `url` — the GraphQL endpoint's shape.
    ///
    /// # Errors
    ///
    /// See [`GithubTransport::get`].
    fn post(&self, url: &str, token: Option<&str>, body: &str) -> Result<Vec<u8>, String>;

    /// Fetches a resource that answers with a redirect to a signed URL —
    /// the log-download shape. The signed target is fetched bare, because
    /// forwarding Authorization makes the blob store reject the request.
    ///
    /// # Errors
    ///
    /// See [`GithubTransport::get`].
    fn download(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, String>;
}

/// The shipping transport. Blocking by design: callers run it on the
/// background executor exactly like git subprocesses.
pub struct UreqTransport;

impl GithubTransport for UreqTransport {
    fn get(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, String> {
        read_response(prepared(ureq::get(url), token).call())
    }

    fn post(&self, url: &str, token: Option<&str>, body: &str) -> Result<Vec<u8>, String> {
        read_response(prepared(ureq::post(url), token).send_string(body))
    }

    fn download(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, String> {
        let agent = ureq::AgentBuilder::new().redirects(0).build();
        match prepared(agent.get(url), token).call() {
            Ok(response) if (300..400).contains(&response.status()) => {
                let Some(target) = response.header("location").map(str::to_owned) else {
                    return Err(String::from("GitHub redirected without a target."));
                };
                read_response(agent.get(&target).call())
            }
            other => read_response(other),
        }
    }
}

/// The standard GitHub headers on any request.
fn prepared(request: ureq::Request, token: Option<&str>) -> ureq::Request {
    let mut request = request
        .set("User-Agent", "sourcefour")
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .timeout(std::time::Duration::from_secs(15));
    if let Some(token) = token {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    request
}

/// Collects a response body, phrasing failures for the interface.
fn read_response(result: Result<ureq::Response, ureq::Error>) -> Result<Vec<u8>, String> {
    match result {
        Ok(response) => {
            let mut body = Vec::new();
            response
                .into_reader()
                .read_to_end(&mut body)
                .map_err(|error| error.to_string())?;
            Ok(body)
        }
        Err(ureq::Error::Status(status, response)) => Err(classify_status(
            status,
            response.header("x-ratelimit-remaining"),
        )),
        Err(transport) => Err(transport.to_string()),
    }
}

/// Phrases a non-2xx status for the interface.
fn classify_status(status: u16, ratelimit_remaining: Option<&str>) -> String {
    match status {
        401 => String::from("GitHub rejected the credentials (401)."),
        403 if ratelimit_remaining == Some("0") => {
            String::from("The GitHub API rate limit is exhausted; try again later.")
        }
        403 => String::from("GitHub refused access (403); the token may lack scopes."),
        404 => String::from("GitHub reports no such resource (404)."),
        // Asking for the checks of a commit GitHub has never seen.
        422 => String::from("Not on GitHub yet."),
        other => format!("GitHub answered with status {other}."),
    }
}

#[cfg(test)]
mod tests {
    use super::classify_status;

    #[test]
    fn statuses_phrase_for_the_interface() {
        assert!(classify_status(401, None).contains("credentials"));
        assert!(classify_status(403, Some("0")).contains("rate limit"));
        assert!(classify_status(403, Some("55")).contains("scopes"));
        assert!(classify_status(404, None).contains("no such resource"));
        assert!(classify_status(422, None).contains("Not on GitHub"));
        assert!(classify_status(500, None).contains("500"));
    }
}
