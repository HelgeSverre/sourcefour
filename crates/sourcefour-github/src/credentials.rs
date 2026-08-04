//! Token storage: a `0600` file, the same fallback `gh` uses.
//!
//! The macOS Keychain binds access to the code signature, and ad-hoc-signed
//! development builds change identity every rebuild; Keychain storage waits
//! for Developer ID signing. Tokens never appear in settings.json or logs.

use std::{collections::BTreeMap, path::Path};

/// The file's whole shape: one token per host.
type Store = BTreeMap<String, HostCredentials>;

#[derive(serde::Deserialize, serde::Serialize)]
struct HostCredentials {
    token: String,
}

/// Reads the token stored for `host`, `None` when absent or unreadable.
#[must_use]
pub fn load_token(path: &Path, host: &str) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let mut store: Store = serde_json::from_str(&contents).ok()?;
    Some(store.remove(host)?.token)
}

/// Writes the token for `host`, creating the file `0600` on first use.
///
/// # Errors
///
/// Returns the underlying error when the file cannot be written.
pub fn store_token(path: &Path, host: &str, token: &str) -> std::io::Result<()> {
    let mut store: Store = std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default();
    store.insert(
        host.to_owned(),
        HostCredentials {
            token: token.to_owned(),
        },
    );
    write_private(
        path,
        &serde_json::to_string_pretty(&store).map_err(std::io::Error::other)?,
    )
}

/// Removes the token for `host`; absence is success.
///
/// # Errors
///
/// Returns the underlying error when the file cannot be rewritten.
pub fn delete_token(path: &Path, host: &str) -> std::io::Result<()> {
    let Some(contents) = std::fs::read_to_string(path).ok() else {
        return Ok(());
    };
    let mut store: Store = serde_json::from_str(&contents).unwrap_or_default();
    store.remove(host);
    write_private(
        path,
        &serde_json::to_string_pretty(&store).map_err(std::io::Error::other)?,
    )
}

/// Writes with owner-only permissions, creating parents as needed.
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents.as_bytes())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::{delete_token, load_token, store_token};

    #[test]
    fn tokens_round_trip_per_host() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("nested").join("credentials.json");

        store_token(&path, "github.com", "ghp_first")?;
        store_token(&path, "github.example.com", "ghp_second")?;

        assert_eq!(
            load_token(&path, "github.com").as_deref(),
            Some("ghp_first")
        );
        assert_eq!(
            load_token(&path, "github.example.com").as_deref(),
            Some("ghp_second")
        );
        assert_eq!(load_token(&path, "unknown.host"), None);
        Ok(())
    }

    #[test]
    fn the_file_is_owner_only() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("credentials.json");

        store_token(&path, "github.com", "ghp_secret")?;

        let mode = std::fs::metadata(&path)?.permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "tokens are not world-readable");
        Ok(())
    }

    #[test]
    fn deleting_removes_only_that_host_and_tolerates_absence()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("credentials.json");

        delete_token(&path, "github.com")?;

        store_token(&path, "github.com", "ghp_first")?;
        store_token(&path, "github.example.com", "ghp_second")?;
        delete_token(&path, "github.com")?;

        assert_eq!(load_token(&path, "github.com"), None);
        assert_eq!(
            load_token(&path, "github.example.com").as_deref(),
            Some("ghp_second")
        );
        Ok(())
    }
}
