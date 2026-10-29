//! Thin Win32 helpers: theme, accent, DPI, work area, DWM attributes, time zone.
//! Not unit-tested; checked with the measurement scripts and by eye.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::UI::ViewManagement::{UIColorType, UISettings};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWINDOWATTRIBUTE, DwmSetWindowAttribute};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::SystemServices::TIME_ZONE_ID_DAYLIGHT;
use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetSystemMetricsForDpi,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, IDYES, MB_DEFBUTTON2, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONWARNING,
    MB_OK, MB_YESNO, MessageBoxW, SM_CXSMICON, SPI_GETWORKAREA,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetForegroundWindow, SystemParametersInfoW,
};
use windows::core::{HSTRING, PCWSTR, w};

const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

// DWMWINDOWATTRIBUTE values (dwmapi.h).
const DWMWA_USE_IMMERSIVE_DARK_MODE: i32 = 20;
const DWMWA_WINDOW_CORNER_PREFERENCE: i32 = 33;
const DWMWA_SYSTEMBACKDROP_TYPE: i32 = 38;
const DWMWCP_ROUND: i32 = 2;
const DWMSBT_TRANSIENTWINDOW: i32 = 3;

pub fn set_per_monitor_dpi_awareness() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn reg_dword(subkey: PCWSTR, name: PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = 4u32;
    let err = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    err.is_ok().then_some(data)
}

pub fn apps_dark() -> bool {
    reg_dword(PERSONALIZE, w!("AppsUseLightTheme")) == Some(0)
}

pub fn taskbar_dark() -> bool {
    // Windows 11's default taskbar is dark; only an explicit 1 means light.
    reg_dword(PERSONALIZE, w!("SystemUsesLightTheme")) != Some(1)
}

/// The system accent for bars: the light variant on dark backgrounds.
pub fn accent(dark: bool) -> slint::Color {
    let kind = if dark {
        UIColorType::AccentLight2
    } else {
        UIColorType::AccentDark1
    };
    if let Ok(c) = UISettings::new().and_then(|s| s.GetColorValue(kind)) {
        return slint::Color::from_rgb_u8(c.R, c.G, c.B);
    }
    // DWM AccentColor is 0xAABBGGRR.
    let v = reg_dword(w!("Software\\Microsoft\\Windows\\DWM"), w!("AccentColor"))
        .unwrap_or(0xFFD4_7800);
    slint::Color::from_rgb_u8(v as u8, (v >> 8) as u8, (v >> 16) as u8)
}

/// The small-icon size for the system DPI (16 px at 100%, 32 px at 200%).
pub fn small_icon_size() -> u32 {
    let dpi = unsafe { GetDpiForSystem() };
    unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) }.max(16) as u32
}

pub fn work_area() -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut r as *mut RECT as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    r
}

/// Local time minus UTC, in seconds, as of now.
pub fn utc_offset_secs() -> i64 {
    let mut tz = TIME_ZONE_INFORMATION::default();
    let id = unsafe { GetTimeZoneInformation(&mut tz) };
    let bias = if id == TIME_ZONE_ID_DAYLIGHT {
        tz.Bias + tz.DaylightBias
    } else {
        tz.Bias + tz.StandardBias
    };
    -i64::from(bias) * 60
}

pub fn hwnd_of(window: &slint::Window) -> Option<HWND> {
    let handle = window.window_handle();
    let h = handle.window_handle().ok()?;
    let RawWindowHandle::Win32(w) = h.as_raw() else {
        return None;
    };
    Some(HWND(w.hwnd.get() as *mut _))
}

fn dwm_set(hwnd: HWND, attr: i32, value: i32) -> bool {
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWINDOWATTRIBUTE(attr),
            &value as *const i32 as *const _,
            4,
        )
        .is_ok()
    }
}

/// Rounded corners, dark frame when dark, and the acrylic backdrop. Returns whether
/// the backdrop was accepted.
pub fn apply_flyout_style(hwnd: HWND, dark: bool) -> bool {
    dwm_set(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND);
    dwm_set(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, i32::from(dark));
    dwm_set(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_TRANSIENTWINDOW)
}

/// A dark title bar for a framed window when the apps theme is dark.
pub fn set_dark_title_bar(hwnd: HWND, dark: bool) {
    dwm_set(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, i32::from(dark));
}

/// A Yes/No question; true for Yes.
pub fn confirm(owner: Option<HWND>, title: &str, text: &str) -> bool {
    let (title, text) = (HSTRING::from(title), HSTRING::from(text));
    unsafe {
        MessageBoxW(
            owner,
            &text,
            &title,
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        ) == IDYES
    }
}

/// A message the user should see.
pub fn show_info(owner: Option<HWND>, title: &str, text: &str) {
    let (title, text) = (HSTRING::from(title), HSTRING::from(text));
    unsafe {
        MessageBoxW(owner, &text, &title, MB_OK | MB_ICONINFORMATION);
    }
}

/// An error the user should see.
pub fn show_error(owner: Option<HWND>, title: &str, text: &str) {
    let (title, text) = (HSTRING::from(title), HSTRING::from(text));
    unsafe {
        MessageBoxW(owner, &text, &title, MB_OK | MB_ICONERROR);
    }
}

pub fn is_foreground(hwnd: HWND) -> bool {
    unsafe { GetForegroundWindow() == hwnd }
}

pub fn bring_to_front(hwnd: HWND) {
    unsafe {
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Opens an `https://` page in the default browser. Anything else is refused.
pub fn open_url(url: &str) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if !url.to_ascii_lowercase().starts_with("https://") {
        log::warn!("refusing to open a non-https page");
        return;
    }
    let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: both strings are NUL-terminated and outlive the call.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    if result.0 as usize <= 32 {
        log::warn!("could not open the usage page ({})", result.0 as usize);
    }
}
