//! The HTTP boundary, behind a trait so tests never open a socket (§12).
//!
//! Failures are the words the interface shows; no caller branches on a
//! failure kind today, so there is none to carry.

/// One authenticated GET returning the raw response body.
pub trait GithubTransport {
    /// Fetches `url` with the standard GitHub headers.
    ///
    /// # Errors
    ///
    /// Returns the user-facing words for transport errors and non-2xx
    /// statuses.
    fn get(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, String>;
}

/// The shipping transport. Blocking by design: callers run it on the
/// background executor exactly like git subprocesses.
pub struct UreqTransport;

impl GithubTransport for UreqTransport {
    fn get(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, String> {
        let mut request = ureq::get(url)
            .set("User-Agent", "sourcefour")
            .set("Accept", "application/vnd.github+json")
            .set("X-GitHub-Api-Version", "2022-11-28")
            .timeout(std::time::Duration::from_secs(15));
        if let Some(token) = token {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        match request.call() {
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
        assert!(classify_status(500, None).contains("500"));
    }
}
