//! Installs the statusline helper as Claude Code's `statusLine` command and removes it
//! again. Claude Code's settings file is changed only here, and only when asked:
//! a backup is written first, every other key is kept (in order), the user's previous
//! `statusLine` command is recorded so the helper can chain it, and uninstalling puts
//! the original back.
//!
//! The settings format (`statusLine: {type: "command", command, padding}`) and how the
//! command runs on Windows (through Git Bash when it is installed, else through
//! PowerShell; forward slashes in paths, since Git Bash eats backslashes) are from
//! https://code.claude.com/docs/en/statusline ("Windows configuration").

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use uqt_core::time::Timestamp;

use super::chain::Shell;

const KEY: &str = "statusLine";
/// The installation record's name in UsageQuotaTray's app-data folder.
pub const STATE_FILE: &str = "statusline.json";

/// The files one installation works on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installer {
    /// Claude Code's user settings (`<config folder>\settings.json`).
    pub settings_file: PathBuf,
    /// UsageQuotaTray's record of the installation, which the helper reads to find
    /// the command it chains.
    pub state_file: PathBuf,
    /// The helper executable.
    pub helper: PathBuf,
    /// The shell Claude Code runs the `statusLine` command through, which decides how a
    /// helper path with spaces is quoted.
    pub shell: Shell,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    NotInstalled,
    Installed {
        /// The user's previous statusline command, run by the helper.
        chained: Option<String>,
    },
    /// Installed once, but the `statusLine` setting has been changed since.
    Replaced,
}

/// What an install did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub backup: Option<PathBuf>,
    pub chained: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Uninstalled {
    /// The original `statusLine` (or the whole original file) is back.
    Restored,
    /// Someone set another `statusLine` since; it was left as it is.
    LeftAlone,
}

#[derive(Debug)]
pub enum InstallError {
    /// The settings file is not a JSON object; it was not changed.
    NotJson(String),
    /// The helper's path has characters a shell would interpret even when quoted.
    UnsafeHelperPath(PathBuf),
    Io(std::io::Error),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotJson(e) => write!(
                f,
                "Claude Code's settings file is not valid JSON ({e}); nothing was changed"
            ),
            Self::UnsafeHelperPath(p) => write!(
                f,
                "the helper's folder {} has quotes or other characters a shell would \
                 interpret; move UsageQuotaTray to a folder without them",
                p.display()
            ),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<std::io::Error> for InstallError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// What an installation records next to UsageQuotaTray's settings.
struct Record {
    command: String,
    /// The `statusLine` value before installing (`Null` when there was none).
    previous: Value,
    had_file: bool,
    backup: Option<PathBuf>,
    /// The settings file text right after installing.
    written: String,
}

impl Record {
    fn to_json(&self) -> Value {
        json!({
            "command": self.command,
            "previous": self.previous,
            "had_file": self.had_file,
            "backup": self.backup.as_ref().map(|p| p.display().to_string()),
            "written": self.written,
        })
    }

    fn read(path: &Path) -> Option<Self> {
        let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        Some(Self {
            command: v["command"].as_str()?.to_string(),
            previous: v["previous"].clone(),
            had_file: v["had_file"].as_bool()?,
            backup: v["backup"].as_str().map(PathBuf::from),
            written: v["written"].as_str()?.to_string(),
        })
    }

    fn chained(&self) -> Option<String> {
        chainable(&self.previous, &self.command)
    }
}

/// The previous value's command, unless it is not a command or is the helper itself.
fn chainable(previous: &Value, helper_command: &str) -> Option<String> {
    if previous["type"].as_str() != Some("command") {
        return None;
    }
    let command = previous["command"].as_str()?.trim();
    (!command.is_empty() && command != helper_command).then(|| command.to_string())
}

/// The command the helper chains, from an installation's record.
pub(crate) fn recorded_chain(state_file: &Path) -> Option<String> {
    Record::read(state_file)?.chained()
}

impl Installer {
    /// The installation for Claude Code's settings in `folder`, recorded in
    /// UsageQuotaTray's `data_dir`.
    pub fn new(folder: &crate::ConfigFolder, data_dir: &Path, helper: PathBuf) -> Self {
        Self {
            settings_file: folder.dir.join("settings.json"),
            state_file: data_dir.join(STATE_FILE),
            helper,
            shell: Shell::from_env(),
        }
    }

    /// The statusline command set now, if any and if it is not the helper. Read only.
    pub fn current_command(&self) -> Option<String> {
        let (settings, _) = read_settings(&self.settings_file).ok()??;
        if self.is_helper(&settings[KEY]) {
            return None;
        }
        chainable(&settings[KEY], &self.helper_command().ok()?)
    }

    /// The `statusLine` command: the helper's path with forward slashes, which both
    /// Git Bash and PowerShell run as written. A path with spaces or other characters
    /// a shell splits on is quoted for [`Installer::shell`]: in single quotes for Git
    /// Bash, and after PowerShell's call operator `&` (a quoted string alone is only a
    /// value there, not a command).
    pub fn helper_command(&self) -> Result<String, InstallError> {
        let forms = self.helper_forms()?;
        Ok(if self.shell.is_bash() {
            forms.bash
        } else {
            forms.powershell
        })
    }

    /// The helper's command as each shell runs it.
    fn helper_forms(&self) -> Result<HelperForms, InstallError> {
        let text = self.helper.display().to_string().replace('\\', "/");
        let plain = |c: char| c.is_ascii_alphanumeric() || "/:._-~".contains(c);
        if !text.is_empty() && text.chars().all(plain) {
            return Ok(HelperForms {
                bash: text.clone(),
                powershell: text,
            });
        }
        // Inside single quotes both shells take every character literally except the
        // quote itself (PowerShell also ends a string at its typographic quotes).
        let quotable =
            |c: char| !c.is_control() && !"'\u{2018}\u{2019}\u{201a}\u{201b}".contains(c);
        if text.is_empty() || !text.chars().all(quotable) {
            return Err(InstallError::UnsafeHelperPath(self.helper.clone()));
        }
        Ok(HelperForms {
            bash: format!("'{text}'"),
            powershell: format!("& '{text}'"),
        })
    }

    /// Whether `line` runs the helper, written for either shell (Git Bash may have
    /// been installed or removed since).
    fn is_helper(&self, line: &Value) -> bool {
        let Ok(forms) = self.helper_forms() else {
            return false;
        };
        let command = line["command"].as_str().map(str::trim);
        command == Some(forms.bash.as_str()) || command == Some(forms.powershell.as_str())
    }

    /// The recorded `statusLine` command when the installation record in the app-data
    /// folder is another copy's helper (a zip copy beside the installed one, say): that
    /// copy's helper still reads the record, and this copy cannot undo it. Read only.
    pub fn record_of_another_helper(&self) -> Option<String> {
        let record = Record::read(&self.state_file)?;
        let command = json!({ "command": record.command });
        (!self.is_helper(&command)).then_some(record.command)
    }

    pub fn status(&self) -> Status {
        if self.helper_forms().is_err() {
            return Status::NotInstalled;
        }
        let current = read_settings(&self.settings_file).ok().flatten();
        let ours = current
            .as_ref()
            .is_some_and(|(v, _)| self.is_helper(&v[KEY]));
        match (Record::read(&self.state_file), ours) {
            (Some(record), true) => Status::Installed {
                chained: record.chained(),
            },
            (None, true) => Status::Installed { chained: None },
            (Some(_), false) => Status::Replaced,
            (None, false) => Status::NotInstalled,
        }
    }

    /// Makes the helper Claude Code's statusline command. Call only after the user
    /// agreed. Installing again while installed changes nothing.
    pub fn install(&self, now: Timestamp) -> Result<Installed, InstallError> {
        let command = self.helper_command()?;
        let current = read_settings(&self.settings_file)?;
        if let Some((v, _)) = &current
            && self.is_helper(&v[KEY])
        {
            let record = Record::read(&self.state_file);
            return Ok(Installed {
                backup: record.as_ref().and_then(|r| r.backup.clone()),
                chained: record.and_then(|r| r.chained()),
            });
        }
        let (mut settings, original) = current.unwrap_or_else(|| (json!({}), String::new()));
        let had_file = self.settings_file.exists();
        let backup = if had_file {
            let path = backup_path(&self.settings_file, now);
            std::fs::copy(&self.settings_file, &path)?;
            Some(path)
        } else {
            None
        };

        let previous = settings.get(KEY).cloned().unwrap_or(Value::Null);
        let mut line = match &previous {
            Value::Object(o) => o.clone(),
            _ => Map::new(),
        };
        line.insert("type".into(), json!("command"));
        line.insert("command".into(), json!(command));
        settings
            .as_object_mut()
            .expect("checked to be an object")
            .insert(KEY.into(), Value::Object(line));
        let mut written = serde_json::to_string_pretty(&settings).map_err(std::io::Error::other)?;
        if original.ends_with('\n') || !had_file {
            written.push('\n');
        }

        let record = Record {
            command,
            previous,
            had_file,
            backup: backup.clone(),
            written: written.clone(),
        };
        // The record first: once Claude Code sees the new command, the helper must
        // already find the command it chains.
        write_atomic(
            &self.state_file,
            serde_json::to_string_pretty(&record.to_json())
                .map_err(std::io::Error::other)?
                .as_bytes(),
        )?;
        write_atomic(&self.settings_file, written.as_bytes())?;
        Ok(Installed {
            backup,
            chained: record.chained(),
        })
    }

    /// Puts the original `statusLine` back: the original file when nothing else
    /// changed since installing, else only the `statusLine` key. A `statusLine` that
    /// is no longer the helper is left alone.
    pub fn uninstall(&self) -> Result<Uninstalled, InstallError> {
        self.helper_forms()?;
        let record = Record::read(&self.state_file);
        let current = read_settings(&self.settings_file)?;
        let outcome = match current {
            Some((mut settings, text)) if self.is_helper(&settings[KEY]) => {
                let original = record.as_ref().and_then(|r| {
                    if text != r.written {
                        return None;
                    }
                    if !r.had_file {
                        return Some(None);
                    }
                    r.backup
                        .as_ref()
                        .and_then(|b| std::fs::read(b).ok())
                        .map(Some)
                });
                match original {
                    Some(Some(bytes)) => write_atomic(&self.settings_file, &bytes)?,
                    Some(None) => std::fs::remove_file(&self.settings_file)?,
                    None => {
                        let map = settings.as_object_mut().expect("checked to be an object");
                        match record.as_ref().map(|r| &r.previous) {
                            Some(previous) if !previous.is_null() => {
                                map.insert(KEY.into(), previous.clone());
                            }
                            _ => {
                                map.shift_remove(KEY);
                            }
                        }
                        let mut out = serde_json::to_string_pretty(&settings)
                            .map_err(std::io::Error::other)?;
                        if text.ends_with('\n') {
                            out.push('\n');
                        }
                        write_atomic(&self.settings_file, out.as_bytes())?;
                    }
                }
                Uninstalled::Restored
            }
            _ => Uninstalled::LeftAlone,
        };
        // The backup was only needed until the original was back. When the file was
        // left alone it may still be the user's only copy of their setting, so it stays.
        if outcome == Uninstalled::Restored
            && let Some(backup) = record.as_ref().and_then(|r| r.backup.as_ref())
        {
            remove_if_present(backup)?;
        }
        remove_if_present(&self.state_file)?;
        Ok(outcome)
    }
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The helper's command as Git Bash and as PowerShell run it.
struct HelperForms {
    bash: String,
    powershell: String,
}

/// The settings as JSON plus their text, `None` when the file does not exist.
fn read_settings(path: &Path) -> Result<Option<(Value, String)>, InstallError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let value: Value =
        serde_json::from_str(&text).map_err(|e| InstallError::NotJson(e.to_string()))?;
    if !value.is_object() {
        return Err(InstallError::NotJson("not an object".into()));
    }
    Ok(Some((value, text)))
}

/// `settings.json.uqt-backup-<unix seconds>` next to the file, never overwriting one.
fn backup_path(settings: &Path, now: Timestamp) -> PathBuf {
    let name = settings.file_name().map_or_else(
        || "settings.json".into(),
        |n| n.to_string_lossy().into_owned(),
    );
    let mut n = 0;
    loop {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!("-{n}")
        };
        let path = settings.with_file_name(format!("{name}.uqt-backup-{}{suffix}", now.0));
        if !path.exists() {
            return path;
        }
        n += 1;
    }
}

/// Writes a sibling temporary file, then renames it over `path`.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("uqt-tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}
