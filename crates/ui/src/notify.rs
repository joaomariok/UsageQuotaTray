//! Toasts through the Windows notification API under the app's AppUserModelID, with a
//! tray balloon when a toast cannot be shown. Win32/WinRT glue; the text comes from
//! [`crate::present::alert_message`] and the XML from [`crate::present::toast_xml`].
//! Not unit-tested: attribution, icon and click handling are checked by eye.

use std::collections::VecDeque;

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager, ToastNotifier};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{
    NIF_INFO, NIIF_INFO, NIM_MODIFY, NOTIFYICONDATAW, SetCurrentProcessExplicitAppUserModelID,
    Shell_NotifyIconW,
};
use windows::core::{HSTRING, IInspectable};

use crate::lifecycle::APP_ID;
use crate::present::{AlertMessage, toast_xml};

/// Toasts kept alive so their click handlers still run from the Action Center.
const KEEP_TOASTS: usize = 16;
/// `tray-icon` numbers its icons from 1 and this app creates exactly one.
const TRAY_ICON_ID: u32 = 1;

/// Sets the explicit AppUserModelID so Windows attributes toasts and the taskbar to the
/// registered identity.
pub fn set_app_id() {
    if let Err(e) = unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_ID)) } {
        log::warn!("could not set the AppUserModelID: {e}");
    }
}

pub struct Toasts {
    notifier: Option<ToastNotifier>,
    shown: VecDeque<ToastNotification>,
}

impl Toasts {
    /// The notifier is created on the first toast, so an app that never alerts never
    /// loads the notification runtime.
    pub fn new() -> Self {
        Self {
            notifier: None,
            shown: VecDeque::new(),
        }
    }

    /// Shows a toast whose click calls `on_click` (on a system thread).
    pub fn show(
        &mut self,
        message: &AlertMessage,
        on_click: impl Fn() + Send + 'static,
    ) -> windows::core::Result<()> {
        if self.notifier.is_none() {
            self.notifier = Some(ToastNotificationManager::CreateToastNotifierWithId(
                &HSTRING::from(APP_ID),
            )?);
        }
        let Some(notifier) = &self.notifier else {
            return Ok(());
        };
        let xml = XmlDocument::new()?;
        xml.LoadXml(&HSTRING::from(toast_xml(message)))?;
        let toast = ToastNotification::CreateToastNotification(&xml)?;
        toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
            move |_, _| {
                on_click();
                Ok(())
            },
        ))?;
        notifier.Show(&toast)?;
        self.shown.push_back(toast);
        while self.shown.len() > KEEP_TOASTS {
            self.shown.pop_front();
        }
        Ok(())
    }
}

fn copy_wide(dst: &mut [u16], text: &str) {
    let units: Vec<u16> = text.encode_utf16().take(dst.len() - 1).collect();
    dst[..units.len()].copy_from_slice(&units);
    dst[units.len()] = 0;
}

/// The fallback: a balloon on the tray icon owned by `hwnd`.
pub fn balloon(hwnd: HWND, message: &AlertMessage) -> bool {
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        uFlags: NIF_INFO,
        dwInfoFlags: NIIF_INFO,
        ..Default::default()
    };
    copy_wide(&mut nid.szInfoTitle, &message.title);
    copy_wide(&mut nid.szInfo, &message.body);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) }.as_bool()
}
