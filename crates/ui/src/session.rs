//! Watches whether the user is present: session lock and unlock, suspend and resume,
//! and input idleness. Runs a hidden top-level window on its own thread (power
//! broadcasts only reach top-level windows) and forwards changes to the engine.

use std::cell::RefCell;

use uqt_core::engine::{Presence, RefreshReason};
use uqt_core::runner::EngineHandle;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, PBT_APMRESUMEAUTOMATIC,
    PBT_APMSUSPEND, RegisterClassW, SetTimer, WINDOW_EX_STYLE, WM_POWERBROADCAST, WM_TIMER,
    WM_WTSSESSION_CHANGE, WNDCLASSW, WS_OVERLAPPED, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
};
use windows::core::w;

/// No input for this long means idle.
const IDLE_AFTER_MS: u32 = 5 * 60 * 1000;
const IDLE_CHECK_MS: u32 = 60 * 1000;

struct Monitor {
    engine: EngineHandle,
    locked: bool,
    suspended: bool,
    last: Presence,
}

impl Monitor {
    fn publish(&mut self, refresh: Option<RefreshReason>) {
        let now = if self.locked || self.suspended {
            Presence::Paused
        } else if input_idle() {
            Presence::Idle
        } else {
            Presence::Active
        };
        if now != self.last {
            log::info!("presence {:?} -> {now:?}", self.last);
            self.last = now;
            self.engine.set_presence(now);
        }
        if let Some(reason) = refresh.filter(|_| now != Presence::Paused) {
            self.engine.refresh(reason);
        }
    }
}

thread_local! {
    static MONITOR: RefCell<Option<Monitor>> = const { RefCell::new(None) };
}

fn input_idle() -> bool {
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return false;
    }
    unsafe { GetTickCount() }.wrapping_sub(info.dwTime) >= IDLE_AFTER_MS
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let handled = MONITOR.with(|m| {
        let mut guard = m.borrow_mut();
        let Some(m) = guard.as_mut() else {
            return false;
        };
        match msg {
            WM_WTSSESSION_CHANGE => match wparam.0 as u32 {
                WTS_SESSION_LOCK => {
                    m.locked = true;
                    m.publish(None);
                }
                WTS_SESSION_UNLOCK => {
                    m.locked = false;
                    m.publish(Some(RefreshReason::Unlock));
                }
                _ => {}
            },
            WM_POWERBROADCAST => match wparam.0 as u32 {
                PBT_APMSUSPEND => {
                    m.suspended = true;
                    m.publish(None);
                }
                PBT_APMRESUMEAUTOMATIC => {
                    m.suspended = false;
                    m.publish(Some(RefreshReason::Resume));
                }
                _ => {}
            },
            WM_TIMER => m.publish(None),
            _ => return false,
        }
        true
    });
    if handled && msg != WM_POWERBROADCAST {
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Starts the presence monitor thread. It lives as long as the process.
pub fn spawn_presence_monitor(engine: EngineHandle) {
    let spawned = std::thread::Builder::new()
        .name("presence".into())
        .spawn(move || {
            MONITOR.with(|m| {
                *m.borrow_mut() = Some(Monitor {
                    engine,
                    locked: false,
                    suspended: false,
                    last: Presence::Active,
                })
            });
            let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
                return;
            };
            let class = w!("UsageQuotaTrayPresence");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: module.into(),
                lpszClassName: class,
                ..Default::default()
            };
            unsafe { RegisterClassW(&wc) };
            let hwnd = match unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    class,
                    w!(""),
                    WS_OVERLAPPED,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    Some(module.into()),
                    None,
                )
            } {
                Ok(h) => h,
                Err(e) => {
                    log::error!("presence window: {e}");
                    return;
                }
            };
            if let Err(e) = unsafe { WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) }
            {
                log::warn!("session notifications unavailable: {e}");
            }
            unsafe { SetTimer(Some(hwnd), 1, IDLE_CHECK_MS, None) };
            let mut msg = MSG::default();
            // GetMessageW returns -1 on error and 0 on WM_QUIT.
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
                unsafe { DispatchMessageW(&msg) };
            }
        });
    if let Err(e) = spawned {
        log::error!("presence thread: {e}");
    }
}
