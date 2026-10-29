//! Switches for measurement and screenshot runs. They exist in every build so the
//! launch options keep one shape, but they are only read from the environment, and
//! their timers only run, when the `dev-hooks` feature is on; a normal build ignores
//! every `UQT_*` variable here.

use std::path::PathBuf;

/// What to open on its own for a screenshot run (`UQT_SHOT`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shot {
    /// Open the popup and keep it open.
    Popup,
    /// Open the settings window once the first state has arrived.
    Settings,
}

/// Test-only launch options. The defaults are a normal run.
#[derive(Clone, Debug, Default)]
pub struct DevHooks {
    /// Measurement mode: write this file once the tray icon exists, and show the popup
    /// once at start (hidden after 2 s) like the prototype measurements did
    /// (`UQT_READY_FILE`).
    pub ready_file: Option<PathBuf>,
    /// Leak test: redraw the icon every 500 ms with changing rings (`UQT_ICON_CHURN=1`).
    pub icon_churn: bool,
    /// Neither register the notification identity nor show toasts, so a measurement run
    /// never writes to the registry (`UQT_NOTIFICATIONS=off`).
    pub notifications_off: bool,
    /// Screenshot runs (`UQT_SHOT=popup` or `UQT_SHOT=settings`).
    pub shot: Option<Shot>,
}

#[cfg(feature = "dev-hooks")]
impl DevHooks {
    /// The hooks set in this process's environment.
    pub fn from_env() -> Self {
        Self {
            ready_file: std::env::var_os("UQT_READY_FILE").map(PathBuf::from),
            icon_churn: std::env::var("UQT_ICON_CHURN").is_ok_and(|v| v == "1"),
            notifications_off: std::env::var("UQT_NOTIFICATIONS").is_ok_and(|v| v == "off"),
            shot: match std::env::var("UQT_SHOT").as_deref() {
                Ok("popup") => Some(Shot::Popup),
                Ok("settings") => Some(Shot::Settings),
                _ => None,
            },
        }
    }
}

#[cfg(feature = "dev-hooks")]
pub(super) use timers::timers;

#[cfg(feature = "dev-hooks")]
mod timers {
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use slint::{ComponentHandle, Timer, TimerMode};

    use super::{DevHooks, Shot};
    use crate::Popup;
    use crate::present::{Ring, Tone};
    use crate::settings_window;
    use crate::shell::{hide_popup, make_icon, show_popup, with_ui};

    /// The timers the hooks need; dropping them stops the hooks.
    pub(in crate::shell) fn timers(
        popup: &Popup,
        style_timer: &Rc<Timer>,
        hooks: &DevHooks,
    ) -> Vec<Timer> {
        let mut timers = Vec::new();
        if let Some(path) = hooks.ready_file.clone() {
            show_popup(popup, style_timer);
            let ready = Timer::default();
            ready.start(
                TimerMode::SingleShot,
                Duration::from_millis(100),
                move || {
                    let _ = std::fs::write(&path, "ready");
                },
            );
            timers.push(ready);
            let hide = Timer::default();
            let weak = popup.as_weak();
            hide.start(TimerMode::SingleShot, Duration::from_secs(2), move || {
                if let Some(p) = weak.upgrade() {
                    hide_popup(&p);
                }
            });
            timers.push(hide);
        }
        match hooks.shot {
            Some(Shot::Popup) => {
                let open = Timer::default();
                let weak = popup.as_weak();
                let style = style_timer.clone();
                open.start(TimerMode::SingleShot, Duration::from_secs(1), move || {
                    if let Some(p) = weak.upgrade() {
                        show_popup(&p, &style);
                    }
                });
                timers.push(open);
            }
            Some(Shot::Settings) => {
                let open = Timer::default();
                open.start(
                    TimerMode::SingleShot,
                    Duration::from_secs(1),
                    settings_window::open,
                );
                timers.push(open);
            }
            None => {}
        }
        if hooks.icon_churn {
            let churn = Timer::default();
            let start = Instant::now();
            churn.start(TimerMode::Repeated, Duration::from_millis(500), move || {
                let t = start.elapsed().as_secs_f32();
                let rings = [
                    Ring {
                        fraction: 0.5 + 0.45 * (t * 0.6).sin(),
                        tone: Tone::Green,
                    },
                    Ring {
                        fraction: 0.5 + 0.45 * (t * 0.4 + 1.0).cos(),
                        tone: Tone::Amber,
                    },
                ];
                with_ui(|ui| {
                    let svg = ui.badges.values().next().cloned().unwrap_or_default();
                    if let (Some(tray), Some(icon)) =
                        (ui.tray.as_ref(), make_icon(&rings, &svg, false))
                    {
                        let _ = tray.set_icon(Some(icon));
                    }
                });
            });
            timers.push(churn);
        }
        timers
    }
}
