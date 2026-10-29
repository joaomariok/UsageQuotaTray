//! The headless command line. The installer runs `usagequotatray.exe --quit` before
//! an upgrade or uninstall, and its uninstaller then runs
//! `usagequotatray.exe --remove-from-pc [--delete-data]` (see
//! `tools/installer/UsageQuotaTray.iss`); without arguments the tray starts.

use std::path::Path;
use std::time::Duration;

use uqt_ui::lifecycle::{
    OutsideChange, QuitOutcome, Registry, RemovalError, remove_from_pc_undoing, request_quit,
};

pub const REMOVE_FROM_PC: &str = "--remove-from-pc";
pub const DELETE_DATA: &str = "--delete-data";
pub const QUIT: &str = "--quit";
/// How long `--quit` waits for the running trays to exit.
pub const QUIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Everything was removed; for `--quit`, no tray is running any more.
pub const EXIT_REMOVED: i32 = 0;
/// A change (Claude Code's statusLine) could not be put back; nothing was removed.
/// The uninstaller stops on this code.
pub const EXIT_NOT_UNDONE: i32 = 1;
/// The changes were undone, but deleting the app's entries failed partway.
pub const EXIT_PARTLY: i32 = 2;
/// The arguments were not understood; nothing was done.
pub const EXIT_USAGE: i32 = 3;
/// `--quit`: a tray was still running when the time was up.
pub const EXIT_STILL_RUNNING: i32 = 4;

/// What this run of the exe does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Tray,
    /// "Remove UsageQuotaTray from this PC" without UI.
    RemoveFromPc {
        delete_data: bool,
    },
    /// Ask the trays running in this session to exit, and wait for them.
    Quit,
}

/// Parses the arguments after the program name. Unknown arguments are refused, so a
/// typo never starts the tray instead of removing it.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let (mut remove, mut delete_data, mut quit) = (false, false, false);
    for arg in args {
        match arg.as_str() {
            REMOVE_FROM_PC => remove = true,
            DELETE_DATA => delete_data = true,
            QUIT => quit = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    match (quit, remove, delete_data) {
        (true, false, false) => Ok(Command::Quit),
        (true, _, _) => Err(format!("{QUIT} takes no other argument")),
        (false, false, false) => Ok(Command::Tray),
        (false, true, delete_data) => Ok(Command::RemoveFromPc { delete_data }),
        (false, false, true) => Err(format!("{DELETE_DATA} needs {REMOVE_FROM_PC}")),
    }
}

/// Removal from the PC, as the in-app button does it after confirming, returning the
/// process exit code.
pub fn remove_from_pc(
    registry: &dyn Registry,
    changes: &[&dyn OutsideChange],
    data_dir: Option<&Path>,
    delete_data: bool,
) -> i32 {
    match remove_from_pc_undoing(registry, changes, data_dir, delete_data) {
        Ok(()) => EXIT_REMOVED,
        Err(RemovalError::NotUndone(_)) => EXIT_NOT_UNDONE,
        Err(RemovalError::Partly(_)) => EXIT_PARTLY,
    }
}

/// `--quit`: asks the trays in this session to exit (the `event` they listen on) and
/// waits up to `timeout` for the `running` marker they hold to go, returning the
/// process exit code.
pub fn quit(event: &str, running: &str, timeout: Duration) -> i32 {
    match request_quit(event, running, timeout) {
        QuitOutcome::NoneRunning | QuitOutcome::Closed => EXIT_REMOVED,
        QuitOutcome::StillRunning => EXIT_STILL_RUNNING,
    }
}
