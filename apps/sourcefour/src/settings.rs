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
    pub(crate) appearance: AppearanceSettings,
    pub(crate) git: GitSettings,
    pub(crate) github: GithubSettings,
    pub(crate) history: HistorySettings,
    pub(crate) diff: DiffSettings,
    pub(crate) video: VideoSettings,
}

/// How the interface reads, as opposed to what it does.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct AppearanceSettings {
    pub(crate) date_display: DateDisplay,
    /// The monospace family for diffs, hashes and logs; unset is the
    /// platform's own.
    ///
    /// Any family the system can resolve is honored, so a hand-edit is a
    /// first-class way to set this — the overlay only offers the common few.
    /// A name nothing resolves is not an error: gpui walks its own fallback
    /// stack instead, which on macOS reaches Helvetica, so a typo shows as
    /// proportional code rather than as a crash or a blank pane.
    pub(crate) mono_font: Option<String>,
}

impl AppearanceSettings {
    /// The monospace family to render with: the override, or the platform's.
    pub(crate) fn mono_font(&self) -> &str {
        self.mono_font.as_deref().unwrap_or(crate::theme::MONO_FONT)
    }
}

/// How a commit's timestamp is written.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DateDisplay {
    /// `YYYY-MM-DD HH:MM` on the clock the commit itself recorded.
    Absolute,
    /// An age: "2 hours ago"; also what unknown future values read as, which
    /// is why it is last — `serde(other)` only sits on the final variant.
    #[default]
    #[serde(other)]
    Relative,
}

/// What the user's own Git is asked to do on their behalf (§6.12).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct GitSettings {
    /// Whether a fetch also drops the remote-tracking refs whose branches
    /// are gone from the remote. Off by default: deleting refs is not what
    /// someone who pressed Fetch asked for.
    pub(crate) fetch_prune: bool,
    /// Where generated linked-worktree paths are placed.
    pub(crate) worktree_location: WorktreeLocation,
}

/// Layout used when suggesting a path for a new linked worktree.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum WorktreeLocation {
    /// Place the checkout below `.worktrees` in the main worktree.
    InsideRepository,
    /// Place the checkout beside the repository's main worktree.
    #[default]
    #[serde(other)]
    Sibling,
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

/// The commit list's own shape (§4.4).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct HistorySettings {
    pub(crate) density: Density,
}

impl HistorySettings {
    /// One history row's height in px, which the list, the virtualization
    /// math and the graph's node centering all read — a row is only ever as
    /// tall as one number says.
    pub(crate) fn row_height(&self) -> f32 {
        match self.density {
            Density::Cozy => crate::theme::HISTORY_ROW_HEIGHT,
            // Six px off the row buys four more commits on the shortest
            // window the app opens, and the 12.5px subject still fits.
            Density::Compact => 24.0,
        }
    }
}

/// How much room a commit gets in the history list.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Density {
    /// More commits per screen, less air around each.
    Compact,
    /// The roomy default; also what unknown future values read as, which is
    /// why it is last — `serde(other)` only sits on the final variant.
    #[default]
    #[serde(other)]
    Cozy,
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
    pub(crate) wrap: bool,
    pub(crate) syntax_highlighting: bool,
    pub(crate) show_whitespace: bool,
    pub(crate) tab_width: u8,
}

impl Default for DiffSettings {
    fn default() -> Self {
        Self {
            line_height: 1.55,
            wrap: false,
            syntax_highlighting: true,
            show_whitespace: false,
            tab_width: 4,
        }
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
    use super::{
        AppSettings, AppearanceSettings, AuthMethod, DateDisplay, Density, DiffSettings,
        GitSettings, GithubSettings, HistorySettings, VideoSettings, WorktreeLocation,
    };

    #[test]
    fn settings_round_trip_through_their_file() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("nested").join("settings.json");
        let settings = AppSettings {
            appearance: AppearanceSettings {
                date_display: DateDisplay::Absolute,
                mono_font: Some(String::from("JetBrains Mono")),
            },
            git: GitSettings {
                fetch_prune: true,
                worktree_location: WorktreeLocation::InsideRepository,
            },
            github: GithubSettings {
                enabled: true,
                auth_method: AuthMethod::Token,
            },
            history: HistorySettings {
                density: Density::Compact,
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

        assert_eq!(
            loaded.diff,
            DiffSettings {
                line_height: 1.7,
                wrap: true,
                ..DiffSettings::default()
            }
        );
        Ok(())
    }

    #[test]
    fn line_height_parses_from_a_json_document() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("settings.json");
        std::fs::write(&path, r#"{"diff": {"line_height": 1.8}}"#)?;

        let loaded = AppSettings::load_from(&path);

        assert_eq!(
            loaded.diff,
            DiffSettings {
                line_height: 1.8,
                ..DiffSettings::default()
            }
        );
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
    #[expect(
        clippy::float_cmp,
        reason = "row_height returns the exact constants this test names"
    )]
    fn a_history_density_survives_a_file_that_predates_it() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("settings.json");
        std::fs::write(&path, r#"{"diff": {"line_height": 1.7}}"#)?;
        assert_eq!(
            AppSettings::load_from(&path).history,
            HistorySettings::default(),
            "a file written before the section reads as the roomy list"
        );
        assert_eq!(HistorySettings::default().row_height(), 30.0);
        assert_eq!(
            HistorySettings {
                density: Density::Compact
            }
            .row_height(),
            24.0
        );

        std::fs::write(
            &path,
            r#"{"history": {"density": "compact", "columns": ["hash"]}}"#,
        )?;
        assert_eq!(
            AppSettings::load_from(&path).history,
            HistorySettings {
                density: Density::Compact
            },
            "a future key inside the section is ignored, not fatal"
        );

        std::fs::write(&path, r#"{"history": {"density": "microscopic"}}"#)?;
        assert_eq!(
            AppSettings::load_from(&path).history.density,
            Density::Cozy,
            "a future density reads as Cozy, not a parse failure"
        );
        Ok(())
    }

    #[test]
    fn a_git_section_survives_a_file_that_predates_it() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("settings.json");
        std::fs::write(&path, r#"{"diff": {"line_height": 1.7}}"#)?;
        assert_eq!(
            AppSettings::load_from(&path).git,
            GitSettings::default(),
            "a file written before the section prunes nothing"
        );
        assert!(
            !GitSettings::default().fetch_prune,
            "a fetch that deletes local refs is never the unasked-for default"
        );
        assert_eq!(
            GitSettings::default().worktree_location,
            WorktreeLocation::Sibling
        );

        std::fs::write(&path, r#"{"git": {"worktree_location": "future-layout"}}"#)?;
        assert_eq!(
            AppSettings::load_from(&path).git.worktree_location,
            WorktreeLocation::Sibling,
            "unknown layouts retain the safe default"
        );

        std::fs::write(
            &path,
            r#"{"git": {"fetch_prune": true, "rebase_on_pull": true}}"#,
        )?;
        assert_eq!(
            AppSettings::load_from(&path).git,
            GitSettings {
                fetch_prune: true,
                ..GitSettings::default()
            },
            "a future key inside the section is ignored, not fatal"
        );
        Ok(())
    }

    #[test]
    fn an_unset_mono_font_is_the_family_this_platform_ships() {
        assert_eq!(
            AppearanceSettings::default().mono_font(),
            if cfg!(target_os = "macos") {
                "Menlo"
            } else if cfg!(target_os = "windows") {
                "Consolas"
            } else {
                "monospace"
            }
        );
        assert_eq!(
            AppearanceSettings {
                mono_font: Some(String::from("Fira Code")),
                ..AppearanceSettings::default()
            }
            .mono_font(),
            "Fira Code",
            "a named family wins over the platform's"
        );
    }

    #[test]
    fn a_mono_font_the_presets_never_offered_round_trips() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("settings.json");
        std::fs::write(&path, r#"{"appearance": {"mono_font": "Comic Mono"}}"#)?;

        let loaded = AppSettings::load_from(&path);
        assert_eq!(loaded.appearance.mono_font(), "Comic Mono");

        loaded.save_to(&path)?;
        assert_eq!(
            AppSettings::load_from(&path)
                .appearance
                .mono_font
                .as_deref(),
            Some("Comic Mono"),
            "a hand-edited family survives a save from the overlay"
        );
        Ok(())
    }

    #[test]
    fn an_appearance_section_survives_a_file_that_predates_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("settings.json");
        std::fs::write(&path, r#"{"diff": {"line_height": 1.7}}"#)?;
        assert_eq!(
            AppSettings::load_from(&path).appearance,
            AppearanceSettings::default(),
            "a file written before the section reads as the default display"
        );

        std::fs::write(
            &path,
            r#"{"appearance": {"date_display": "absolute", "hue": "warm"}}"#,
        )?;
        assert_eq!(
            AppSettings::load_from(&path).appearance,
            AppearanceSettings {
                date_display: DateDisplay::Absolute,
                mono_font: None,
            },
            "a future key inside the section is ignored, not fatal"
        );

        std::fs::write(&path, r#"{"appearance": {"date_display": "stardate"}}"#)?;
        assert_eq!(
            AppSettings::load_from(&path).appearance.date_display,
            DateDisplay::Relative,
            "a future display reads as Relative, not a parse failure"
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
        assert_eq!(
            DiffSettings {
                line_height: 0.5,
                ..DiffSettings::default()
            }
            .row_height(),
            11.0
        );
        assert_eq!(
            DiffSettings {
                line_height: 99.0,
                ..DiffSettings::default()
            }
            .row_height(),
            33.0
        );
    }
}
