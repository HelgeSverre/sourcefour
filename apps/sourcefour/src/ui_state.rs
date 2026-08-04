//! Persisted interface state: panel sizes, section order, collapse state.
//!
//! One tolerant JSON file per user. Unknown fields are ignored and missing
//! fields fall back to defaults, so older and newer builds can share it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Everything the interface remembers between launches.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct UiState {
    pub(crate) sidebar_width: Option<f32>,
    pub(crate) graph_width: Option<f32>,
    pub(crate) details_height: Option<f32>,
    /// Section names in display order; ignored unless it names all sections.
    pub(crate) section_order: Option<Vec<String>>,
    pub(crate) collapsed_sections: Vec<String>,
    /// "unified" or "split".
    pub(crate) diff_mode: Option<String>,
    pub(crate) details_collapsed: bool,
}

impl UiState {
    /// Reads persisted state, falling back to defaults on any failure: a
    /// missing or corrupt state file must never block the window.
    pub(crate) fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default()
    }

    /// Writes the state, creating the directory on first save.
    ///
    /// # Errors
    ///
    /// Returns the underlying error when the file cannot be written; callers
    /// log it, because losing interface state must never interrupt the user.
    pub(crate) fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// Loads from the default per-user location.
    pub(crate) fn load() -> Self {
        default_path().map_or_else(Self::default, |path| Self::load_from(&path))
    }

    /// Saves to the default per-user location, logging failures.
    pub(crate) fn save(&self) {
        let Some(path) = default_path() else {
            return;
        };
        if let Err(error) = self.save_to(&path) {
            tracing::warn!(%error, "interface state could not be saved");
        }
    }
}

/// `~/Library/Application Support/Sourcefour/state.json` on macOS.
fn default_path() -> Option<PathBuf> {
    support_file("state.json")
}

/// The user's home directory. Windows sets `USERPROFILE` where Unix sets
/// `HOME`.
pub(crate) fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// A file in the app's per-user support directory,
/// `~/Library/Application Support/Sourcefour/` on macOS.
pub(crate) fn support_file(name: &str) -> Option<PathBuf> {
    Some(
        home_directory()?
            .join("Library")
            .join("Application Support")
            .join("Sourcefour")
            .join(name),
    )
}

#[cfg(test)]
mod tests {
    use super::UiState;

    #[test]
    fn state_round_trips_through_its_file() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("nested").join("state.json");
        let state = UiState {
            sidebar_width: Some(310.0),
            graph_width: Some(140.0),
            details_height: Some(200.0),
            section_order: Some(vec![
                String::from("remotes"),
                String::from("worktrees"),
                String::from("branches"),
            ]),
            collapsed_sections: vec![String::from("branches")],
            diff_mode: Some(String::from("split")),
            details_collapsed: true,
        };

        state.save_to(&path)?;
        let loaded = UiState::load_from(&path);

        assert_eq!(loaded, state, "the parent directory is created on demand");
        Ok(())
    }

    #[test]
    fn a_missing_or_broken_file_yields_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;

        let absent = UiState::load_from(&directory.path().join("nowhere.json"));
        assert_eq!(absent, UiState::default());

        let broken = directory.path().join("broken.json");
        std::fs::write(&broken, "{ not json")?;
        assert_eq!(UiState::load_from(&broken), UiState::default());
        Ok(())
    }

    #[test]
    fn unknown_fields_from_a_newer_build_are_tolerated() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("state.json");
        std::fs::write(
            &path,
            r#"{"sidebar_width": 250.0, "hologram_mode": true, "collapsed_sections": ["remotes"]}"#,
        )?;

        let loaded = UiState::load_from(&path);

        assert_eq!(loaded.sidebar_width, Some(250.0));
        assert_eq!(loaded.collapsed_sections, vec![String::from("remotes")]);
        assert_eq!(loaded.section_order, None, "missing fields default");
        Ok(())
    }
}
