//! Alerts on screen: the notification identity Windows shows on every toast, and
//! raising an Alert as a toast (or a tray balloon when the toast fails).

use std::path::Path;

use uqt_core::engine::Alert;
use windows::Win32::Foundation::HWND;

use super::{open_popup, with_ui};
use crate::icon::app_icon_png;
use crate::lifecycle::{self, CurrentUserRegistry};
use crate::notify;
use crate::platform;
use crate::present::{self, Moment};

/// Shows an alert as a toast, or as a tray balloon when the toast fails.
pub(super) fn raise(alert: &Alert) {
    with_ui(|ui| {
        let at = Moment {
            now: ui.clock.now(),
            utc_offset_secs: platform::utc_offset_secs(),
        };
        let message = present::alert_message(alert, at);
        log::info!("alert: {} ({})", message.title, alert.meter_key);
        let Some(toasts) = ui.toasts.as_mut() else {
            return;
        };
        let click = || {
            let _ = slint::invoke_from_event_loop(open_popup);
        };
        if let Err(e) = toasts.show(&message, click) {
            log::warn!("toast failed ({e}); showing a balloon");
            if let Some(tray) = &ui.tray {
                notify::balloon(HWND(tray.window_handle()), &message);
            }
        }
    });
}

/// Writes the notification icon next to the settings and registers the identity.
pub(super) fn register_identity(data_dir: Option<&Path>) {
    let Some(dir) = data_dir else { return };
    let icon = dir.join("notification.png");
    if let Some(png) = app_icon_png(64)
        && std::fs::create_dir_all(dir).is_ok()
        && std::fs::read(&icon).ok().as_deref() != Some(png.as_slice())
        && let Err(e) = std::fs::write(&icon, png)
    {
        log::warn!("could not write the notification icon: {e}");
    }
    if let Err(e) = lifecycle::register_notifications(&CurrentUserRegistry, &icon) {
        log::warn!("could not register the notification identity: {e}");
    }
}
