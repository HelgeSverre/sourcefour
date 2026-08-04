//! User settings: deliberate configuration, kept apart from window state.
//!
//! Same tolerance contract as [`crate::ui_state`]: unknown fields are
//! ignored and missing fields fall back to defaults, so older and newer
//! builds can share the file. Secrets never live here.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Everything the user has configured, one section per concern.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct AppSettings {
    pub(crate) github: GithubSettings,
}

/// The GitHub integration's configuration (§ post-v1 integrations).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct GithubSettings {
    pub(crate) enabled: bool,
    pub(crate) auth_method: AuthMethod,
    /// The repository host; the REST endpoint derives from it. Reserved for
    /// GitHub Enterprise later.
    pub(crate) host: String,
}

impl Default for GithubSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            auth_method: AuthMethod::Off,
            host: String::from("github.com"),
        }
    }
}

/// How the GitHub client authenticates.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AuthMethod {
    /// A stored personal access token.
    Token,
    /// Borrow the token of an installed, signed-in `gh` CLI.
    GhCli,
    /// No authentication configured; also what unknown future values read as.
    #[default]
    #[serde(other)]
    Off,
}

impl AppSettings {
    /// Reads settings, falling back to defaults on any failure: a missing or
    /// corrupt file must never block the window.
    pub(crate) fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default()
    }

    /// Writes the settings, creating the directory on first save.
    ///
    /// # Errors
    ///
    /// Returns the underlying error when the file cannot be written; callers
    /// log it, because a failed save must never interrupt the user.
    pub(crate) fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// Loads from the default per-user location.
    pub(crate) fn load() -> Self {
        crate::ui_state::support_file("settings.json")
            .map_or_else(Self::default, |path| Self::load_from(&path))
    }

    /// Saves to the default per-user location, logging failures.
    pub(crate) fn save(&self) {
        let Some(path) = crate::ui_state::support_file("settings.json") else {
            return;
        };
        if let Err(error) = self.save_to(&path) {
            tracing::warn!(%error, "settings could not be saved");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppSettings, AuthMethod, GithubSettings};

    #[test]
    fn settings_round_trip_through_their_file() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("nested").join("settings.json");
        let settings = AppSettings {
            github: GithubSettings {
                enabled: true,
                auth_method: AuthMethod::Token,
                host: String::from("github.example.com"),
            },
        };

        settings.save_to(&path)?;
        let loaded = AppSettings::load_from(&path);

        assert_eq!(
            loaded, settings,
            "the parent directory is created on demand"
        );
        Ok(())
    }

    #[test]
    fn a_missing_or_broken_file_yields_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;

        let absent = AppSettings::load_from(&directory.path().join("nowhere.json"));
        assert_eq!(absent, AppSettings::default());
        assert_eq!(absent.github.host, "github.com");

        let broken = directory.path().join("broken.json");
        std::fs::write(&broken, "{ not json")?;
        assert_eq!(AppSettings::load_from(&broken), AppSettings::default());
        Ok(())
    }

    #[test]
    fn unknown_fields_and_auth_methods_are_tolerated() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{
                "github": {"enabled": true, "auth_method": "quantum-entanglement"},
                "holograms": {"enabled": true}
            }"#,
        )?;

        let loaded = AppSettings::load_from(&path);

        assert!(loaded.github.enabled);
        assert_eq!(
            loaded.github.auth_method,
            AuthMethod::Off,
            "a future auth method reads as Off, not a parse failure"
        );
        assert_eq!(loaded.github.host, "github.com", "missing fields default");
        Ok(())
    }
}
