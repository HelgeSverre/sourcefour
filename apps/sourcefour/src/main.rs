//! The Sourcefour desktop entry point.

mod app;
mod demo;
mod theme;
mod views;

use std::{env, ffi::OsString, path::PathBuf, process::ExitCode};

use tracing_subscriber::EnvFilter;

use crate::app::run;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LaunchRequest {
    pub(crate) path: PathBuf,
    pub(crate) demo: bool,
}

fn main() -> ExitCode {
    initialize_tracing();
    match parse_args(env::args_os().skip(1)) {
        Ok(request) => run(&request),
        Err(ArgumentError::HelpRequested) => {
            println!("usage: sourcefour [PATH] [--demo]");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("sourcefour: {error}");
            eprintln!("usage: sourcefour [PATH] [--demo]");
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
    for argument in arguments {
        if argument == "--demo" {
            demo = true;
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
    Ok(LaunchRequest { path, demo })
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
    use super::{ArgumentError, LaunchRequest, parse_args};
    use std::{ffi::OsString, path::PathBuf};

    #[test]
    fn parses_a_path_and_demo_mode() -> Result<(), ArgumentError> {
        let request = parse_args([OsString::from("repository"), OsString::from("--demo")])?;
        assert_eq!(
            request,
            LaunchRequest {
                path: PathBuf::from("repository"),
                demo: true
            }
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
