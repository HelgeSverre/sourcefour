//! The HTTP boundary, behind a trait so tests never open a socket (§12).

/// Why an API call failed, coarse enough for the interface to phrase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiFailureKind {
    /// Missing, expired, or under-scoped credentials (401/403).
    Auth,
    /// The rate limit is exhausted; try later.
    RateLimited,
    /// The repository or object does not exist — or the token cannot see it.
    NotFound,
    /// The network itself failed.
    Network,
    /// The response was not what this build understands.
    Protocol,
}

/// A failed API call: a kind to branch on and words to show.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiFailure {
    pub kind: ApiFailureKind,
    pub message: String,
}

impl ApiFailure {
    pub(crate) fn new(kind: ApiFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// One authenticated GET returning the raw response body.
pub trait GithubTransport {
    /// Fetches `url` with the standard GitHub headers.
    ///
    /// # Errors
    ///
    /// Returns a classified [`ApiFailure`] for transport errors and non-2xx
    /// statuses.
    fn get(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, ApiFailure>;
}

/// The shipping transport. Blocking by design: callers run it on the
/// background executor exactly like git subprocesses.
pub struct UreqTransport;

impl GithubTransport for UreqTransport {
    fn get(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, ApiFailure> {
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
                    .map_err(|error| ApiFailure::new(ApiFailureKind::Network, error.to_string()))?;
                Ok(body)
            }
            Err(ureq::Error::Status(status, response)) => Err(classify_status(
                status,
                response.header("x-ratelimit-remaining"),
            )),
            Err(transport) => Err(ApiFailure::new(
                ApiFailureKind::Network,
                transport.to_string(),
            )),
        }
    }
}

/// Maps a non-2xx status onto the failure the interface should phrase.
fn classify_status(status: u16, ratelimit_remaining: Option<&str>) -> ApiFailure {
    match status {
        401 => ApiFailure::new(
            ApiFailureKind::Auth,
            "GitHub rejected the credentials (401).",
        ),
        403 if ratelimit_remaining == Some("0") => ApiFailure::new(
            ApiFailureKind::RateLimited,
            "The GitHub API rate limit is exhausted; try again later.",
        ),
        403 => ApiFailure::new(
            ApiFailureKind::Auth,
            "GitHub refused access (403); the token may lack scopes.",
        ),
        404 => ApiFailure::new(
            ApiFailureKind::NotFound,
            "GitHub reports no such resource (404).",
        ),
        other => ApiFailure::new(
            ApiFailureKind::Protocol,
            format!("GitHub answered with status {other}."),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiFailureKind, classify_status};

    #[test]
    fn statuses_classify_for_the_interface() {
        assert_eq!(classify_status(401, None).kind, ApiFailureKind::Auth);
        assert_eq!(
            classify_status(403, Some("0")).kind,
            ApiFailureKind::RateLimited
        );
        assert_eq!(classify_status(403, Some("55")).kind, ApiFailureKind::Auth);
        assert_eq!(classify_status(404, None).kind, ApiFailureKind::NotFound);
        assert_eq!(classify_status(500, None).kind, ApiFailureKind::Protocol);
    }
}
