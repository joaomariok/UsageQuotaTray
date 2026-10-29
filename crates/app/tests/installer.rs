//! The installer script agrees with the app: the Start-with-Windows value it writes is
//! the one the app reads back as on, it waits on the mutex the tray holds and first
//! asks the tray to quit, its shortcut carries the app's notification identity, and
//! its uninstaller runs the app's headless removal and goes on only on the exit codes
//! that mean the statusLine is back. Reads `tools/installer/UsageQuotaTray.iss` with
//! its comments stripped, so a commented-out line never passes; installs nothing.

use std::path::Path;

use uqt_app::cli;
use uqt_ui::lifecycle::{
    APP_ID, Autostart, MemoryRegistry, QUIT_EVENT, RUN_KEY, RUNNING_MUTEX, Registry, UNINSTALL_KEY,
};

/// The script without comments.
fn script() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/installer/UsageQuotaTray.iss");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    strip_comments(&text)
}

/// Drops `;` comment lines before [Code], and `//`, `{ }` and `(* *)` comments inside
/// it (outside strings).
fn strip_comments(text: &str) -> String {
    let (sections, code) = match text.find("\n[Code]") {
        Some(at) => text.split_at(at + 1),
        None => (text, ""),
    };
    let mut out: String = sections
        .lines()
        .filter(|l| !l.trim_start().starts_with(';'))
        .map(|l| format!("{l}\n"))
        .collect();
    let chars: Vec<char> = code.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if in_string {
            out.push(c);
            in_string = c != '\'';
            i += 1;
        } else if c == '\'' {
            in_string = true;
            out.push(c);
            i += 1;
        } else if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '{' {
            while i < chars.len() && chars[i] != '}' {
                i += 1;
            }
            i += 1;
        } else if c == '(' && next == Some('*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == ')') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

#[test]
fn comments_are_stripped_but_strings_are_kept() {
    let text = "[Setup]\n; AppMutex=X\nAppMutex=Y\n[Code]\n// a\n{ b }\n(* c *)\n\
                x := '{app} // not a comment';\n";
    let stripped = strip_comments(text);
    assert!(!stripped.contains("AppMutex=X"), "{stripped}");
    assert!(stripped.contains("AppMutex=Y"), "{stripped}");
    for gone in ["// a", "{ b }", "(* c *)"] {
        assert!(!stripped.contains(gone), "{stripped}");
    }
    assert!(
        stripped.contains("x := '{app} // not a comment';"),
        "{stripped}"
    );
}

/// Whitespace runs as one space.
fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Words outside strings, with their byte offsets.
fn word_spans(text: &str) -> Vec<(usize, &str)> {
    let mut words = Vec::new();
    let mut in_string = false;
    let mut start = None;
    for (at, c) in text.char_indices() {
        if c == '\'' {
            in_string = !in_string;
        }
        let word_char = !in_string && (c.is_ascii_alphanumeric() || c == '_');
        match (word_char, start) {
            (true, None) => start = Some(at),
            (false, Some(s)) => {
                words.push((s, &text[s..at]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        words.push((s, &text[s..]));
    }
    words
}

/// The statements of a [Code] procedure or function, between its `begin` and its own
/// `end`, whitespace squeezed.
fn body(script: &str, name: &str) -> String {
    let code = &script[script.find("[Code]").expect("a [Code] section")..];
    let header = ["procedure ", "function "]
        .iter()
        .find_map(|kind| {
            let needle = format!("{kind}{name}");
            code.match_indices(&needle)
                .map(|(at, _)| at)
                .find(|&at| code[at + needle.len()..].starts_with(['(', ';', ':']))
        })
        .unwrap_or_else(|| panic!("no procedure or function {name}"));
    let rest = &code[header..];
    let words = word_spans(rest);
    let begin = words
        .iter()
        .position(|(_, w)| w.eq_ignore_ascii_case("begin"))
        .expect("begin");
    let mut depth = 0;
    for (at, word) in &words[begin..] {
        if word.eq_ignore_ascii_case("begin") || word.eq_ignore_ascii_case("case") {
            depth += 1;
        } else if word.eq_ignore_ascii_case("end") {
            depth -= 1;
            if depth == 0 {
                let start = words[begin].0 + "begin".len();
                return squeeze(&rest[start..*at]);
            }
        }
    }
    panic!("no end for {name}")
}

/// The value of a `Name = value;` constant in [Code].
fn constant(script: &str, name: &str) -> String {
    let code = &script[script.find("[Code]").expect("a [Code] section")..];
    let prefix = format!("{name} =");
    let line = code
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no constant {name}"));
    line[prefix.len()..]
        .trim()
        .trim_end_matches(';')
        .trim()
        .to_string()
}

/// An Inno Setup parameter value: quoted, embedded quotes doubled.
fn inno_quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// The script's non-empty lines.
fn lines(script: &str) -> Vec<&str> {
    script
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
}

#[test]
fn the_run_value_is_written_exactly_as_the_app_writes_it() {
    let reg = MemoryRegistry::new();
    let exe = Path::new(r"{app}\usagequotatray.exe");
    Autostart::new(&reg, exe).set(true).unwrap();
    let app_value = reg.read_string(RUN_KEY, APP_ID).unwrap();
    // A known-good literal, independent of how the app builds it.
    assert_eq!(app_value, r#""{app}\usagequotatray.exe""#);

    let script = script();
    let entry = lines(&script)
        .into_iter()
        .find(|l| {
            l.starts_with("Root: HKCU;") && l.contains(&format!("Subkey: {}", inno_quoted(RUN_KEY)))
        })
        .expect("a [Registry] entry under the Run key");
    assert!(
        entry.contains(&format!("ValueName: {}", inno_quoted(APP_ID))),
        "{entry}"
    );
    assert!(entry.contains("ValueType: string"), "{entry}");
    assert!(
        entry.contains(&format!("ValueData: {}", inno_quoted(&app_value))),
        "{entry}"
    );
    assert!(entry.contains("uninsdeletevalue"), "{entry}");
    assert!(entry.ends_with("Check: ShouldWriteRunValue"), "{entry}");
    // An upgrade compares the existing value with the installed exe in the app's form.
    let init = body(&script, "InitializeSetup");
    assert!(
        init.contains("RegQueryStringValue(HKCU, RunKey, RunValueName, RunData)"),
        "{init}"
    );
    assert!(
        init.contains(r#"SameText(RunData, '"' + OldExe + '"')"#),
        "{init}"
    );
    assert_eq!(constant(&script, "RunKey"), format!("'{RUN_KEY}'"));
    assert_eq!(constant(&script, "RunValueName"), format!("'{APP_ID}'"));
}

#[test]
fn start_with_windows_is_offered_on_a_fresh_install_only_and_kept_as_the_app_left_it() {
    let script = script();
    let lines = lines(&script);
    assert!(lines.contains(&"UsePreviousTasks=no"));
    for task in ["startwithwindows", "launchnow"] {
        let line = lines
            .iter()
            .find(|l| l.starts_with(&format!("Name: \"{task}\";")))
            .unwrap_or_else(|| panic!("task {task}"));
        assert!(line.ends_with("Check: IsFreshInstall"), "{line}");
    }
    assert_eq!(body(&script, "IsFreshInstall"), "Result := not IsUpgrade;");
    assert_eq!(
        body(&script, "ShouldWriteRunValue"),
        "if IsUpgrade then Result := RunValueWasOurs else Result := \
         WizardIsTaskSelected('startwithwindows');"
    );
    assert_eq!(
        constant(&script, "UninstallKey"),
        format!("'{UNINSTALL_KEY}'")
    );
    let init = body(&script, "InitializeSetup");
    assert!(
        init.contains("IsUpgrade := RegKeyExists(HKCU, UninstallKey);"),
        "{init}"
    );
    assert!(
        init.contains("RunValueWasOurs := IsUpgrade and (OldExe <> '') and"),
        "{init}"
    );
}

#[test]
fn an_upgrade_closes_the_running_tray_first_and_starts_it_again_only_if_it_ran() {
    let script = script();
    let quit = body(&script, "QuitRunningTray");
    assert!(
        quit.contains(&format!(
            "if Exec(Exe, '{}', '', SW_HIDE, ewWaitUntilTerminated, ResultCode)",
            cli::QUIT
        )),
        "{quit}"
    );
    let init = body(&script, "InitializeSetup");
    let was_running = init
        .find(&format!(
            "WasRunning := CheckForMutexes('{RUNNING_MUTEX}');"
        ))
        .expect("WasRunning recorded");
    let quits = init
        .find("if WasRunning and IsUpgrade then QuitRunningTray(OldExe);")
        .expect("--quit on an upgrade");
    assert!(was_running < quits, "{init}");
    assert_eq!(
        body(&script, "ShouldRelaunch"),
        "Result := IsUpgrade and WasRunning;"
    );
    assert!(lines(&script).contains(
        &r#"Filename: "{app}\usagequotatray.exe"; Flags: nowait; Check: ShouldRelaunch"#
    ));
    assert_eq!(QUIT_EVENT, "UsageQuotaTray.Quit");
}

#[test]
fn the_uninstaller_closes_the_tray_after_the_confirmation_and_before_the_mutex_check() {
    let script = script();
    assert_eq!(
        body(&script, "CurUninstallStepChanged"),
        format!(
            "if CurUninstallStep = usAppMutexCheck then begin if CheckForMutexes('{RUNNING_MUTEX}') \
             then QuitRunningTray(ExpandConstant('{{app}}\\usagequotatray.exe')); end else if \
             CurUninstallStep = usUninstall then RemoveFromPc;"
        )
    );
}

#[test]
fn the_uninstaller_goes_on_only_when_the_removal_ran_and_exited_removed_or_partly() {
    let script = script();
    assert_eq!(
        constant(&script, "ExitRemoved"),
        cli::EXIT_REMOVED.to_string()
    );
    assert_eq!(
        constant(&script, "ExitPartly"),
        cli::EXIT_PARTLY.to_string()
    );
    assert_eq!(
        body(&script, "RemovalAccepted"),
        "Result := (ResultCode = ExitRemoved) or (ResultCode = ExitPartly);"
    );
    let remove = body(&script, "RemoveFromPc");
    let exec = remove
        .find(
            "if not Exec(Exe, Params, '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then \
             StopUninstall(",
        )
        .expect("an Exec failure stops the uninstall");
    let codes = remove
        .find("if not RemovalAccepted(ResultCode) then StopUninstall(RemovalFailure(ResultCode));")
        .expect("any other exit code stops the uninstall");
    assert!(exec < codes, "{remove}");
    let stop = body(&script, "StopUninstall");
    assert!(stop.ends_with("Abort;"), "{stop}");
    assert!(stop.contains("Nothing was removed."), "{stop}");
}

#[test]
fn the_installer_waits_on_the_mutex_every_tray_holds() {
    let script = script();
    assert!(
        lines(&script).contains(&format!("AppMutex={RUNNING_MUTEX}").as_str()),
        "AppMutex={RUNNING_MUTEX}"
    );
    assert!(script.contains(&format!("CheckForMutexes('{RUNNING_MUTEX}')")));
}

#[test]
fn the_start_menu_shortcut_carries_the_notification_identity() {
    let script = script();
    let shortcut = lines(&script)
        .into_iter()
        .find(|l| l.starts_with("Name: \"{autoprograms}\\"))
        .expect("a Start menu shortcut");
    assert!(
        shortcut.contains(r#"Filename: "{app}\usagequotatray.exe""#),
        "{shortcut}"
    );
    assert!(
        shortcut.contains(&format!("AppUserModelID: {}", inno_quoted(APP_ID))),
        "{shortcut}"
    );
}

#[test]
fn the_uninstaller_runs_the_headless_removal_and_stops_when_it_says_so() {
    let script = script();
    let remove = body(&script, "RemoveFromPc");
    assert!(
        remove.contains(&format!("Params := '{}';", cli::REMOVE_FROM_PC)),
        "{remove}"
    );
    assert!(
        remove.contains(&format!("Params := Params + ' ' + '{}';", cli::DELETE_DATA)),
        "{remove}"
    );
    assert!(script.contains(&format!("ExitNotUndone = {};", cli::EXIT_NOT_UNDONE)));
    assert!(body(&script, "RemovalFailure").contains("if ResultCode = ExitNotUndone then"));
}
