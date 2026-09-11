# Changelog

All notable changes to Sourcefour are documented in this file.

## [0.1.2] - 2026-09-11

### Added

- Status bar label showing when history is scoped to a branch.
- Automatic GitHub connection check on startup, when credentials are already
  usable.
- Editable, verifiable ffmpeg/ffprobe path in Settings → Diffs, replacing the
  settings.json-only override.
- Website link in Settings → About.

### Fixed

- Commit graph not rendering until the list was scrolled, whenever the
  working tree had uncommitted changes.

## [0.1.1] - 2026-09-10

### Added

- Homebrew cask distribution for the macOS app.

### Fixed

- Debian package validation.

## [0.1.0] - 2026-09-10

Initial public release.

### Added

- Native, virtualized commit history with a commit graph, branch and tag scopes,
  search, worktrees, remotes, and ahead/behind status.
- Commit details and changed-file navigation with unified and split diffs,
  syntax highlighting, merge-parent comparisons, and line counts.
- Working-tree support for staging, unstaging, committing, fetching, pulling,
  pushing, and creating branches.
- Rich previews for documents, images, SVGs, animated GIFs, and video frames.
- Optional GitHub integration for pull requests, checks, Actions runs, job logs,
  and failure-focused log analysis.
- Configurable appearance, history density, dates, fetch behavior, fonts, and
  external FFmpeg discovery.
- Signed and notarized universal macOS package, Windows MSI, Linux AppImage,
  release archives, checksums, and Homebrew distribution.

### Fixed

- Renderer memory growth when browsing large or complex repository histories.
- Crashes and excessive work when commits contain very large changed-file lists.
- Repository discovery and worktree handling across Windows path forms.
- Text input, scrolling, overlay, diff-layout, and Actions-status edge cases.

[0.1.2]: https://github.com/HelgeSverre/sourcefour/releases/tag/v0.1.2
[0.1.1]: https://github.com/HelgeSverre/sourcefour/releases/tag/v0.1.1
[0.1.0]: https://github.com/HelgeSverre/sourcefour/releases/tag/v0.1.0
