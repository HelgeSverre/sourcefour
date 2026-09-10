use std::path::{Path, PathBuf};

/// Resolves symlinks while keeping Windows paths compatible with Git output.
pub(crate) fn canonical(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    #[cfg(windows)]
    {
        without_verbatim_prefix(canonical)
    }

    #[cfg(not(windows))]
    {
        canonical
    }
}

#[cfg(any(windows, test))]
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    let Some(path) = path.to_str() else {
        return path;
    };

    if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{path}"));
    }

    path.strip_prefix(r"\\?\")
        .map_or_else(|| PathBuf::from(path), PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::without_verbatim_prefix;
    use std::path::PathBuf;

    #[test]
    fn removes_windows_verbatim_disk_prefix() {
        assert_eq!(
            without_verbatim_prefix(PathBuf::from(r"\\?\C:\sourcefour")),
            PathBuf::from(r"C:\sourcefour")
        );
    }

    #[test]
    fn converts_windows_verbatim_unc_prefix() {
        assert_eq!(
            without_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\sourcefour")),
            PathBuf::from(r"\\server\sourcefour")
        );
    }
}
