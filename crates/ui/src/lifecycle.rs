//! The app's footprint on Windows: the Start-with-Windows entry, the notification
//! identity, removal from the PC, and one instance per session. Registry access goes
//! through [`Registry`] so the rules are tested on [`MemoryRegistry`]; only
//! [`CurrentUserRegistry`] touches HKCU.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE, WAIT_OBJECT_0,
    WIN32_ERROR,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, INFINITE, OpenMutexW, ResetEvent, SYNCHRONIZATION_SYNCHRONIZE,
    SetEvent, WaitForMultipleObjects,
};
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
    /// A file in the app-data folder that deleting the data must keep, with why: it
    /// records a change this copy did not undo that something else still relies on
    /// (another copy's statusline helper). `None` when nothing needs keeping.
    fn kept_in_data(&self) -> Option<(PathBuf, String)> {
        None
    }
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
    delete_entries(registry, data_dir, delete_data, &[]).map(|_| ())
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
/// the app (and its record of the change) is still there. Deleting the data keeps the
/// files a change still needs ([`OutsideChange::kept_in_data`]), and then says so as
/// a partial removal.
pub fn remove_from_pc_undoing(
    registry: &dyn Registry,
    changes: &[&dyn OutsideChange],
    data_dir: Option<&Path>,
    delete_data: bool,
) -> Result<(), RemovalError> {
    for change in changes.iter().filter(|c| c.pending().is_some()) {
        change.undo().map_err(RemovalError::NotUndone)?;
    }
    let keep: Vec<(PathBuf, String)> = changes.iter().filter_map(|c| c.kept_in_data()).collect();
    match delete_entries(registry, data_dir, delete_data, &keep) {
        Ok(kept) if kept.is_empty() => Ok(()),
        Ok(kept) => Err(RemovalError::Partly(io::Error::other(kept.join("; ")))),
        Err(e) => Err(RemovalError::Partly(e)),
    }
}

/// Deletes the entries and, when asked, the data folder except the files in `keep`
/// that are in it, returning why each kept file was kept.
fn delete_entries(
    registry: &dyn Registry,
    data_dir: Option<&Path>,
    delete_data: bool,
    keep: &[(PathBuf, String)],
) -> io::Result<Vec<String>> {
    registry.delete_value(RUN_KEY, APP_ID)?;
    registry.delete_tree(AUMID_KEY)?;
    let Some(dir) = data_dir.filter(|d| delete_data && d.exists()) else {
        return Ok(Vec::new());
    };
    let same =
        |a: &Path, b: &Path| fold(&a.display().to_string()) == fold(&b.display().to_string());
    let keep: Vec<&(PathBuf, String)> = keep
        .iter()
        .filter(|(file, _)| file.parent().is_some_and(|p| same(p, dir)) && file.exists())
        .collect();
    if keep.is_empty() {
        std::fs::remove_dir_all(dir)?;
        return Ok(Vec::new());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if keep.iter().any(|(file, _)| same(file, &path)) {
            continue;
        }
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(keep
        .iter()
        .map(|(file, why)| format!("kept {} because {why}", file.display()))
        .collect())
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

/// The mutex every running tray holds, whatever its data folder, so the installer
/// (`AppMutex` in `tools/installer/UsageQuotaTray.iss`) can tell the app is running.
pub const RUNNING_MUTEX: &str = "UsageQuotaTray.Running";

/// Holds a named mutex that any number of instances may share, while this one runs.
pub struct RunningMarker(HANDLE);

impl RunningMarker {
    /// `None` only when the mutex cannot be created at all.
    pub fn hold(name: &str) -> Option<Self> {
        unsafe { CreateMutexW(None, false, &HSTRING::from(name)) }
            .ok()
            .map(Self)
    }
}

impl Drop for RunningMarker {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// The event `usagequotatray.exe --quit` sets to ask every running tray to exit, as the
/// installer does before an upgrade or uninstall. Unprefixed like [`RUNNING_MUTEX`], so
/// it lives in this session's namespace: only trays in the same session see it.
pub const QUIT_EVENT: &str = "UsageQuotaTray.Quit";

/// A handle that moves to the listener thread (a `HANDLE` is a raw pointer).
struct SendHandle(HANDLE);
// SAFETY: a kernel handle may be used from any thread.
unsafe impl Send for SendHandle {}

fn quit_event(name: &str) -> io::Result<HANDLE> {
    // Manual reset: one request wakes every tray, until the requester resets it.
    unsafe { CreateEventW(None, true, false, &HSTRING::from(name)) }.map_err(io::Error::other)
}

/// Waits on a thread for a quit request ([`request_quit`]) and calls `on_quit` once.
/// Dropping it stops the wait without calling `on_quit`.
pub struct QuitListener {
    stop: HANDLE,
    thread: Option<JoinHandle<()>>,
}

impl QuitListener {
    pub fn spawn(event: &str, on_quit: impl FnOnce() + Send + 'static) -> io::Result<Self> {
        let quit = SendHandle(quit_event(event)?);
        let stop = match unsafe { CreateEventW(None, true, false, PCWSTR::null()) } {
            Ok(stop) => stop,
            Err(e) => {
                unsafe {
                    let _ = CloseHandle(quit.0);
                }
                return Err(io::Error::other(e));
            }
        };
        let stop_for_thread = SendHandle(stop);
        let thread = std::thread::spawn(move || {
            let (quit, stop) = (quit, stop_for_thread);
            let woke = unsafe { WaitForMultipleObjects(&[quit.0, stop.0], false, INFINITE) };
            unsafe {
                let _ = CloseHandle(quit.0);
            }
            if woke == WAIT_OBJECT_0 {
                on_quit();
            }
        });
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for QuitListener {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.stop);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        unsafe {
            let _ = CloseHandle(self.stop);
        }
    }
}

/// What [`request_quit`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitOutcome {
    /// No tray held the running marker.
    NoneRunning,
    /// Every tray exited.
    Closed,
    /// A tray still held the running marker when the time was up.
    StillRunning,
}

/// Whether any process holds the named mutex (what the installer's `AppMutex` checks).
fn mutex_exists(name: &str) -> bool {
    match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &HSTRING::from(name)) } {
        Ok(handle) => {
            unsafe {
                let _ = CloseHandle(handle);
            }
            true
        }
        Err(_) => false,
    }
}

/// Asks every tray in this session to exit (sets `event`) and waits up to `timeout`
/// for the `running` marker to be released. The request is withdrawn afterwards, so a
/// tray started later does not see it.
pub fn request_quit(event: &str, running: &str, timeout: Duration) -> QuitOutcome {
    if !mutex_exists(running) {
        return QuitOutcome::NoneRunning;
    }
    let Ok(handle) = quit_event(event) else {
        return QuitOutcome::StillRunning;
    };
    unsafe {
        let _ = SetEvent(handle);
    }
    let started = Instant::now();
    let outcome = loop {
        if !mutex_exists(running) {
            break QuitOutcome::Closed;
        }
        if started.elapsed() >= timeout {
            break QuitOutcome::StillRunning;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    unsafe {
        let _ = ResetEvent(handle);
        let _ = CloseHandle(handle);
    }
    outcome
}

/// The Apps & features entry Inno Setup writes for the installer's AppId
/// (`tools/installer/UsageQuotaTray.iss`): `<AppId>_is1` under the per-user Uninstall key.
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{9C02A488-5E56-4016-835D-BE4F735473FD}_is1";

/// The installer's uninstaller, when this copy in `exe_dir` is the installed one: the
/// Apps entry for our AppId names `exe_dir` as its `InstallLocation`, and the file its
/// `UninstallString` starts is there. A zip copy elsewhere has none, whatever it holds.
pub fn installed_uninstaller(registry: &dyn Registry, exe_dir: &Path) -> Option<PathBuf> {
    let location = registry.read_string(UNINSTALL_KEY, "InstallLocation")?;
    let trimmed = |p: &str| fold(p.trim().trim_end_matches(['\\', '/']));
    if trimmed(&location) != trimmed(&exe_dir.display().to_string()) {
        return None;
    }
    let command = registry.read_string(UNINSTALL_KEY, "UninstallString")?;
    let command = command.trim();
    let path = match command.strip_prefix('"') {
        Some(rest) => &rest[..rest.find('"')?],
        None => command.split(' ').next()?,
    };
    let path = PathBuf::from(path);
    path.is_file().then_some(path)
}
