//! The running tray app: tray icon, context menu, popup, and the hand-off from the
//! scheduler thread to the UI thread. Win32 and Slint glue; the content it shows comes
//! from [`crate::present`]. Alerts and the notification identity live in
//! [`notifications`], the Start-with-Windows toggle in [`autostart`], and the switches
//! for measurement runs in [`dev_hooks`].

mod autostart;
mod dev_hooks;
mod notifications;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, PhysicalPosition, Timer, TimerMode, VecModel};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use uqt_core::engine::{Catalog, Engine, EngineState, RefreshReason};
use uqt_core::push::PushServer;
use uqt_core::runner::{EngineHandle, EngineThread, Update};
use uqt_core::settings::{Settings, SettingsEdit, SettingsStore};
use uqt_core::time::Clock;

pub(crate) use autostart::set as set_autostart;
pub use dev_hooks::{DevHooks, Shot};

use crate::badge::{BadgeOrigin, resolve_badge, with_default_fill};
use crate::icon::{IconStyle, render_icon};
use crate::lifecycle::{self, CurrentUserRegistry, OutsideChange};
use crate::notify::{self, Toasts};
use crate::present::{self, ActionRequest, Moment, PopupView, Ring, SettingsView, Tone, TrayView};
use crate::{Popup, RowData, platform, session, settings_window};

const POPUP_WIDTH: f32 = 360.0;
const POPUP_MARGIN: f32 = 12.0;
/// A tray click this soon after the popup hid itself is the click that hid it.
const REOPEN_GUARD: Duration = Duration::from_millis(400);

/// Launch options the app crate decides.
#[derive(Clone, Debug, Default)]
pub struct ShellOptions {
    /// The user's Badge override folder.
    pub badges_dir: Option<PathBuf>,
    /// The local app-data folder (settings, logs, Badge overrides, notification icon).
    pub data_dir: Option<PathBuf>,
    /// The settings the engine started with.
    pub settings: Settings,
    /// Where settings changes are saved; `None` keeps them in memory only.
    pub settings_store: Option<SettingsStore>,
    /// This exe, for the Start-with-Windows entry.
    pub exe: Option<PathBuf>,
    /// The push channel's pipe name; `None` starts no push channel.
    pub push_pipe: Option<String>,
    /// Changes to other programs' files that "Remove UsageQuotaTray from this PC"
    /// undoes first (the statusline helper in Claude Code's settings).
    pub outside_changes: Vec<Rc<dyn OutsideChange>>,
    /// Measurement and screenshot switches; the defaults are a normal run.
    pub dev: DevHooks,
}

struct Ui {
    popup: slint::Weak<Popup>,
    tray: Option<TrayIcon>,
    state: Option<EngineState>,
    catalog: Catalog,
    settings: Settings,
    store: Option<SettingsStore>,
    exe: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    outside_changes: Vec<Rc<dyn OutsideChange>>,
    toasts: Option<Toasts>,
    autostart_item: CheckMenuItem,
    clock: Arc<dyn Clock>,
    engine: EngineHandle,
    badges_dir: Option<PathBuf>,
    badges: HashMap<&'static str, String>,
    shown_icon: Option<IconKey>,
    shown_tooltip: String,
    was_foreground: bool,
    auto_hidden_at: Option<Instant>,
    /// Whether the popup's "Unrecognized" groups are expanded.
    unrecognized_open: bool,
}

#[derive(Clone, PartialEq)]
struct IconKey {
    rings: Vec<(u32, Tone)>,
    badge: Option<&'static str>,
    dot: bool,
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

fn make_icon(rings: &[Ring], badge_svg: &str, dot: bool) -> Option<tray_icon::Icon> {
    let size = platform::small_icon_size();
    let rgba = render_icon(size, rings, badge_svg, icon_style(), dot);
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
        Some((popup, ui.unrecognized_open))
    })
    .flatten();
    if let (Some((view, open)), Some(popup)) = (rows, with_ui(|ui| ui.popup.upgrade()).flatten()) {
        let (rows, height) = popup_rows(&view, open);
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
        dot: view.dot,
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
                .or_insert_with(|| {
                    let chosen = resolve_badge(id, spec.svg, ui.badges_dir.as_deref());
                    match chosen.origin {
                        BadgeOrigin::Shipped => with_default_fill(&chosen.svg, spec.color),
                        BadgeOrigin::Override => chosen.svg,
                    }
                })
                .clone(),
            _ => String::new(),
        };
        if let Some(icon) = make_icon(&view.rings, &svg, view.dot) {
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
/// `unrecognized_open` expands the "Unrecognized" groups.
fn popup_rows(view: &PopupView, unrecognized_open: bool) -> (Vec<RowData>, f32) {
    let mut rows = Vec::new();
    let text_row = |kind: i32, title: &str| RowData {
        kind,
        title: title.into(),
        ..Default::default()
    };
    let meter_rows = |rows: &mut Vec<RowData>, meters: &[present::MeterRow]| {
        for m in meters {
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
                meta: format!("{} \u{b7} {} \u{b7} {}", m.source, m.exactness, m.updated).into(),
                fraction: m.fraction.unwrap_or(0.0),
                has_bar: m.fraction.is_some(),
                tone: match m.tone {
                    Tone::Amber => 1,
                    Tone::Red => 2,
                    Tone::Green | Tone::Grey => 0,
                },
                stale: m.stale,
            });
            if let Some(b) = &m.breakdown {
                rows.push(text_row(3, b));
            }
        }
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
            meter_rows(&mut rows, &acc.meters);
            if !acc.unrecognized.is_empty() {
                let arrow = if unrecognized_open {
                    "\u{25be}"
                } else {
                    "\u{25b8}"
                };
                rows.push(text_row(
                    4,
                    &format!("{arrow} Unrecognized ({})", acc.unrecognized.len()),
                ));
                if unrecognized_open {
                    meter_rows(&mut rows, &acc.unrecognized);
                }
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
            4 => 24.0,
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

thread_local! {
    static STYLE_TIMER: RefCell<Option<Rc<Timer>>> = const { RefCell::new(None) };
}

/// Opens the popup unless it is open (a toast click).
fn open_popup() {
    let Some(p) = with_ui(|ui| ui.popup.upgrade()).flatten() else {
        return;
    };
    if !p.window().is_visible()
        && let Some(timer) = STYLE_TIMER.with(|t| t.borrow().clone())
    {
        show_popup(&p, &timer);
    }
}

/// A new state from the scheduler thread, on the UI thread.
fn on_update(update: Update) {
    with_ui(|ui| {
        ui.state = Some(update.state);
        ui.catalog = update.catalog;
    });
    present_now();
    for alert in &update.alerts {
        notifications::raise(alert);
    }
    settings_window::refresh();
    for outcome in &update.outcomes {
        let owner = settings_window::hwnd();
        match &outcome.result {
            Ok(text) => platform::show_info(owner, "UsageQuotaTray", text),
            Err(text) => platform::show_error(owner, "UsageQuotaTray", text),
        }
    }
}

/// A settings button: asks first when the module wants that, then runs the action on
/// the scheduler thread; its outcome comes back with the next update.
pub(crate) fn run_action(request: ActionRequest) {
    if let Some(question) = &request.confirm
        && !platform::confirm(settings_window::hwnd(), "UsageQuotaTray", question)
    {
        return;
    }
    with_ui(|ui| ui.engine.run_action(request.assistant, request.key));
}

/// What the settings window shows now.
pub(crate) fn settings_view() -> Option<SettingsView> {
    with_ui(|ui| {
        let empty = EngineState {
            now: ui.clock.now(),
            assistants: Vec::new(),
        };
        present::settings_view(
            &ui.catalog,
            ui.state.as_ref().unwrap_or(&empty),
            &ui.settings,
            autostart::enabled(ui),
        )
    })
}

pub(crate) fn data_folder_text() -> String {
    with_ui(|ui| ui.data_dir.as_ref().map(|d| d.display().to_string()))
        .flatten()
        .unwrap_or_else(|| "(no app-data folder)".into())
}

/// Applies one settings change: saved at once, sent to the engine, shown again.
pub(crate) fn apply_edit(edit: SettingsEdit) {
    with_ui(|ui| {
        ui.settings.apply(edit);
        if let Some(store) = &ui.store
            && let Err(e) = store.save(&ui.settings)
        {
            log::error!("could not save settings: {e}");
        }
        ui.engine.apply_settings(ui.settings.clone());
    });
    settings_window::refresh();
}

/// "Remove UsageQuotaTray from this PC", after the user confirms: first undoes the
/// changes made to other programs' files (the statusline helper), then deletes the
/// app's entries; then quits. When undoing fails nothing is deleted and the app stays.
/// An installed copy hands over to the installer's uninstaller instead, without asking
/// and without quitting: the uninstaller asks its own questions, closes this tray
/// (`--quit`) and runs the same removal headless. Cancelling there leaves the tray
/// running.
pub(crate) fn remove_from_pc(delete_data: bool) {
    let owner = settings_window::hwnd();
    let exe_dir = with_ui(|ui| ui.exe.clone())
        .flatten()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()));
    if let Some(uninstaller) = exe_dir
        .as_deref()
        .and_then(|dir| lifecycle::installed_uninstaller(&CurrentUserRegistry, dir))
    {
        log::info!("starting the uninstaller {}", uninstaller.display());
        if let Err(e) = std::process::Command::new(&uninstaller).spawn() {
            platform::show_error(
                owner,
                "Remove UsageQuotaTray",
                &format!("Could not start the uninstaller: {e}"),
            );
        }
        return;
    }
    let changes = with_ui(|ui| ui.outside_changes.clone()).unwrap_or_default();
    let changes: Vec<&dyn OutsideChange> = changes.iter().map(|c| c.as_ref()).collect();
    let folder = delete_data.then(data_folder_text);
    let text = lifecycle::removal_question(&changes, folder.as_deref());
    if !platform::confirm(owner, "Remove UsageQuotaTray", &text) {
        return;
    }
    let data_dir = with_ui(|ui| ui.data_dir.clone()).flatten();
    log::info!("removing UsageQuotaTray from this PC (delete data: {delete_data})");
    let result = lifecycle::remove_from_pc_undoing(
        &CurrentUserRegistry,
        &changes,
        data_dir.as_deref(),
        delete_data,
    );
    if let Err(e) = &result {
        platform::show_error(owner, "Remove UsageQuotaTray", &e.to_string());
        if matches!(e, lifecycle::RemovalError::NotUndone(_)) {
            return;
        }
    }
    // Nothing may write settings after this.
    with_ui(|ui| ui.store = None);
    let _ = slint::quit_event_loop();
}

/// Runs the tray app until "Exit". The engine moves to the scheduler thread.
pub fn run(engine: Engine, clock: Arc<dyn Clock>, options: ShellOptions) -> Result<(), String> {
    platform::set_per_monitor_dpi_awareness();
    let notifications = !options.dev.notifications_off;
    if notifications {
        // Before any window exists, so Windows attributes everything to the identity.
        notify::set_app_id();
        notifications::register_identity(options.data_dir.as_deref());
    }
    let popup = Popup::new().map_err(|e| e.to_string())?;

    let autostart_on = autostart::enabled_for(options.exe.as_deref());
    let menu = Menu::new();
    let refresh_item = MenuItem::new("Refresh now", true, None);
    let usage_item = MenuItem::new("Open usage page", true, None);
    let settings_item = MenuItem::new("Settings\u{2026}", true, None);
    let autostart_item = CheckMenuItem::new("Start with Windows", true, autostart_on, None);
    let exit_item = MenuItem::new("Exit", true, None);
    for item in [
        &refresh_item as &dyn tray_icon::menu::IsMenuItem,
        &usage_item,
        &settings_item,
        &autostart_item,
        &PredefinedMenuItem::separator(),
        &exit_item,
    ] {
        menu.append(item).map_err(|e| e.to_string())?;
    }
    let (refresh_id, settings_id, autostart_id, exit_id) = (
        refresh_item.id().clone(),
        settings_item.id().clone(),
        autostart_item.id().clone(),
        exit_item.id().clone(),
    );
    let usage_id = usage_item.id().clone();

    let tray = TrayIconBuilder::new()
        .with_icon(make_icon(&[], "", false).ok_or("icon")?)
        .with_tooltip("UsageQuotaTray")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| e.to_string())?;

    let engine_thread = EngineThread::spawn(engine, clock.clone(), |update| {
        let _ = slint::invoke_from_event_loop(move || on_update(update));
    });
    let handle = engine_thread.handle();
    session::spawn_presence_monitor(handle.clone());
    // Kept until the event loop ends; dropping it stops the listener.
    let _push = options.push_pipe.as_deref().and_then(|pipe| {
        let engine = handle.clone();
        match PushServer::spawn(pipe, move |m| engine.push(m)) {
            Ok(server) => Some(server),
            Err(e) => {
                log::warn!("push channel not started: {e}");
                None
            }
        }
    });

    UI.with(|u| {
        *u.borrow_mut() = Some(Ui {
            popup: popup.as_weak(),
            tray: Some(tray),
            state: None,
            catalog: Catalog::default(),
            settings: options.settings.clone(),
            store: options.settings_store.clone(),
            exe: options.exe.clone(),
            data_dir: options.data_dir.clone(),
            outside_changes: options.outside_changes.clone(),
            toasts: notifications.then(Toasts::new),
            autostart_item,
            clock,
            engine: handle.clone(),
            badges_dir: options.badges_dir.clone(),
            badges: HashMap::new(),
            shown_icon: None,
            shown_tooltip: String::new(),
            was_foreground: false,
            auto_hidden_at: None,
            unrecognized_open: false,
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
        let weak = popup.as_weak();
        popup.on_toggle_unrecognized(move || {
            with_ui(|ui| ui.unrecognized_open = !ui.unrecognized_open);
            present_now();
            if let Some(p) = weak.upgrade() {
                place(&p);
            }
        });
    }

    // Tray and menu events arrive off the UI thread; hop onto it. Rc is not Send, so the
    // style timer is reached through a thread-local.
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
        } else if e.id == settings_id {
            let _ = slint::invoke_from_event_loop(settings_window::open);
        } else if e.id == autostart_id {
            let _ = slint::invoke_from_event_loop(|| {
                // The menu toggles its own check mark; follow it.
                let on = with_ui(|ui| ui.autostart_item.is_checked()).unwrap_or(false);
                set_autostart(on);
            });
        } else if e.id == usage_id {
            let _ = slint::invoke_from_event_loop(|| {
                let page = with_ui(|ui| {
                    let at = Moment {
                        now: ui.clock.now(),
                        utc_offset_secs: platform::utc_offset_secs(),
                    };
                    present::usage_page(ui.state.as_ref()?, at)
                })
                .flatten();
                match page {
                    Some(url) => platform::open_url(&url),
                    None => log::info!("no usage page to open"),
                }
            });
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

    #[cfg(feature = "dev-hooks")]
    let dev_timers = dev_hooks::timers(&popup, &style_timer, &options.dev);

    // `usagequotatray.exe --quit`: the installer closes the tray before an upgrade or
    // uninstall, as "Exit" does. Kept until the event loop ends.
    let _quit = lifecycle::QuitListener::spawn(lifecycle::QUIT_EVENT, || {
        let _ = slint::invoke_from_event_loop(|| {
            log::info!("asked to quit (--quit)");
            let _ = slint::quit_event_loop();
        });
    })
    .map_err(|e| log::warn!("quit requests not heard: {e}"))
    .ok();

    slint::run_event_loop_until_quit().map_err(|e| e.to_string())?;

    #[cfg(feature = "dev-hooks")]
    drop(dev_timers);
    engine_thread.shutdown();
    // Removes the tray icon before the process exits.
    UI.with(|u| u.borrow_mut().take());
    Ok(())
}
