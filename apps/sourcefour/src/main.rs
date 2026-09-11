//! The Sourcefour desktop entry point.

mod app;
mod context_menu;
mod demo;
mod diff_highlight;
mod diff_split;
mod graph_paint;
mod history;
mod panels;
mod persist;
mod settings;
mod settings_ui;
mod text_input;
mod theme;
mod ui_state;
mod views;
mod window_chrome;

use std::{env, ffi::OsString, path::PathBuf, process::ExitCode};

use tracing_subscriber::EnvFilter;

use crate::{app::run, demo::Scene};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LaunchRequest {
    pub(crate) path: PathBuf,
    pub(crate) demo: bool,
    /// Window size override, used by the visual-regression captures (§12.4).
    pub(crate) window: Option<(f32, f32)>,
    /// Which demo scene to seed; ignored without `--demo` (§12.4).
    pub(crate) scene: Scene,
}

const USAGE: &str = "usage: sourcefour [PATH] [--demo] [--width N --height N] [--scene NAME]";

/// When the process entered `main`, for the startup measurement (§12.5).
/// Dynamic-loader time before `main` is excluded; the ledger says so.
static PROCESS_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Milliseconds since the process entered `main`.
pub(crate) fn since_process_start() -> u128 {
    PROCESS_START
        .get()
        .map_or(0, |start| start.elapsed().as_millis())
}

fn main() -> ExitCode {
    PROCESS_START.set(std::time::Instant::now()).ok();
    initialize_tracing();
    match parse_args(env::args_os().skip(1)) {
        Ok(request) => run(&request),
        Err(ArgumentError::HelpRequested) => {
            println!("{USAGE}");
            println!("scenes: {}", Scene::NAMES);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("sourcefour: {error}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn initialize_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .without_time()
        .init();
}

pub(crate) fn parse_args(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<LaunchRequest, ArgumentError> {
    let mut demo = false;
    let mut path = None;
    let mut width = None;
    let mut height = None;
    let mut scene = Scene::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        if argument == "--demo" {
            demo = true;
        } else if argument == "--scene" {
            scene = arguments
                .next()
                .and_then(|value| Scene::from_name(&value.to_string_lossy()))
                .ok_or_else(|| ArgumentError::Unknown(argument.clone()))?;
        } else if argument == "--width" || argument == "--height" {
            let value = arguments
                .next()
                .and_then(|value| value.to_string_lossy().parse::<f32>().ok())
                .filter(|value| *value >= 320.0)
                .ok_or_else(|| ArgumentError::Unknown(argument.clone()))?;
            if argument == "--width" {
                width = Some(value);
            } else {
                height = Some(value);
            }
        } else if argument == "--help" || argument == "-h" {
            return Err(ArgumentError::HelpRequested);
        } else if argument.to_string_lossy().starts_with('-') {
            return Err(ArgumentError::Unknown(argument));
        } else if path.replace(PathBuf::from(&argument)).is_some() {
            return Err(ArgumentError::TooManyPaths);
        }
    }
    let path = match path {
        Some(path) => path,
        None => env::current_dir().map_err(ArgumentError::CurrentDirectory)?,
    };
    Ok(LaunchRequest {
        path,
        demo,
        window: width.zip(height),
        scene,
    })
}

#[derive(Debug)]
pub(crate) enum ArgumentError {
    Unknown(OsString),
    TooManyPaths,
    HelpRequested,
    CurrentDirectory(std::io::Error),
}

impl std::fmt::Display for ArgumentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(argument) => write!(
                formatter,
                "unknown argument `{}`",
                argument.to_string_lossy()
            ),
            Self::TooManyPaths => formatter.write_str("only one path may be provided"),
            Self::HelpRequested => formatter.write_str("help requested"),
            Self::CurrentDirectory(error) => {
                write!(formatter, "could not read current directory: {error}")
            }
        }
    }
}

impl std::error::Error for ArgumentError {}

#[cfg(test)]
mod tests {
    use super::{ArgumentError, LaunchRequest, Scene, parse_args};
    use std::{ffi::OsString, path::PathBuf};

    #[test]
    fn parses_a_path_and_demo_mode() -> Result<(), ArgumentError> {
        let request = parse_args([OsString::from("repository"), OsString::from("--demo")])?;
        assert_eq!(
            request,
            LaunchRequest {
                path: PathBuf::from("repository"),
                demo: true,
                window: None,
                scene: Scene::Overview,
            }
        );
        Ok(())
    }

    #[test]
    fn parses_a_capture_scene() -> Result<(), ArgumentError> {
        let request = parse_args(["--demo", "--scene", "split"].map(OsString::from))?;
        assert_eq!(request.scene, Scene::Split);

        assert!(
            parse_args(["--scene", "nonexistent"].map(OsString::from)).is_err(),
            "an unknown scene name is rejected rather than silently ignored"
        );
        Ok(())
    }

    #[test]
    fn parses_a_window_size_for_visual_captures() -> Result<(), ArgumentError> {
        let request = parse_args(
            ["repo", "--demo", "--width", "1000", "--height", "700"].map(OsString::from),
        )?;
        assert_eq!(request.window, Some((1000.0, 700.0)));

        assert!(
            parse_args(["--width", "abc"].map(OsString::from)).is_err(),
            "a malformed size is rejected"
        );
        assert!(
            parse_args(["--width", "10", "--height", "10"].map(OsString::from)).is_err(),
            "absurdly small windows are rejected"
        );
        Ok(())
    }

    #[test]
    fn rejects_unknown_options() {
        assert!(matches!(
            parse_args([OsString::from("--revision")]),
            Err(ArgumentError::Unknown(_))
        ));
    }
}
