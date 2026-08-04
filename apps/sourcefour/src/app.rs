use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui::{
    App, AppContext, Application, AssetSource, Bounds, Menu, MenuItem, Result, SharedString,
    TitlebarOptions, WindowBounds, WindowOptions, actions, px, size,
};
use sourcefour_git::{discover, display_name};
use sourcefour_model::{RepoFailure, RepoLocation};

use crate::{
    LaunchRequest, demo,
    theme::{
        ERROR_HEIGHT, ERROR_WIDTH, INITIAL_HEIGHT, INITIAL_WIDTH, MINIMUM_HEIGHT, MINIMUM_WIDTH,
    },
    views::{ErrorWindow, SourcefourWindow},
};

actions!(sourcefour, [Quit]);

/// Returned once the user dismisses the "not a repository" window.
const DISCOVERY_FAILED: u8 = 2;
/// Returned when no window could be opened at all.
const WINDOW_FAILED: u8 = 1;

/// What a launch resolved to before any window exists.
#[derive(Clone, Debug)]
pub(crate) struct WindowLaunch {
    pub(crate) demo: bool,
    pub(crate) name: String,
    pub(crate) path: String,
    /// Absent in demo mode, which must render without touching a repository.
    pub(crate) location: Option<RepoLocation>,
}

/// The outcome of resolving a launch request.
#[derive(Debug)]
pub(crate) enum Launch {
    /// A repository window, or the deterministic demo fixture.
    Window(WindowLaunch),
    /// Discovery failed, so only the error window opens.
    Failed(RepoFailure),
}

impl Launch {
    /// Resolves the request without touching GPUI, so the decision is testable.
    ///
    /// Demo mode never discovers: the screenshot fixture must render the same
    /// window wherever it is invoked from.
    pub(crate) fn resolve(request: &LaunchRequest) -> Self {
        if request.demo {
            return Self::Window(WindowLaunch {
                demo: true,
                name: demo::REPOSITORY_NAME.to_owned(),
                path: demo::REPOSITORY_PATH.to_owned(),
                location: None,
            });
        }
        match discover(&request.path) {
            Ok(location) => Self::Window(WindowLaunch {
                demo: false,
                name: display_name(&location),
                // A bare repository has no worktree, so show the repository itself.
                path: display_path(
                    location
                        .active_worktree_path
                        .as_deref()
                        .unwrap_or(&location.common_dir),
                ),
                location: Some(location),
            }),
            Err(failure) => Self::Failed(failure),
        }
    }
}

pub(crate) fn run(request: &LaunchRequest) -> ExitCode {
    let launch = Launch::resolve(request);
    let failed = matches!(launch, Launch::Failed(_));
    let opened = Arc::new(AtomicBool::new(false));
    let opened_in_app = Arc::clone(&opened);
    Application::new()
        .with_assets(SourcefourAssets::new())
        .run(move |cx: &mut App| {
            // One invocation is one window: closing it closes Sourcefour.
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.bind_keys(history_keymap());
            cx.bind_keys(crate::text_input::keymap());
            // A bare process gets no quit shortcut from macOS: the standard
            // application menu is what makes Cmd+Q work.
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.bind_keys([gpui::KeyBinding::new("cmd-q", Quit, None)]);
            cx.set_menus(vec![Menu {
                name: SharedString::from("Sourcefour"),
                items: vec![MenuItem::action("Quit Sourcefour", Quit)],
            }]);
            let result = match launch {
                Launch::Window(window) => {
                    let options = window_options(
                        INITIAL_WIDTH,
                        INITIAL_HEIGHT,
                        Some((MINIMUM_WIDTH, MINIMUM_HEIGHT)),
                        cx,
                    );
                    cx.open_window(options, move |_window, cx| {
                        cx.new(|cx| SourcefourWindow::new(window, cx))
                    })
                    .map(|_| ())
                }
                Launch::Failed(failure) => {
                    let options = window_options(ERROR_WIDTH, ERROR_HEIGHT, None, cx);
                    cx.open_window(options, move |_window, cx| {
                        cx.new(|_| ErrorWindow::new(&failure))
                    })
                    .map(|_| ())
                }
            };
            if let Err(error) = result {
                tracing::error!(%error, "could not open Sourcefour window");
            } else {
                opened_in_app.store(true, Ordering::Release);
            }
            cx.activate(true);
        });
    launch_exit_code(opened.load(Ordering::Acquire), failed)
}

/// History navigation keys, declared once rather than matched ad hoc (§8.5).
fn history_keymap() -> Vec<gpui::KeyBinding> {
    use crate::views::{
        CloseDiff, FilterEnter, FilterEscape, FocusFilter, PageDown, PageUp, SelectFirstCommit,
        SelectLastLoadedCommit, SelectNextCommit, SelectPreviousCommit,
    };
    vec![
        gpui::KeyBinding::new("down", SelectNextCommit, Some("History")),
        gpui::KeyBinding::new("up", SelectPreviousCommit, Some("History")),
        gpui::KeyBinding::new("pagedown", PageDown, Some("History")),
        gpui::KeyBinding::new("pageup", PageUp, Some("History")),
        gpui::KeyBinding::new("home", SelectFirstCommit, Some("History")),
        gpui::KeyBinding::new("end", SelectLastLoadedCommit, Some("History")),
        // The filter is reachable from anywhere in the window (§4.7).
        gpui::KeyBinding::new("cmd-f", FocusFilter, None),
        gpui::KeyBinding::new("escape", FilterEscape, Some("FilterInput")),
        gpui::KeyBinding::new("enter", FilterEnter, Some("FilterInput")),
        gpui::KeyBinding::new("escape", CloseDiff, Some("Diff")),
    ]
}

fn window_options(
    width: f32,
    height: f32,
    minimum: Option<(f32, f32)>,
    cx: &mut App,
) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Sourcefour".into()),
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(px(10.0), px(13.0))),
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(width), px(height)),
            cx,
        ))),
        window_min_size: minimum.map(|(width, height)| size(px(width), px(height))),
        ..Default::default()
    }
}

fn launch_exit_code(opened: bool, failed: bool) -> ExitCode {
    if !opened {
        ExitCode::from(WINDOW_FAILED)
    } else if failed {
        ExitCode::from(DISCOVERY_FAILED)
    } else {
        ExitCode::SUCCESS
    }
}

/// Renders a path the way a shell prompt would, shortening the home directory.
pub(crate) fn display_path(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(relative) if relative.as_os_str().is_empty() => String::from("~"),
        Some(relative) => format!("~/{}", relative.display()),
        None => path.display().to_string(),
    }
}

struct SourcefourAssets {
    base: PathBuf,
}

impl SourcefourAssets {
    fn new() -> Self {
        Self {
            base: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets"),
        }
    }
}

impl AssetSource for SourcefourAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        fs::read(self.base.join(path))
            .map(|data| Some(Cow::Owned(data)))
            .map_err(Into::into)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        fs::read_dir(self.base.join(path))
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        entry
                            .ok()
                            .and_then(|entry| entry.file_name().into_string().ok())
                            .map(SharedString::from)
                    })
                    .collect()
            })
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        process::ExitCode,
    };

    use sourcefour_model::RepoFailureKind;
    use sourcefour_test_support::TempRepo;

    use super::{Launch, display_path, launch_exit_code};
    use crate::LaunchRequest;

    fn request(path: impl Into<PathBuf>, demo: bool) -> LaunchRequest {
        LaunchRequest {
            path: path.into(),
            demo,
        }
    }

    #[test]
    fn demo_mode_opens_without_a_repository() {
        let launch = Launch::resolve(&request("/nowhere/at/all", true));

        let Launch::Window(window) = launch else {
            panic!("demo mode must open a window");
        };
        assert!(window.demo);
        assert!(!window.name.is_empty());
        assert!(!window.path.is_empty());
        assert_eq!(
            window.location, None,
            "demo mode must not depend on a repository"
        );
    }

    #[test]
    fn a_repository_resolves_to_its_name_and_worktree_path() {
        let repository = TempRepo::init();

        let launch = Launch::resolve(&request(repository.path(), false));

        let Launch::Window(window) = launch else {
            panic!("a repository must open a window");
        };
        assert!(!window.demo);
        assert_eq!(window.name, "repository");
        assert_eq!(window.path, display_path(repository.path()));
        assert!(
            window.location.is_some(),
            "the window carries the location it will load metadata from"
        );
    }

    #[test]
    fn a_linked_worktree_resolves_to_that_worktree_path() {
        let repository = TempRepo::init();
        let linked = repository.add_worktree("side");

        let launch = Launch::resolve(&request(&linked, false));

        let Launch::Window(window) = launch else {
            panic!("a linked worktree must open a window");
        };
        assert_eq!(window.name, "repository");
        assert_eq!(window.path, display_path(&linked));
    }

    #[test]
    fn a_path_outside_any_repository_resolves_to_a_failure() {
        let launch = Launch::resolve(&request(std::env::temp_dir(), false));

        let Launch::Failed(failure) = launch else {
            panic!("a non-repository path must not open the main window");
        };
        assert_eq!(failure.kind, RepoFailureKind::NotARepository);
    }

    #[test]
    fn exit_codes_separate_dismissed_errors_from_window_failures() {
        assert_eq!(launch_exit_code(true, false), ExitCode::SUCCESS);
        assert_eq!(launch_exit_code(true, true), ExitCode::from(2));
        assert_eq!(launch_exit_code(false, false), ExitCode::from(1));
    }

    #[test]
    fn display_path_shortens_the_home_directory() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set on this platform"));

        assert_eq!(
            display_path(&home.join("code/sourcefour")),
            "~/code/sourcefour"
        );
        assert_eq!(display_path(&home), "~");
        assert_eq!(display_path(Path::new("/opt/elsewhere")), "/opt/elsewhere");
    }
}
