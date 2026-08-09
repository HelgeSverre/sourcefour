//! Persisted interface state: panel sizes, section order, collapse state.
//!
//! One tolerant JSON file per user. Unknown fields are ignored and missing
//! fields fall back to defaults, so older and newer builds can share it; see
//! [`crate::persist`] for both halves of that.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Last reported restored size and display mode of the main window.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct WindowState {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) mode: WindowMode,
}

/// How the window was displayed when its state was recorded.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WindowMode {
    #[default]
    Windowed,
    Maximized,
    Fullscreen,
}

/// Everything the interface remembers between launches.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct UiState {
    pub(crate) window: Option<WindowState>,
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
    /// Reads persisted state; see [`crate::persist::load_json_or_default`]
    /// for the contract.
    pub(crate) fn load_from(path: &Path) -> Self {
        crate::persist::load_json_or_default(path)
    }

    /// Writes the state; see [`crate::persist::save_json_pretty`].
    ///
    /// # Errors
    ///
    /// See [`crate::persist::save_json_pretty`].
    pub(crate) fn save_to(&self, path: &Path) -> std::io::Result<()> {
        crate::persist::save_json_pretty(path, self)
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
    crate::persist::support_file("state.json")
}

#[cfg(test)]
mod tests {
    use super::{UiState, WindowMode, WindowState};

    #[test]
    fn state_round_trips_through_its_file() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("nested").join("state.json");
        let state = UiState {
            window: Some(WindowState {
                width: 1440.0,
                height: 900.0,
                mode: WindowMode::Maximized,
            }),
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

    #[test]
    fn positions_from_older_state_files_are_ignored() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("state.json");
        std::fs::write(
            &path,
            r#"{"window":{"x":2400.0,"y":120.0,"width":1280.0,"height":800.0,"mode":"windowed"}}"#,
        )?;

        assert_eq!(
            UiState::load_from(&path).window,
            Some(WindowState {
                width: 1280.0,
                height: 800.0,
                mode: WindowMode::Windowed,
            })
        );
        Ok(())
    }
}
