//! User settings: deliberate configuration, kept apart from window state.
//!
//! Same tolerance contract as [`crate::ui_state`], both halves of it in
//! [`crate::persist`]: unknown fields are ignored on read and kept on write,
//! and missing fields fall back to defaults, so older and newer builds can
//! share the file — and so can a hand-edit. Secrets never live here.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Everything the user has configured, one section per concern.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct AppSettings {
    pub(crate) github: GithubSettings,
    pub(crate) diff: DiffSettings,
    pub(crate) video: VideoSettings,
}

/// The GitHub integration's configuration (§ post-v1 integrations).
///
/// github.com only: a host setting would be dead weight until Enterprise
/// support actually plumbs its own API base and credentials through.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct GithubSettings {
    pub(crate) enabled: bool,
    pub(crate) auth_method: AuthMethod,
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

/// The diff overlay's typography (§6.11).
///
/// `line_height` is a unitless multiplier of the mono font size, the
/// convention Zed, VS Code, and `JetBrains` all use — never a pixel value, so
/// the number stays meaningful if the font size ever changes.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct DiffSettings {
    pub(crate) line_height: f32,
}

impl Default for DiffSettings {
    fn default() -> Self {
        Self { line_height: 1.55 }
    }
}

impl DiffSettings {
    /// One diff row's height in px. 11.0 is the diff mono font size; the
    /// clamp keeps a hand-edited settings.json sane.
    pub(crate) fn row_height(&self) -> f32 {
        (11.0 * self.line_height.clamp(1.0, 3.0)).round()
    }
}

/// Where the video tools live, when the usual places are not where they are.
///
/// A directory rather than a binary: `ffmpeg` and `ffprobe` are installed
/// side by side, and naming one would leave the other still to find. Unset is
/// the normal case — the search covers `PATH` and the standard prefixes on its
/// own, and this exists for the install that is somewhere else entirely.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct VideoSettings {
    pub(crate) ffmpeg_dir: Option<std::path::PathBuf>,
}

impl AppSettings {
    /// Reads settings, falling back to defaults on any failure: a missing or
    /// corrupt file must never block the window.
    pub(crate) fn load_from(path: &Path) -> Self {
        crate::persist::load_json_or_default(path)
    }

    /// Writes the settings, creating the directory on first save.
    ///
    /// # Errors
    ///
    /// Returns the underlying error when the file cannot be written; callers
    /// log it, because a failed save must never interrupt the user.
    pub(crate) fn save_to(&self, path: &Path) -> std::io::Result<()> {
        crate::persist::save_json_pretty(path, self)
    }

    /// Loads from the default per-user location.
    pub(crate) fn load() -> Self {
        crate::persist::support_file("settings.json")
            .map_or_else(Self::default, |path| Self::load_from(&path))
    }

    /// Saves to the default per-user location, logging failures.
    pub(crate) fn save(&self) {
        let Some(path) = crate::persist::support_file("settings.json") else {
            return;
        };
        if let Err(error) = self.save_to(&path) {
            tracing::warn!(%error, "settings could not be saved");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppSettings, AuthMethod, DiffSettings, GithubSettings, VideoSettings};

    #[test]
    fn settings_round_trip_through_their_file() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("nested").join("settings.json");
        let settings = AppSettings {
            github: GithubSettings {
                enabled: true,
                auth_method: AuthMethod::Token,
            },
            diff: DiffSettings::default(),
            video: VideoSettings {
                ffmpeg_dir: Some(std::path::PathBuf::from("/opt/ffmpeg/bin")),
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
        Ok(())
    }

    #[test]
    fn diff_defaults_round_trip_through_json() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("settings.json");
        let settings = AppSettings::default();

        settings.save_to(&path)?;
        let loaded = AppSettings::load_from(&path);

        assert_eq!(loaded, settings);
        Ok(())
    }

    #[test]
    fn unknown_fields_inside_diff_are_tolerated() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("settings.json");
        std::fs::write(&path, r#"{"diff": {"line_height": 1.7, "wrap": true}}"#)?;

        let loaded = AppSettings::load_from(&path);

        assert_eq!(loaded.diff, DiffSettings { line_height: 1.7 });
        Ok(())
    }

    #[test]
    fn line_height_parses_from_a_json_document() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("settings.json");
        std::fs::write(&path, r#"{"diff": {"line_height": 1.8}}"#)?;

        let loaded = AppSettings::load_from(&path);

        assert_eq!(loaded.diff, DiffSettings { line_height: 1.8 });
        Ok(())
    }

    #[test]
    fn a_video_directory_survives_a_file_that_predates_it() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("settings.json");
        std::fs::write(&path, r#"{"diff": {"line_height": 1.7}}"#)?;
        assert_eq!(
            AppSettings::load_from(&path).video,
            VideoSettings::default(),
            "a file written before the section reads as no override"
        );

        std::fs::write(
            &path,
            r#"{"video": {"ffmpeg_dir": "/opt/ffmpeg/bin", "codec": "av1"}}"#,
        )?;
        assert_eq!(
            AppSettings::load_from(&path).video,
            VideoSettings {
                ffmpeg_dir: Some(std::path::PathBuf::from("/opt/ffmpeg/bin")),
            },
            "a future key inside the section is ignored, not fatal"
        );
        Ok(())
    }

    #[test]
    fn a_save_keeps_the_keys_this_build_has_no_field_for() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"holograms": {"on": true}, "diff": {"line_height": 1.7, "wrap": true}}"#,
        )?;

        let mut loaded = AppSettings::load_from(&path);
        loaded.diff.line_height = 1.8;
        loaded.save_to(&path)?;

        let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        assert_eq!(
            written["holograms"]["on"],
            serde_json::json!(true),
            "a whole section this build never heard of survives the save"
        );
        assert_eq!(
            written["diff"]["wrap"],
            serde_json::json!(true),
            "so does a foreign key inside a section this build does write"
        );
        assert_eq!(
            written["diff"]["line_height"],
            serde_json::json!(f64::from(1.8_f32)),
            "the field this build owns is the one that changed"
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "row_height is computed from the exact constants this test compares against"
    )]
    fn row_height_clamps_to_a_sane_range() {
        assert_eq!(DiffSettings { line_height: 0.5 }.row_height(), 11.0);
        assert_eq!(DiffSettings { line_height: 99.0 }.row_height(), 33.0);
    }
}
