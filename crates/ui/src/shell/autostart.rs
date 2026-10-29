//! The Start-with-Windows toggle, shared by the tray menu and the settings window.

use std::path::Path;

use super::{Ui, with_ui};
use crate::lifecycle::{Autostart, CurrentUserRegistry};
use crate::settings_window;

/// Whether the Run entry starts `exe` (off without an exe).
pub(super) fn enabled_for(exe: Option<&Path>) -> bool {
    exe.is_some_and(|exe| Autostart::new(&CurrentUserRegistry, exe).is_enabled())
}

pub(super) fn enabled(ui: &Ui) -> bool {
    enabled_for(ui.exe.as_deref())
}

/// The Start-with-Windows toggle, from the menu or the settings window.
pub(crate) fn set(on: bool) {
    with_ui(|ui| {
        if let Some(exe) = ui.exe.as_deref()
            && let Err(e) = Autostart::new(&CurrentUserRegistry, exe).set(on)
        {
            log::error!("could not change Start with Windows: {e}");
        }
        ui.autostart_item.set_checked(enabled(ui));
    });
    settings_window::refresh();
}
