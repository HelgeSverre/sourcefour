//! Platform policy for child processes launched by Sourcefour's GUI.

use std::{ffi::OsStr, process::Command};

/// Builds a command that stays in the background when launched by the GUI.
///
/// Windows otherwise gives console-subsystem children such as Git and `FFmpeg`
/// a visible console window. Other platforms need no extra process flags.
#[must_use]
pub fn background_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    configure(&mut command);
    command
}

#[cfg(windows)]
fn configure(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure(_: &mut Command) {}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::background_command;

    #[test]
    fn preserves_the_program_and_builder_interface() {
        let mut command = background_command("git");
        command.arg("--version");

        assert_eq!(command.get_program(), OsStr::new("git"));
        assert!(command.get_args().eq([OsStr::new("--version")]));
    }
}
