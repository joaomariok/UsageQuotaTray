//! The push channel: a named pipe on which small helper programs hand messages to the
//! running tray, for push Sources (a hook the Assistant calls) of any module.
//!
//! The channel only carries [`PushMessage`]s. What a payload means is up to the module
//! whose push Source accepts it ([`crate::contract::Source::accept_push`]); Core never
//! looks inside.

use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

/// Messages bigger than this are dropped unread.
pub const MAX_MESSAGE: usize = 64 * 1024;
const WIRE_VERSION: u64 = 1;
/// How long a sender waits for a busy pipe before giving up.
const BUSY_WAIT: Duration = Duration::from_millis(250);
const BUSY_STEP: Duration = Duration::from_millis(25);

/// One message for one push Source.
#[derive(Clone, Debug, PartialEq)]
pub struct PushMessage {
    /// The Assistant module's id.
    pub assistant: String,
    /// The push Source's id.
    pub source: String,
    /// Where the sender ran, in the module's own terms (such as a config folder), so
    /// the module can tell which Account it belongs to.
    pub origin: String,
    /// The module's data, opaque to Core.
    pub payload: Value,
}

impl PushMessage {
    pub fn encode(&self) -> Vec<u8> {
        json!({
            "v": WIRE_VERSION,
            "assistant": self.assistant,
            "source": self.source,
            "origin": self.origin,
            "payload": self.payload,
        })
        .to_string()
        .into_bytes()
    }

    /// `None` for anything that is not a message of this version.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let v: Value = serde_json::from_slice(bytes).ok()?;
        if v["v"].as_u64() != Some(WIRE_VERSION) {
            return None;
        }
        let text = |k: &str| v[k].as_str().map(str::to_string);
        Some(Self {
            assistant: text("assistant")?,
            source: text("source")?,
            origin: text("origin")?,
            payload: v.get("payload").cloned().unwrap_or(Value::Null),
        })
    }
}

/// The pipe of the tray instance that uses `data_dir`, so a measurement run with its
/// own app-data folder never receives the user's real pushes.
pub fn pipe_name(data_dir: &Path) -> String {
    // FNV-1a over the case-folded path.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data_dir.display().to_string().to_lowercase().bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!(r"\\.\pipe\UsageQuotaTray-push-{hash:016x}")
}

/// Sends one message and returns at once when no tray is listening. A busy pipe is
/// retried for a quarter of a second, then the message is dropped.
pub fn send(pipe: &str, message: &PushMessage) -> io::Result<()> {
    let bytes = message.encode();
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "message too large",
        ));
    }
    let mut waited = Duration::ZERO;
    let mut file = loop {
        match std::fs::OpenOptions::new().write(true).open(pipe) {
            Ok(f) => break f,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && waited < BUSY_WAIT => {
                std::thread::sleep(BUSY_STEP);
                waited += BUSY_STEP;
            }
            Err(e) => return Err(e),
        }
    };
    file.write_all(&bytes)?;
    file.flush()
}

const ERROR_PIPE_BUSY: i32 = 231;

/// Reads one connection's message, at most [`MAX_MESSAGE`] bytes.
fn read_message(mut reader: impl Read) -> Option<PushMessage> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_MESSAGE as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_MESSAGE {
        log::warn!("push message too large; dropped");
        return None;
    }
    PushMessage::decode(&bytes)
}

pub use server::PushServer;

#[cfg(windows)]
mod server {
    use std::fs::File;
    use std::io;
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::JoinHandle;

    use windows::Win32::Foundation::{
        CloseHandle, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
        LocalFree,
    };
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows::Win32::Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::{HSTRING, PWSTR};

    use super::{MAX_MESSAGE, PushMessage, read_message};

    /// Listens on the push pipe on its own thread and hands every valid message to
    /// `deliver`. Only the current user can connect; remote clients are refused.
    pub struct PushServer {
        name: String,
        stop: Arc<AtomicBool>,
        join: Option<JoinHandle<()>>,
    }

    /// The SDDL that lets only `sid` (full access) use the pipe; nothing inherited.
    pub fn security_sddl(sid: &str) -> String {
        format!("D:P(A;;GA;;;{sid})")
    }

    /// The current user's SID as a string (`S-1-5-21-...`).
    pub fn current_user_sid() -> io::Result<String> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
            let token = OwnedHandle::from_raw_handle(token.0);
            let raw = HANDLE(std::os::windows::io::AsRawHandle::as_raw_handle(&token));
            let mut len = 0u32;
            let _ = GetTokenInformation(raw, TokenUser, None, 0, &mut len);
            // u64 storage keeps the TOKEN_USER aligned.
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            GetTokenInformation(raw, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len)?;
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut text = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut text)?;
            let sid = text.to_string().map_err(io::Error::other);
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
            sid
        }
    }

    fn create_pipe(name: &str) -> io::Result<HANDLE> {
        let sddl = HSTRING::from(security_sddl(&current_user_sid()?));
        unsafe {
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &sddl,
                SDDL_REVISION_1,
                &mut sd,
                None,
            )?;
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            };
            let handle = CreateNamedPipeW(
                &HSTRING::from(name),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                0,
                MAX_MESSAGE as u32,
                0,
                Some(&attributes),
            );
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            if handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            Ok(handle)
        }
    }

    impl PushServer {
        /// Fails when the pipe exists already (another tray, or someone squatting on
        /// the name): the first instance only.
        pub fn spawn(
            name: &str,
            deliver: impl Fn(PushMessage) + Send + 'static,
        ) -> io::Result<Self> {
            let handle = create_pipe(name)?;
            // HANDLE is not Send; the thread owns it from here.
            let raw = handle.0 as usize;
            let stop = Arc::new(AtomicBool::new(false));
            let stopping = stop.clone();
            let join = std::thread::Builder::new()
                .name("push-channel".into())
                .spawn(move || {
                    let handle = HANDLE(raw as *mut _);
                    while !stopping.load(Ordering::Acquire) {
                        let connected = unsafe { ConnectNamedPipe(handle, None) };
                        // A client that connected before this call shows as "already
                        // connected", or as "no data" when it has closed again; its
                        // bytes may still be waiting either way.
                        let connected = match connected {
                            Ok(()) => true,
                            Err(e) => [ERROR_PIPE_CONNECTED, ERROR_NO_DATA]
                                .iter()
                                .any(|c| e.code() == c.to_hresult()),
                        };
                        if stopping.load(Ordering::Acquire) {
                            break;
                        }
                        if connected {
                            // Borrow the handle as a File for reading; never close it.
                            let file = std::mem::ManuallyDrop::new(unsafe {
                                File::from_raw_handle(handle.0)
                            });
                            match read_message(&*file) {
                                Some(m) => deliver(m),
                                None => log::warn!("unreadable push message dropped"),
                            }
                        }
                        let _ = unsafe { DisconnectNamedPipe(handle) };
                    }
                    let _ = unsafe { CloseHandle(handle) };
                })?;
            Ok(Self {
                name: name.to_string(),
                stop,
                join: Some(join),
            })
        }

        /// Stops listening and waits for the thread.
        pub fn stop(mut self) {
            self.shutdown();
        }

        fn shutdown(&mut self) {
            self.stop.store(true, Ordering::Release);
            let Some(join) = self.join.take() else { return };
            // Wake the blocked ConnectNamedPipe; the pipe may be busy for a moment
            // with a sender, so keep trying until the thread is gone.
            for _ in 0..200 {
                if join.is_finished() {
                    break;
                }
                let _ = std::fs::OpenOptions::new().write(true).open(&self.name);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            if join.is_finished() {
                let _ = join.join();
            } else {
                log::warn!("push channel thread did not stop");
            }
        }
    }

    impl Drop for PushServer {
        fn drop(&mut self) {
            self.shutdown();
        }
    }
}

#[cfg(windows)]
pub use server::{current_user_sid, security_sddl};
