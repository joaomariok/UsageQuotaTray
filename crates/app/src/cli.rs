//! The headless command line. The installer's uninstaller runs
//! `usagequotatray.exe --remove-from-pc [--delete-data]` (see
//! `tools/installer/UsageQuotaTray.iss`); without arguments the tray starts.

use std::path::Path;

use uqt_ui::lifecycle::{OutsideChange, Registry, RemovalError, remove_from_pc_undoing};

pub const REMOVE_FROM_PC: &str = "--remove-from-pc";
pub const DELETE_DATA: &str = "--delete-data";

/// Everything was removed.
pub const EXIT_REMOVED: i32 = 0;
/// A change (Claude Code's statusLine) could not be put back; nothing was removed.
/// The uninstaller stops on this code.
pub const EXIT_NOT_UNDONE: i32 = 1;
/// The changes were undone, but deleting the app's entries failed partway.
pub const EXIT_PARTLY: i32 = 2;
/// The arguments were not understood; nothing was done.
pub const EXIT_USAGE: i32 = 3;

/// What this run of the exe does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Tray,
    /// "Remove UsageQuotaTray from this PC" without UI.
    RemoveFromPc {
        delete_data: bool,
    },
}

/// Parses the arguments after the program name. Unknown arguments are refused, so a
/// typo never starts the tray instead of removing it.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let (mut remove, mut delete_data) = (false, false);
    for arg in args {
        match arg.as_str() {
            REMOVE_FROM_PC => remove = true,
            DELETE_DATA => delete_data = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    match (remove, delete_data) {
        (false, false) => Ok(Command::Tray),
        (true, delete_data) => Ok(Command::RemoveFromPc { delete_data }),
        (false, true) => Err(format!("{DELETE_DATA} needs {REMOVE_FROM_PC}")),
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
