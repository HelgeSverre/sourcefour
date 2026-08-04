//! Mapping a git remote URL onto a GitHub repository identity.

/// The GitHub repository a remote points at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GithubRemote {
    /// The web host, `github.com` for now.
    pub host: String,
    pub owner: String,
    pub repo: String,
}

impl GithubRemote {
    /// Reads `owner/repo` out of the ssh and https remote URL forms Git
    /// produces for github.com. Anything else — other hosts, local paths —
    /// is `None`: not an error, just not GitHub.
    #[must_use]
    pub fn parse(url: &str) -> Option<Self> {
        if let Some(rest) = url.strip_prefix("git@") {
            // The scp-like form: git@host:owner/repo(.git)
            let (host, path) = rest.split_once(':')?;
            return Self::from_parts(host, path);
        }
        let rest = url
            .strip_prefix("ssh://git@")
            .or_else(|| url.strip_prefix("https://"))
            .or_else(|| url.strip_prefix("http://"))?;
        let (host, path) = rest.split_once('/')?;
        Self::from_parts(host, path)
    }

    fn from_parts(host: &str, path: &str) -> Option<Self> {
        if host != "github.com" {
            return None;
        }
        let path = path.trim_end_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let mut segments = path.split('/');
        let owner = segments.next()?;
        let repo = segments.next()?;
        if owner.is_empty() || repo.is_empty() || segments.next().is_some() {
            return None;
        }
        Some(Self {
            host: host.to_owned(),
            owner: owner.to_owned(),
            repo: repo.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::GithubRemote;

    fn remote(owner: &str, repo: &str) -> GithubRemote {
        GithubRemote {
            host: String::from("github.com"),
            owner: owner.to_owned(),
            repo: repo.to_owned(),
        }
    }

    #[test]
    fn github_remote_urls_parse_in_every_git_form() {
        for url in [
            "git@github.com:HelgeSverre/sourcefour.git",
            "git@github.com:HelgeSverre/sourcefour",
            "ssh://git@github.com/HelgeSverre/sourcefour.git",
            "https://github.com/HelgeSverre/sourcefour.git",
            "https://github.com/HelgeSverre/sourcefour",
            "https://github.com/HelgeSverre/sourcefour/",
            "http://github.com/HelgeSverre/sourcefour.git",
        ] {
            assert_eq!(
                GithubRemote::parse(url),
                Some(remote("HelgeSverre", "sourcefour")),
                "{url} names the repository"
            );
        }
    }

    #[test]
    fn non_github_urls_are_none_not_errors() {
        for url in [
            "git@gitlab.com:group/project.git",
            "https://gitlab.com/group/project.git",
            "https://github.com/only-owner",
            "https://github.com/",
            "/Users/someone/code/local-repo",
            "file:///tmp/repo.git",
            "",
        ] {
            assert_eq!(GithubRemote::parse(url), None, "{url:?} is not GitHub");
        }
    }

    #[test]
    fn deep_paths_keep_only_owner_and_repo() {
        // Enterprise-style deep paths are out of scope for v1; the two-segment
        // form is the contract.
        assert_eq!(
            GithubRemote::parse("https://github.com/a/b/c"),
            None,
            "three segments do not silently truncate"
        );
    }
}
