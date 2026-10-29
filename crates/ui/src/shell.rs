//! The running tray app: tray icon, context menu, popup, and the hand-off from the
//! scheduler thread to the UI thread. Win32 and Slint glue; the content it shows comes
//! from [`crate::present`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, PhysicalPosition, Timer, TimerMode, VecModel};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use uqt_core::engine::{Engine, EngineState, RefreshReason};
use uqt_core::runner::{EngineHandle, EngineThread};
use uqt_core::time::Clock;

use crate::badge::resolve_badge;
use crate::icon::{IconStyle, render_rgba};
use crate::present::{self, Moment, PopupView, Ring, Tone, TrayView};
use crate::{Popup, RowData, platform, session};

const POPUP_WIDTH: f32 = 360.0;
const POPUP_MARGIN: f32 = 12.0;
/// A tray click this soon after the popup hid itself is the click that hid it.
const REOPEN_GUARD: Duration = Duration::from_millis(400);

/// Launch options the app crate decides.
#[derive(Clone, Debug, Default)]
pub struct ShellOptions {
    /// The user's Badge override folder.
    pub badges_dir: Option<PathBuf>,
    /// Measurement mode: write this file once the tray icon exists, and show the popup
    /// once at start (hidden after 2 s) like the prototype measurements did.
    pub ready_file: Option<PathBuf>,
    /// Leak test: redraw the icon every 500 ms with changing rings.
    pub icon_churn: bool,
}

struct Ui {
    popup: slint::Weak<Popup>,
    tray: Option<TrayIcon>,
    state: Option<EngineState>,
    clock: Arc<dyn Clock>,
    engine: EngineHandle,
    badges_dir: Option<PathBuf>,
    badges: HashMap<&'static str, String>,
    shown_icon: Option<IconKey>,
    shown_tooltip: String,
    was_foreground: bool,
    auto_hidden_at: Option<Instant>,
}

#[derive(Clone, PartialEq)]
struct IconKey {
    rings: Vec<(u32, Tone)>,
    badge: Option<&'static str>,
    size: u32,
    style: IconStyle,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| u.borrow_mut().as_mut().map(f))
}

fn icon_style() -> IconStyle {
    if platform::taskbar_dark() {
        IconStyle::DarkTaskbar
    } else {
        IconStyle::LightTaskbar
    }
}

fn make_icon(rings: &[Ring], badge_svg: &str) -> Option<tray_icon::Icon> {
    let size = platform::small_icon_size();
    let rgba = render_rgba(size, rings, badge_svg, icon_style());
    tray_icon::Icon::from_rgba(rgba, size, size).ok()
}

/// Recomputes the icon, tooltip and popup rows from the latest state and the current
/// time. The icon is replaced only when it would look different; `tray-icon` destroys
/// the previous HICON on every replacement.
fn present_now() {
    let rows = with_ui(|ui| {
        let state = ui.state.as_ref()?;
        let at = Moment {
            now: ui.clock.now(),
            utc_offset_secs: platform::utc_offset_secs(),
        };
        let view = present::tray(state, at);
        let popup = present::popup(state, at);
        update_tray(ui, &view);
        Some(popup)
    })
    .flatten();
    if let (Some(view), Some(popup)) = (rows, with_ui(|ui| ui.popup.upgrade()).flatten()) {
        let (rows, height) = popup_rows(&view);
        popup.set_rows(ModelRc::new(VecModel::from(rows)));
        popup.set_content_height(height);
    }
}

fn update_tray(ui: &mut Ui, view: &TrayView) {
    let key = IconKey {
        rings: view
            .rings
            .iter()
            .map(|r| ((r.fraction.clamp(0.0, 1.0) * 1000.0) as u32, r.tone))
            .collect(),
        badge: view.assistant_id,
        size: platform::small_icon_size(),
        style: icon_style(),
    };
    let Some(tray) = ui.tray.as_ref() else {
        return;
    };
    if ui.shown_icon.as_ref() != Some(&key) {
        let svg = match (view.assistant_id, view.badge) {
            (Some(id), Some(spec)) => ui
                .badges
                .entry(id)
                .or_insert_with(|| resolve_badge(id, spec.svg, ui.badges_dir.as_deref()).svg)
                .clone(),
            _ => String::new(),
        };
        if let Some(icon) = make_icon(&view.rings, &svg) {
            let _ = tray.set_icon(Some(icon));
            ui.shown_icon = Some(key);
        }
    }
    if ui.shown_tooltip != view.tooltip {
        let _ = tray.set_tooltip(Some(&view.tooltip));
        ui.shown_tooltip = view.tooltip.clone();
    }
}

/// Flattens the popup view into Slint rows and returns the window's logical height.
fn popup_rows(view: &PopupView) -> (Vec<RowData>, f32) {
    let mut rows = Vec::new();
    let text_row = |kind: i32, title: &str| RowData {
        kind,
        title: title.into(),
        ..Default::default()
    };
    for section in &view.sections {
        rows.push(text_row(0, &section.title));
        for acc in &section.accounts {
            rows.push(text_row(1, &acc.title));
            if let Some(p) = &acc.problem {
                rows.push(text_row(3, p));
            }
            for n in &acc.notices {
                rows.push(text_row(3, n));
            }
            for m in &acc.meters {
                let amount = match &m.limit {
                    Some(l) => format!("{} {l}", m.amount),
                    None => m.amount.clone(),
                };
                let detail = [m.reset.as_deref(), m.pace.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" \u{b7} ");
                rows.push(RowData {
                    kind: 2,
                    title: m.label.as_str().into(),
                    amount: amount.into(),
                    detail: detail.into(),
                    meta: format!("{} \u{b7} {} \u{b7} {}", m.source, m.exactness, m.updated)
                        .into(),
                    fraction: m.fraction.unwrap_or(0.0),
                    has_bar: m.fraction.is_some(),
                    tone: match m.tone {
                        Tone::Amber => 1,
                        Tone::Red => 2,
                        Tone::Green | Tone::Grey => 0,
                    },
                    stale: m.stale,
                });
            }
        }
    }
    if rows.is_empty() {
        rows.push(text_row(3, "No Accounts found"));
    }
    let content: f32 = rows
        .iter()
        .map(|r| match r.kind {
            0 => 32.0,
            1 => 24.0,
            2 => 72.0,
            _ => 20.0,
        })
        .sum();
    let height = 32.0 + content + 4.0 * (rows.len() as f32 - 1.0);
    (rows, height)
}

fn place(popup: &Popup) {
    let wa = platform::work_area();
    let sf = popup.window().scale_factor();
    let w = (POPUP_WIDTH * sf).round() as i32;
    let h = (popup.get_content_height() * sf).round() as i32;
    let m = (POPUP_MARGIN * sf).round() as i32;
    popup
        .window()
        .set_position(PhysicalPosition::new(wa.right - m - w, wa.bottom - m - h));
}

fn show_popup(popup: &Popup, style_timer: &Rc<Timer>) {
    let dark = platform::apps_dark();
    popup.set_dark(dark);
    popup.set_accent(platform::accent(dark));
    present_now();
    if popup.show().is_err() {
        return;
    }
    place(popup);
    with_ui(|ui| {
        ui.was_foreground = false;
        ui.engine.refresh(RefreshReason::PopupOpen);
    });
    // The HWND exists after the first event-loop turn; style and focus it then.
    let weak = popup.as_weak();
    style_timer.start(
        TimerMode::SingleShot,
        Duration::from_millis(30),
        move || {
            let Some(p) = weak.upgrade() else { return };
            if let Some(hwnd) = platform::hwnd_of(p.window()) {
                let acrylic = platform::apply_flyout_style(hwnd, dark);
                p.set_solid(!acrylic);
                platform::bring_to_front(hwnd);
            }
            place(&p);
        },
    );
}

fn hide_popup(popup: &Popup) {
    let _ = popup.hide();
    with_ui(|ui| ui.was_foreground = false);
}

/// Runs the tray app until "Exit". The engine moves to the scheduler thread.
pub fn run(engine: Engine, clock: Arc<dyn Clock>, options: ShellOptions) -> Result<(), String> {
    platform::set_per_monitor_dpi_awareness();
    let popup = Popup::new().map_err(|e| e.to_string())?;

    let menu = Menu::new();
    let refresh_item = MenuItem::new("Refresh now", true, None);
    let exit_item = MenuItem::new("Exit", true, None);
    menu.append(&refresh_item).map_err(|e| e.to_string())?;
    menu.append(&exit_item).map_err(|e| e.to_string())?;
    let (refresh_id, exit_id) = (refresh_item.id().clone(), exit_item.id().clone());

    let tray = TrayIconBuilder::new()
        .with_icon(make_icon(&[], "").ok_or("icon")?)
        .with_tooltip("UsageQuotaTray")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| e.to_string())?;

    let engine_thread = EngineThread::spawn(engine, clock.clone(), |state| {
        let _ = slint::invoke_from_event_loop(move || {
            with_ui(|ui| ui.state = Some(state));
            present_now();
        });
    });
    let handle = engine_thread.handle();
    session::spawn_presence_monitor(handle.clone());

    UI.with(|u| {
        *u.borrow_mut() = Some(Ui {
            popup: popup.as_weak(),
            tray: Some(tray),
            state: None,
            clock,
            engine: handle.clone(),
            badges_dir: options.badges_dir.clone(),
            badges: HashMap::new(),
            shown_icon: None,
            shown_tooltip: String::new(),
            was_foreground: false,
            auto_hidden_at: None,
        })
    });

    let style_timer = Rc::new(Timer::default());
    {
        let weak = popup.as_weak();
        popup.on_dismiss(move || {
            if let Some(p) = weak.upgrade() {
                hide_popup(&p);
            }
        });
    }

    // Tray and menu events arrive off the UI thread; hop onto it. Rc is not Send, so the
    // style timer is reached through a thread-local.
    thread_local!(static STYLE_TIMER: RefCell<Option<Rc<Timer>>> = const { RefCell::new(None) });
    STYLE_TIMER.with(|t| *t.borrow_mut() = Some(style_timer.clone()));
    let weak = popup.as_weak();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = e
        {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(p) = weak.upgrade() else { return };
                if p.window().is_visible() {
                    hide_popup(&p);
                    return;
                }
                let just_hidden = with_ui(|ui| {
                    ui.auto_hidden_at
                        .is_some_and(|t| t.elapsed() < REOPEN_GUARD)
                })
                .unwrap_or(false);
                if !just_hidden && let Some(timer) = STYLE_TIMER.with(|t| t.borrow().clone()) {
                    show_popup(&p, &timer);
                }
            });
        }
    }));
    let menu_engine = handle.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == refresh_id {
            menu_engine.refresh(RefreshReason::Manual);
        } else if e.id == exit_id {
            let _ = slint::invoke_from_event_loop(|| {
                let _ = slint::quit_event_loop();
            });
        }
    }));

    // Light dismiss: hide once the popup has had focus and lost it.
    let dismiss_timer = Timer::default();
    {
        let weak = popup.as_weak();
        dismiss_timer.start(TimerMode::Repeated, Duration::from_millis(250), move || {
            let Some(p) = weak.upgrade() else { return };
            if !p.window().is_visible() {
                return;
            }
            let Some(hwnd) = platform::hwnd_of(p.window()) else {
                return;
            };
            let front = platform::is_foreground(hwnd);
            let lost = with_ui(|ui| {
                if front {
                    ui.was_foreground = true;
                    false
                } else if ui.was_foreground {
                    ui.auto_hidden_at = Some(Instant::now());
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
            if lost {
                hide_popup(&p);
            }
        });
    }

    // Countdowns and "updated N min ago" move with the clock.
    let minute_timer = Timer::default();
    minute_timer.start(TimerMode::Repeated, Duration::from_secs(60), present_now);

    let measure_timers = measurement_timers(&popup, &style_timer, &options);

    slint::run_event_loop_until_quit().map_err(|e| e.to_string())?;

    drop(measure_timers);
    engine_thread.shutdown();
    // Removes the tray icon before the process exits.
    UI.with(|u| u.borrow_mut().take());
    Ok(())
}

fn measurement_timers(
    popup: &Popup,
    style_timer: &Rc<Timer>,
    options: &ShellOptions,
) -> Vec<Timer> {
    let mut timers = Vec::new();
    if let Some(path) = options.ready_file.clone() {
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
    if options.icon_churn {
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
                if let (Some(tray), Some(icon)) = (ui.tray.as_ref(), make_icon(&rings, &svg)) {
                    let _ = tray.set_icon(Some(icon));
                }
            });
        });
        timers.push(churn);
    }
    timers
}
