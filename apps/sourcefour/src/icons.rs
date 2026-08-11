//! Typed identities for every SVG embedded in the application.

macro_rules! define_icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub(crate) enum Icon {
            $($variant),+
        }

        impl Icon {
            #[cfg(test)]
            pub(crate) const ALL: &[Self] = &[$(Self::$variant),+];

            pub(crate) const fn path(self) -> &'static str {
                match self {
                    $(Self::$variant => concat!("icons/", $file)),+
                }
            }
        }

        pub(crate) const ASSETS: &[(&str, &[u8])] = &[
            $(
                (
                    concat!("icons/", $file),
                    include_bytes!(concat!("../assets/icons/", $file)).as_slice(),
                )
            ),+
        ];
    };
}

define_icons! {
    Archive => "archive.svg",
    ArrowDownToLine => "arrow-down-to-line.svg",
    ArrowUpFromLine => "arrow-up-from-line.svg",
    ChevronDown => "chevron-down.svg",
    ChevronLeft => "chevron-left.svg",
    ChevronRight => "chevron-right.svg",
    ChevronUp => "chevron-up.svg",
    CloudDownload => "cloud-download.svg",
    Folder => "folder.svg",
    FolderPlus => "folder-plus.svg",
    GitBranch => "git-branch.svg",
    GitCommitHorizontal => "git-commit-horizontal.svg",
    GitMerge => "git-merge.svg",
    Globe => "globe.svg",
    Search => "search.svg",
    Settings => "settings.svg",
}
