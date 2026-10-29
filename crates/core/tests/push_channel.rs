//! The push channel over a real named pipe with a name unique to each test. No tray
//! and no user data are involved.

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::json;
use uqt_core::push::{self, PushMessage, PushServer};

fn unique_pipe(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!(r"\\.\pipe\uqt-test-{tag}-{}-{nanos}", std::process::id())
}

fn message(n: u32) -> PushMessage {
    PushMessage {
        assistant: "pushy".into(),
        source: "hook".into(),
        origin: "here".into(),
        payload: json!({ "count": n }),
    }
}

#[test]
fn a_sent_message_reaches_the_server() {
    let pipe = unique_pipe("deliver");
    let (tx, rx) = mpsc::channel();
    let server = PushServer::spawn(&pipe, move |m| {
        let _ = tx.send(m);
    })
    .expect("server starts");
    for n in 1..=3 {
        push::send(&pipe, &message(n)).expect("sent");
        let got = rx.recv_timeout(Duration::from_secs(5)).expect("delivered");
        assert_eq!(got, message(n));
    }
    server.stop();
}

#[test]
fn sending_without_a_server_fails_at_once() {
    let pipe = unique_pipe("absent");
    let started = Instant::now();
    assert!(push::send(&pipe, &message(1)).is_err());
    assert!(started.elapsed() < Duration::from_millis(200));
}

#[test]
fn garbage_is_dropped_and_the_server_keeps_listening() {
    let pipe = unique_pipe("garbage");
    let (tx, rx) = mpsc::channel();
    let server = PushServer::spawn(&pipe, move |m| {
        let _ = tx.send(m);
    })
    .unwrap();
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).open(&pipe).unwrap();
        f.write_all(b"{ definitely not a message").unwrap();
    }
    push::send(&pipe, &message(9)).expect("sent after garbage");
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), message(9));
    server.stop();
}

#[test]
fn a_second_server_on_the_same_name_is_refused() {
    let pipe = unique_pipe("twice");
    let first = PushServer::spawn(&pipe, |_| {}).unwrap();
    assert!(PushServer::spawn(&pipe, |_| {}).is_err());
    first.stop();
}

#[test]
fn only_the_current_user_may_connect() {
    let sid = push::current_user_sid().expect("sid");
    assert!(sid.starts_with("S-1-5-"), "{sid}");
    // Protected DACL, one entry: full access for this user. No Everyone (WD),
    // Authenticated Users (AU), Builtin Users (BU) or Anonymous (AN).
    assert_eq!(push::security_sddl(&sid), format!("D:P(A;;GA;;;{sid})"));
}

#[test]
fn each_app_data_folder_gets_its_own_pipe() {
    let a = push::pipe_name(Path::new(r"C:\Users\me\AppData\Local\UsageQuotaTray"));
    let b = push::pipe_name(Path::new(r"c:\users\ME\appdata\local\usagequotatray"));
    let c = push::pipe_name(Path::new(r"D:\measure\UsageQuotaTray"));
    assert_eq!(a, b, "case does not matter");
    assert_ne!(a, c);
    assert!(a.starts_with(r"\\.\pipe\UsageQuotaTray-push-"), "{a}");
}
