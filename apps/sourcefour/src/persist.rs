//! Where the per-user files live, and the tolerant read and write both of
//! them share.
//!
//! [`crate::ui_state`] and [`crate::settings`] make the same promise in their
//! own words — unknown fields are ignored, missing fields fall back to
//! defaults, so older and newer builds can share a file. The promise is one
//! pair of functions, so it lives here rather than in whichever module
//! happened to need it first.

use std::path::{Path, PathBuf};

/// Reads a tolerant JSON file: a missing or corrupt file must never block
/// the window, so any failure yields the default.
pub(crate) fn load_json_or_default<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

/// Writes pretty JSON, creating the directory on first save and preserving
/// keys already in the file that this build has no field for: read tolerance
/// is only half a promise if the next save drops them. A hand-written note,
/// or a newer build's section, survives an older build touching one toggle.
///
/// # Errors
///
/// Returns the underlying error when the file cannot be written; callers log
/// it, because a failed save must never interrupt the user.
pub(crate) fn save_json_pretty<T: serde::Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let mut json = serde_json::to_value(value).map_err(std::io::Error::other)?;
    if let Ok(text) = std::fs::read_to_string(path)
        && let Ok(existing) = serde_json::from_str(&text)
    {
        keep_unknown(&mut json, existing);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&json).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

/// Copies every key `known` has no place for out of `existing`, recursively.
///
/// Objects only. A value this build does write wins outright, including an
/// array or a null, because that is the value the user just chose; only a key
/// absent from what this build serializes is taken from the file.
fn keep_unknown(known: &mut serde_json::Value, existing: serde_json::Value) {
    let (serde_json::Value::Object(known), serde_json::Value::Object(existing)) = (known, existing)
    else {
        return;
    };
    for (key, value) in existing {
        match known.get_mut(&key) {
            Some(known) => keep_unknown(known, value),
            None => {
                known.insert(key, value);
            }
        }
    }
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
    use super::keep_unknown;
    use serde_json::json;

    #[test]
    fn a_key_this_build_writes_wins_over_the_one_in_the_file() {
        let mut known = json!({"diff": {"line_height": 1.8}, "list": [1]});
        keep_unknown(
            &mut known,
            json!({"diff": {"line_height": 1.55, "wrap": true}, "list": [9, 9], "extra": null}),
        );

        assert_eq!(
            known,
            json!({"diff": {"line_height": 1.8, "wrap": true}, "list": [1], "extra": null}),
            "the new value stays, the foreign keys arrive, arrays are not merged"
        );
    }

    #[test]
    fn a_shape_that_is_not_an_object_is_left_alone() {
        let mut known = json!({"diff": 3});
        keep_unknown(&mut known, json!({"diff": {"line_height": 1.55}}));

        assert_eq!(
            known,
            json!({"diff": 3}),
            "a hand-edit that changed a section's type does not merge into it"
        );
    }
}
