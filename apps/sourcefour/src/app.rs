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
    App, AppContext, Application, AssetSource, Bounds, Result, SharedString, TitlebarOptions,
    WindowBounds, WindowOptions, px, size,
};

use crate::{
    LaunchRequest,
    theme::{INITIAL_HEIGHT, INITIAL_WIDTH, MINIMUM_HEIGHT, MINIMUM_WIDTH},
    views::SourcefourWindow,
};

pub(crate) fn run(request: LaunchRequest) -> ExitCode {
    let path = request.path;
    let demo = request.demo;
    let opened = Arc::new(AtomicBool::new(false));
    let opened_in_app = Arc::clone(&opened);
    Application::new()
        .with_assets(SourcefourAssets::new())
        .run(move |cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(INITIAL_WIDTH), px(INITIAL_HEIGHT)), cx);
            let result = cx.open_window(
                WindowOptions {
                    titlebar: Some(TitlebarOptions {
                        title: Some("Sourcefour".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(gpui::point(px(10.0), px(13.0))),
                    }),
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(MINIMUM_WIDTH), px(MINIMUM_HEIGHT))),
                    ..Default::default()
                },
                move |_window, cx| cx.new(|_| SourcefourWindow::new(path, demo)),
            );
            if let Err(error) = result {
                tracing::error!(%error, "could not open Sourcefour window");
            } else {
                opened_in_app.store(true, Ordering::Release);
            }
            cx.activate(true);
        });
    launch_exit_code(opened.load(Ordering::Acquire))
}

fn launch_exit_code(opened: bool) -> ExitCode {
    if opened {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

pub(crate) fn display_path(path: &Path) -> String {
    path.display().to_string()
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
    use std::process::ExitCode;

    use super::launch_exit_code;

    #[test]
    fn window_open_failure_is_nonzero() {
        assert_eq!(launch_exit_code(false), ExitCode::from(1));
        assert_eq!(launch_exit_code(true), ExitCode::SUCCESS);
    }
}
