//! Slint glue for the settings window. It is created when opened and dropped when
//! closed, so it costs nothing while hidden. What it shows comes from
//! [`crate::present::settings_view`]; every change goes back through the shell.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};
use uqt_core::settings::SettingsEdit;

use crate::nav::{NavKey, step};
use crate::present::{Input, RowKind, SettingRow, SettingsView};
use crate::{NavLogic, SettingRowData, SettingsWindow, platform, shell};

/// Typing in a text field applies after this pause (or at once on Enter).
const TYPING_PAUSE: Duration = Duration::from_millis(900);

struct Open {
    window: SettingsWindow,
    rows: Rc<VecModel<SettingRowData>>,
    view: SettingsView,
    pending: Option<SettingsEdit>,
    typing: Timer,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
}

fn row_data(r: &SettingRow) -> SettingRowData {
    SettingRowData {
        kind: match r.kind {
            RowKind::Assistant => 0,
            RowKind::Account => 1,
            RowKind::Meter => 2,
            RowKind::Toggle => 3,
            RowKind::Text => 4,
            RowKind::Note => 5,
        },
        label: r.label.as_str().into(),
        detail: r.detail.as_str().into(),
        checked: r.checked,
        enabled: r.enabled,
        editable: r.editable,
        text: r.text.as_str().into(),
        placeholder: r.placeholder.as_str().into(),
    }
}

/// Shows the window, creating it if needed.
pub fn open() {
    let shown = OPEN.with(|o| {
        o.borrow().as_ref().map(|open| {
            let _ = open.window.show();
            if let Some(hwnd) = platform::hwnd_of(open.window.window()) {
                platform::bring_to_front(hwnd);
            }
        })
    });
    if shown.is_some() {
        return;
    }
    let Some(view) = shell::settings_view() else {
        return;
    };
    let window = match SettingsWindow::new() {
        Ok(w) => w,
        Err(e) => {
            log::error!("settings window: {e}");
            return;
        }
    };
    let rows = Rc::new(VecModel::from(
        view.rows.iter().map(row_data).collect::<Vec<_>>(),
    ));
    window.set_rows(ModelRc::from(rows.clone()));
    window.set_version(env!("CARGO_PKG_VERSION").into());
    window.set_data_folder(shell::data_folder_text().into());
    set_scalars(&window, &view);
    wire(&window);
    let _ = window.show();
    if let Some(hwnd) = platform::hwnd_of(window.window()) {
        platform::set_dark_title_bar(hwnd, platform::apps_dark());
        platform::bring_to_front(hwnd);
    }
    OPEN.with(|o| {
        *o.borrow_mut() = Some(Open {
            window,
            rows,
            view,
            pending: None,
            typing: Timer::default(),
        })
    });
}

fn set_scalars(window: &SettingsWindow, view: &SettingsView) {
    let options: Vec<slint::SharedString> = view
        .poll_options
        .iter()
        .map(|o| o.as_str().into())
        .collect();
    window.set_poll_options(ModelRc::new(VecModel::from(options)));
    window.set_poll_index(view.poll_index as i32);
    window.set_poll_warning(view.poll_warning.clone().unwrap_or_default().into());
    window.set_reset_notifications(view.reset_notifications);
    window.set_start_with_windows(view.start_with_windows);
}

/// Re-renders an open window from the latest state and settings.
pub fn refresh() {
    let Some(view) = shell::settings_view() else {
        return;
    };
    OPEN.with(|o| {
        let mut o = o.borrow_mut();
        let Some(open) = o.as_mut() else { return };
        let data: Vec<SettingRowData> = view.rows.iter().map(row_data).collect();
        if data.len() == open.rows.row_count() {
            // Updating in place keeps focus in a text field being typed in.
            for (i, d) in data.into_iter().enumerate() {
                if open.rows.row_data(i).as_ref() != Some(&d) {
                    open.rows.set_row_data(i, d);
                }
            }
        } else {
            open.rows.set_vec(data);
        }
        set_scalars(&open.window, &view);
        open.view = view;
    });
}

fn row_edit(index: i32, input: Input) -> Option<SettingsEdit> {
    OPEN.with(|o| {
        o.borrow()
            .as_ref()?
            .view
            .rows
            .get(usize::try_from(index).ok()?)?
            .edit(input)
    })
}

fn apply_pending() {
    let edit = OPEN.with(|o| o.borrow_mut().as_mut().and_then(|open| open.pending.take()));
    if let Some(edit) = edit {
        shell::apply_edit(edit);
    }
}

fn wire(window: &SettingsWindow) {
    window.global::<NavLogic>().on_step(|current, count, code| {
        let at = usize::try_from(current).unwrap_or(0);
        let len = usize::try_from(count).unwrap_or(0);
        match NavKey::from_code(code) {
            Some(key) => step(at, len, key) as i32,
            None => current,
        }
    });
    window.on_row_toggled(|i, on| {
        if let Some(edit) = row_edit(i, Input::Toggled(on)) {
            shell::apply_edit(edit);
        } else {
            refresh();
        }
    });
    window.on_row_edited(|i, text| {
        let edit = row_edit(i, Input::Text(text.into()));
        OPEN.with(|o| {
            let mut o = o.borrow_mut();
            let Some(open) = o.as_mut() else { return };
            open.pending = edit;
            if open.pending.is_some() {
                open.typing
                    .start(TimerMode::SingleShot, TYPING_PAUSE, apply_pending);
            } else {
                open.typing.stop();
            }
        });
    });
    window.on_row_accepted(|i, text| {
        OPEN.with(|o| {
            if let Some(open) = o.borrow_mut().as_mut() {
                open.typing.stop();
                open.pending = None;
            }
        });
        match row_edit(i, Input::Text(text.into())) {
            Some(edit) => shell::apply_edit(edit),
            None => refresh(),
        }
    });
    window.on_poll_selected(|i| {
        let edit = OPEN.with(|o| {
            o.borrow()
                .as_ref()
                .and_then(|open| open.view.poll_edit(usize::try_from(i).ok()?))
        });
        if let Some(edit) = edit {
            shell::apply_edit(edit);
        }
    });
    window.on_reset_notifications_toggled(|on| {
        shell::apply_edit(SettingsEdit::ResetNotifications(on));
    });
    window.on_start_with_windows_toggled(shell::set_autostart);
    window.on_remove_requested(shell::remove_from_pc);
    window.window().on_close_requested(|| {
        apply_pending();
        // Dropped after this callback returns, never inside it.
        Timer::single_shot(Duration::ZERO, || {
            OPEN.with(|o| o.borrow_mut().take());
        });
        CloseRequestResponse::HideWindow
    });
}

/// The open window's handle, as the owner of message boxes.
pub fn hwnd() -> Option<windows::Win32::Foundation::HWND> {
    OPEN.with(|o| {
        o.borrow()
            .as_ref()
            .and_then(|open| platform::hwnd_of(open.window.window()))
    })
}
