//! The app's footprint on Windows: the Start-with-Windows entry, the notification
//! identity, removal from the PC, and one instance per session. Registry access goes
//! through [`Registry`] so the rules are tested on [`MemoryRegistry`]; only
//! [`CurrentUserRegistry`] touches HKCU.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE, WIN32_ERROR,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::{HSTRING, PCWSTR};

/// The Run value name, the AppUserModelID and the notification display name.
pub const APP_ID: &str = "UsageQuotaTray";
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Where unpackaged apps register their notification identity (the Windows App SDK
/// does the same; undocumented for plain Win32, hence the tray balloon fallback).
pub const AUMID_KEY: &str = r"Software\Classes\AppUserModelId\UsageQuotaTray";

/// String values under HKEY_CURRENT_USER.
pub trait Registry {
    fn read_string(&self, key: &str, name: &str) -> Option<String>;
    /// Creates the key when it is missing.
    fn write_string(&self, key: &str, name: &str, value: &str) -> io::Result<()>;
    /// Succeeds when the value is already gone.
    fn delete_value(&self, key: &str, name: &str) -> io::Result<()>;
    /// Deletes the key with its values and subkeys; succeeds when it is already gone.
    fn delete_tree(&self, key: &str) -> io::Result<()>;
}

/// An in-memory registry for tests. Key paths compare case-insensitively, as in Windows.
#[derive(Debug, Default)]
pub struct MemoryRegistry {
    values: Mutex<BTreeMap<(String, String), String>>,
}

impl MemoryRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, String), String>> {
        self.values.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn fold(s: &str) -> String {
    s.to_lowercase()
}

impl Registry for MemoryRegistry {
    fn read_string(&self, key: &str, name: &str) -> Option<String> {
        self.lock().get(&(fold(key), fold(name))).cloned()
    }

    fn write_string(&self, key: &str, name: &str, value: &str) -> io::Result<()> {
        self.lock()
            .insert((fold(key), fold(name)), value.to_string());
        Ok(())
    }

    fn delete_value(&self, key: &str, name: &str) -> io::Result<()> {
        self.lock().remove(&(fold(key), fold(name)));
        Ok(())
    }

    fn delete_tree(&self, key: &str) -> io::Result<()> {
        let key = fold(key);
        let sub = format!("{key}\\");
        self.lock()
            .retain(|(k, _), _| *k != key && !k.starts_with(&sub));
        Ok(())
    }
}

/// The real HKEY_CURRENT_USER.
pub struct CurrentUserRegistry;

fn check(err: WIN32_ERROR) -> io::Result<()> {
    if err.is_ok() || err == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(err.0 as i32))
    }
}

impl Registry for CurrentUserRegistry {
    fn read_string(&self, key: &str, name: &str) -> Option<String> {
        let (key, name) = (HSTRING::from(key), HSTRING::from(name));
        let mut size = 0u32;
        let probe = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &key,
                &name,
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size),
            )
        };
        if probe.is_err() || size == 0 {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        let err = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &key,
                &name,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if err.is_err() {
            return None;
        }
        let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    fn write_string(&self, key: &str, name: &str, value: &str) -> io::Result<()> {
        let mut hkey = HKEY::default();
        check(unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                &HSTRING::from(key),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut hkey,
                None,
            )
        })?;
        let data: Vec<u8> = value
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        let result =
            check(unsafe { RegSetValueExW(hkey, &HSTRING::from(name), None, REG_SZ, Some(&data)) });
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        result
    }

    fn delete_value(&self, key: &str, name: &str) -> io::Result<()> {
        check(unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name))
        })
    }

    fn delete_tree(&self, key: &str) -> io::Result<()> {
        // With a subkey, RegDeleteTreeW removes the subkey itself too.
        check(unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key)) })
    }
}

/// The current user's Run entry for this exe.
pub struct Autostart<'a> {
    registry: &'a dyn Registry,
    command: String,
}

impl<'a> Autostart<'a> {
    pub fn new(registry: &'a dyn Registry, exe: &Path) -> Self {
        Self {
            registry,
            command: format!("\"{}\"", exe.display()),
        }
    }

    /// On only when the entry starts this exe; an entry left by a moved copy is off.
    pub fn is_enabled(&self) -> bool {
        self.registry
            .read_string(RUN_KEY, APP_ID)
            .is_some_and(|v| v.eq_ignore_ascii_case(&self.command))
    }

    pub fn set(&self, on: bool) -> io::Result<()> {
        if on {
            self.registry.write_string(RUN_KEY, APP_ID, &self.command)
        } else {
            self.registry.delete_value(RUN_KEY, APP_ID)
        }
    }
}

/// Registers the toast identity: display name and icon shown on every toast.
pub fn register_notifications(registry: &dyn Registry, icon: &Path) -> io::Result<()> {
    registry.write_string(AUMID_KEY, "DisplayName", APP_ID)?;
    registry.write_string(AUMID_KEY, "IconUri", &icon.display().to_string())
}

/// A change UsageQuotaTray made to another program's files on the user's request (the
/// statusline helper in Claude Code's settings), which removal undoes first. The app
/// crate supplies these; the UI never names an Assistant.
pub trait OutsideChange: std::fmt::Debug {
    /// What undoing it does, for the confirmation ("put back Claude Code's original
    /// statusLine setting"); `None` when there is nothing to undo now.
    fn pending(&self) -> Option<String>;
    /// Undoes it. The error is shown to the user.
    fn undo(&self) -> Result<(), String>;
}

/// The confirmation for "Remove UsageQuotaTray from this PC": what is undone first,
/// then what is deleted.
pub fn removal_question(changes: &[&dyn OutsideChange], data_folder: Option<&str>) -> String {
    let first: Vec<String> = changes.iter().filter_map(|c| c.pending()).collect();
    let mut text = String::from("This will ");
    if !first.is_empty() {
        text.push_str(&format!("first {}, then ", first.join(" and ")));
    }
    match data_folder {
        Some(folder) => text.push_str(&format!(
            "delete the Start with Windows entry, the notification registration and the \
             folder {folder}"
        )),
        None => {
            text.push_str("delete the Start with Windows entry and the notification registration")
        }
    }
    text.push_str(", and quit UsageQuotaTray.\n\nRemove it now?");
    text
}

/// "Remove UsageQuotaTray from this PC": the Run value, the notification identity and,
/// when asked, the app-data folder (settings, logs, Badge overrides).
pub fn remove_from_pc(
    registry: &dyn Registry,
    data_dir: Option<&Path>,
    delete_data: bool,
) -> io::Result<()> {
    delete_entries(registry, data_dir, delete_data)
}

/// Why a removal stopped.
#[derive(Debug)]
pub enum RemovalError {
    /// A change could not be undone; nothing was deleted.
    NotUndone(String),
    /// The changes were undone, but deleting the app's entries failed partway.
    Partly(io::Error),
}

impl std::fmt::Display for RemovalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotUndone(e) => write!(f, "Nothing was removed: {e}"),
            Self::Partly(e) => write!(f, "Not everything could be removed: {e}"),
        }
    }
}

/// [`remove_from_pc`] after undoing `changes` that have something pending. When one
/// cannot be undone nothing is deleted, so the user can fix it and try again while
/// the app (and its record of the change) is still there.
pub fn remove_from_pc_undoing(
    registry: &dyn Registry,
    changes: &[&dyn OutsideChange],
    data_dir: Option<&Path>,
    delete_data: bool,
) -> Result<(), RemovalError> {
    for change in changes.iter().filter(|c| c.pending().is_some()) {
        change.undo().map_err(RemovalError::NotUndone)?;
    }
    delete_entries(registry, data_dir, delete_data).map_err(RemovalError::Partly)
}

fn delete_entries(
    registry: &dyn Registry,
    data_dir: Option<&Path>,
    delete_data: bool,
) -> io::Result<()> {
    registry.delete_value(RUN_KEY, APP_ID)?;
    registry.delete_tree(AUMID_KEY)?;
    if delete_data
        && let Some(dir) = data_dir
        && dir.exists()
    {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

/// The single-instance name for a data folder: one instance per session and folder, so
/// a measurement run with a throwaway folder can run beside the real app.
pub fn instance_name(data_dir: &Path) -> String {
    // FNV-1a over the case-folded path.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in fold(&data_dir.display().to_string()).bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("Local\\{APP_ID}-{hash:016x}")
}

/// Holds a named mutex while this instance runs.
pub struct SingleInstance(HANDLE);

impl SingleInstance {
    /// `None` when another instance holds `name`.
    pub fn acquire(name: &str) -> Option<Self> {
        let handle = unsafe { CreateMutexW(None, false, &HSTRING::from(name)) }.ok()?;
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return None;
        }
        Some(Self(handle))
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
