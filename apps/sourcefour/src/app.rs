use std::{
    borrow::Cow,
    path::Path,
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

use crate::ui_state::{UiState, WindowMode, WindowState};
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
    /// The demo scene to seed; meaningless outside demo mode (§12.4).
    pub(crate) scene: demo::Scene,
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
                scene: request.scene,
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
                scene: demo::Scene::Overview,
            }),
            Err(failure) => Self::Failed(failure),
        }
    }
}

/// §12.5 phase probe; prints only under `SOURCEFOUR_STARTUP_LOG`.
fn startup_phase(name: &str) {
    if std::env::var_os("SOURCEFOUR_STARTUP_LOG").is_some() {
        eprintln!("phase {name} {}", crate::since_process_start());
    }
}

pub(crate) fn run(request: &LaunchRequest) -> ExitCode {
    let size_override = request.window;
    let launch = Launch::resolve(request);
    let ui_state = match &launch {
        Launch::Window(window) if !window.demo && size_override.is_none() => UiState::load(),
        _ => UiState::default(),
    };
    startup_phase("resolved");
    let failed = matches!(launch, Launch::Failed(_));
    let opened = Arc::new(AtomicBool::new(false));
    let opened_in_app = Arc::clone(&opened);
    Application::new()
        .with_assets(SourcefourAssets::new())
        .run(move |cx: &mut App| {
            startup_phase("gpui-run");
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
            startup_phase("pre-window");
            let result = match launch {
                Launch::Window(launch) => {
                    let (width, height) = size_override.unwrap_or((INITIAL_WIDTH, INITIAL_HEIGHT));
                    let window_state = size_override.is_none().then_some(ui_state.window).flatten();
                    let options = window_options(
                        width,
                        height,
                        Some((MINIMUM_WIDTH, MINIMUM_HEIGHT)),
                        window_state,
                        cx,
                    );
                    cx.open_window(options, move |window, cx| {
                        cx.new(|cx| SourcefourWindow::new(launch, &ui_state, window, cx))
                    })
                    .map(|_| ())
                }
                Launch::Failed(failure) => {
                    let options = window_options(ERROR_WIDTH, ERROR_HEIGHT, None, None, cx);
                    cx.open_window(options, move |_window, cx| {
                        cx.new(|_| ErrorWindow::new(&failure))
                    })
                    .map(|_| ())
                }
            };
            startup_phase("window-opened");
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
        CloseActionsRun, CloseDiff, CloseSettings, CopyPreviewSelection, FilterEnter, FilterEscape,
        FocusDetails, FocusFilter, NextActionsJob, NextActionsStep, OpenSettings, PageDown, PageUp,
        PrevActionsJob, PrevActionsStep, SelectFirstCommit, SelectLastLoadedCommit,
        SelectNextCommit, SelectPreviousCommit, ToggleDetails,
    };
    vec![
        gpui::KeyBinding::new("down", SelectNextCommit, Some("History")),
        gpui::KeyBinding::new("up", SelectPreviousCommit, Some("History")),
        // Same pair the Actions run overlay already binds to jobs, so the
        // hand doesn't have to relearn which pane takes vi motion.
        gpui::KeyBinding::new("j", SelectNextCommit, Some("History")),
        gpui::KeyBinding::new("k", SelectPreviousCommit, Some("History")),
        gpui::KeyBinding::new("pagedown", PageDown, Some("History")),
        gpui::KeyBinding::new("pageup", PageUp, Some("History")),
        gpui::KeyBinding::new("home", SelectFirstCommit, Some("History")),
        gpui::KeyBinding::new("end", SelectLastLoadedCommit, Some("History")),
        // The filter is reachable from anywhere in the window (§4.7).
        gpui::KeyBinding::new("cmd-f", FocusFilter, None),
        gpui::KeyBinding::new("escape", FilterEscape, Some("FilterInput")),
        gpui::KeyBinding::new("enter", FilterEnter, Some("FilterInput")),
        // A "History"-context binding fires from any focused descendant,
        // filter input included, unless something more specific to
        // "FilterInput" claims the same key first — escape and enter do that
        // above by overriding to a filter action; j/k have no filter action
        // to give them, so `NoAction` is what stops the letter from being
        // read as a navigation command instead of typed into the query.
        gpui::KeyBinding::new("j", gpui::NoAction, Some("FilterInput")),
        gpui::KeyBinding::new("k", gpui::NoAction, Some("FilterInput")),
        // Space has the same flaw below: without this, a multi-word filter
        // query toggles the details pane instead of typing the space.
        gpui::KeyBinding::new("space", gpui::NoAction, Some("FilterInput")),
        gpui::KeyBinding::new("escape", CloseDiff, Some("Diff")),
        gpui::KeyBinding::new("cmd-c", CopyPreviewSelection, Some("Diff")),
        gpui::KeyBinding::new("space", ToggleDetails, Some("History")),
        gpui::KeyBinding::new("enter", FocusDetails, Some("History")),
        // Settings, the macOS way (§ settings overlay).
        gpui::KeyBinding::new("cmd-,", OpenSettings, None),
        gpui::KeyBinding::new("escape", CloseSettings, Some("Settings")),
        // The Actions run overlay: jobs on j/k, steps on arrows.
        gpui::KeyBinding::new("escape", CloseActionsRun, Some("Actions")),
        gpui::KeyBinding::new("j", NextActionsJob, Some("Actions")),
        gpui::KeyBinding::new("k", PrevActionsJob, Some("Actions")),
        gpui::KeyBinding::new("down", NextActionsStep, Some("Actions")),
        gpui::KeyBinding::new("up", PrevActionsStep, Some("Actions")),
    ]
}

fn window_options(
    width: f32,
    height: f32,
    minimum: Option<(f32, f32)>,
    state: Option<WindowState>,
    cx: &mut App,
) -> WindowOptions {
    let display_size = cx.primary_display().map(|display| display.bounds().size);
    let (width, height, mode) = state.map_or((width, height, WindowMode::Windowed), |state| {
        (state.width, state.height, state.mode)
    });
    let (width, height) = clamped_window_size(
        width,
        height,
        minimum,
        display_size.map(|size| (size.width.0, size.height.0)),
    );
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let window_bounds = match mode {
        WindowMode::Windowed => WindowBounds::Windowed(bounds),
        WindowMode::Maximized => WindowBounds::Maximized(bounds),
        WindowMode::Fullscreen => WindowBounds::Fullscreen(bounds),
    };
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Sourcefour".into()),
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(px(10.0), px(13.0))),
        }),
        window_bounds: Some(window_bounds),
        window_min_size: minimum.map(|(width, height)| size(px(width), px(height))),
        ..Default::default()
    }
}

fn clamped_window_size(
    width: f32,
    height: f32,
    minimum: Option<(f32, f32)>,
    maximum: Option<(f32, f32)>,
) -> (f32, f32) {
    let (minimum_width, minimum_height) = minimum.unwrap_or((1.0, 1.0));
    let (maximum_width, maximum_height) = maximum.unwrap_or((f32::MAX, f32::MAX));
    (
        width.clamp(minimum_width, maximum_width.max(minimum_width)),
        height.clamp(minimum_height, maximum_height.max(minimum_height)),
    )
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
///
/// Windows hands back `\\?\C:\…` verbatim paths from canonicalization, which no
/// user would recognise and which also defeats the home-directory match, so
/// that prefix goes before anything else looks at the path.
pub(crate) fn display_path(path: &Path) -> String {
    let path = path
        .to_str()
        .and_then(|text| text.strip_prefix(r"\\?\"))
        .map_or(path, Path::new);
    let home = crate::persist::home_directory();
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(relative) if relative.as_os_str().is_empty() => String::from("~"),
        Some(relative) => format!("~/{}", relative.display()),
        None => path.display().to_string(),
    }
}

struct SourcefourAssets;

impl SourcefourAssets {
    fn new() -> Self {
        Self
    }
}

/// Every asset the interface can ask for, compiled into the binary.
///
/// Reading these from disk would mean a different layout per install channel —
/// a macOS bundle's Resources, an installer's program directory, a bare
/// `cargo install`ed binary with no directory at all. Embedding them means the
/// binary is the whole app wherever it lands. Add a file here when you add one
/// to `assets/`.
/// `include_bytes!` needs a literal path, so this names each file once and
/// derives both the lookup key and the bytes from it.
macro_rules! asset {
    ($path:literal) => {
        (
            $path,
            include_bytes!(concat!("../assets/", $path)).as_slice(),
        )
    };
}

const ASSETS: &[(&str, &[u8])] = &[
    asset!("icons/archive.svg"),
    asset!("icons/arrow-down-to-line.svg"),
    asset!("icons/arrow-up-from-line.svg"),
    asset!("icons/chevron-down.svg"),
    asset!("icons/chevron-right.svg"),
    asset!("icons/cloud-download.svg"),
    asset!("icons/git-branch.svg"),
    asset!("icons/git-commit-horizontal.svg"),
    asset!("icons/git-merge.svg"),
    asset!("icons/globe.svg"),
    asset!("icons/search.svg"),
    asset!("icons/settings.svg"),
];

impl AssetSource for SourcefourAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ASSETS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let prefix = path.trim_end_matches('/');
        Ok(ASSETS
            .iter()
            .filter_map(|(name, _)| name.strip_prefix(prefix)?.strip_prefix('/'))
            .map(SharedString::from)
            .collect())
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

    use gpui::AssetSource;

    use super::{Launch, SourcefourAssets, clamped_window_size, display_path, launch_exit_code};
    use crate::LaunchRequest;

    fn request(path: impl Into<PathBuf>, demo: bool) -> LaunchRequest {
        LaunchRequest {
            path: path.into(),
            demo,
            window: None,
            scene: crate::demo::Scene::Overview,
        }
    }

    #[test]
    fn a_restored_window_fits_the_current_display() {
        assert_eq!(
            clamped_window_size(2400.0, 400.0, Some((900.0, 600.0)), Some((1920.0, 1080.0)),),
            (1920.0, 600.0)
        );
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
    fn every_icon_on_disk_is_embedded_in_the_binary() -> Result<(), Box<dyn std::error::Error>> {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("icons");

        for entry in std::fs::read_dir(directory)? {
            let name = entry?.file_name();
            let path = format!("icons/{}", name.to_string_lossy());

            let loaded = SourcefourAssets.load(&path)?;

            assert!(
                loaded.is_some_and(|bytes| !bytes.is_empty()),
                "{path} is on disk but not in ASSETS — an installed build would \
                 render it as a blank square"
            );
        }
        Ok(())
    }

    #[test]
    fn display_path_shortens_the_home_directory() {
        let home = crate::persist::home_directory().expect("every platform names a home directory");

        assert_eq!(
            display_path(&home.join("code").join("sourcefour")),
            format!("~/code{}sourcefour", std::path::MAIN_SEPARATOR)
        );
        assert_eq!(display_path(&home), "~");
        assert_eq!(display_path(Path::new("/opt/elsewhere")), "/opt/elsewhere");

        // Windows canonicalization yields these; they must never reach the eye.
        assert_eq!(
            display_path(Path::new(r"\\?\C:\code\sourcefour")),
            r"C:\code\sourcefour"
        );
    }
}
