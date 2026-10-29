//! How the helper finds the shell for the chained statusline command, and that it
//! passes the same input through and returns the command's output.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use uqt_assistant_anthropic::statusline::chain::{Shell, run};

/// A file system where only `existing` files are there.
fn only(existing: &'static [&'static str]) -> impl Fn(&Path) -> bool {
    move |p: &Path| existing.iter().any(|e| Path::new(e) == p)
}

#[test]
fn claude_codes_git_bash_setting_wins() {
    let shell = Shell::resolve(
        Some(OsString::from(r"D:\tools\Git\bin\bash.exe")),
        Some(OsString::from(r"C:\Program Files\Git\cmd")),
        only(&[
            r"D:\tools\Git\bin\bash.exe",
            r"C:\Program Files\Git\cmd\git.exe",
        ]),
    );
    assert_eq!(
        shell,
        Shell::bash(PathBuf::from(r"D:\tools\Git\bin\bash.exe"))
    );
}

#[test]
fn git_bash_is_found_beside_git_on_the_path() {
    let shell = Shell::resolve(
        None,
        Some(OsString::from(r"C:\Windows;C:\Program Files\Git\cmd")),
        only(&[
            r"C:\Program Files\Git\cmd\git.exe",
            r"C:\Program Files\Git\bin\bash.exe",
        ]),
    );
    assert_eq!(
        shell,
        Shell::bash(PathBuf::from(r"C:\Program Files\Git\bin\bash.exe"))
    );
}

#[test]
fn git_bash_is_found_from_the_mingw_folder_too() {
    let shell = Shell::resolve(
        None,
        Some(OsString::from(r"D:\Git\mingw64\bin")),
        only(&[r"D:\Git\mingw64\bin\git.exe", r"D:\Git\bin\bash.exe"]),
    );
    assert_eq!(shell, Shell::bash(PathBuf::from(r"D:\Git\bin\bash.exe")));
}

#[test]
fn without_git_bash_powershell_runs_the_command() {
    let shell = Shell::resolve(
        Some(OsString::from(r"D:\missing\bash.exe")),
        Some(OsString::from(r"C:\Windows")),
        only(&[]),
    );
    assert_eq!(shell, Shell::powershell());
}

#[test]
fn the_chained_command_gets_the_same_input_and_its_output_comes_back() {
    let out = run(
        &Shell::powershell(),
        "[Console]::In.ReadToEnd().Trim().ToUpper()",
        br#"{"model": "opus"}"#,
        Duration::from_secs(30),
    )
    .expect("runs");
    assert_eq!(String::from_utf8_lossy(&out).trim(), r#"{"MODEL": "OPUS"}"#);
}

#[test]
fn a_hanging_command_is_stopped() {
    let started = Instant::now();
    let _ = run(
        &Shell::powershell(),
        "Start-Sleep -Seconds 30",
        b"",
        Duration::from_millis(1500),
    );
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_shell_that_cannot_start_gives_nothing() {
    let shell = Shell::bash(PathBuf::from(r"Z:\no\such\bash.exe"));
    assert_eq!(run(&shell, "echo hi", b"", Duration::from_secs(1)), None);
}
