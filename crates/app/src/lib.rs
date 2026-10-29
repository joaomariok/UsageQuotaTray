//! The composition root: the only crate that names Assistant modules.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use uqt_assistant_anthropic::statusline::chain::{self, Shell, chained_command};
use uqt_assistant_anthropic::statusline::installer::{Installer, STATE_FILE};
use uqt_assistant_anthropic::{ClaudeCodeAssistant, ConfigFolder, statusline};
use uqt_assistant_fake::{FakeAssistant, FakeControl, sources};
use uqt_core::contract::{Assistant, FetchError};
use uqt_core::push;

/// The statusline helper, shipped next to the app's exe.
pub const HELPER_EXE: &str = "uqt-statusline.exe";
/// A chained statusline command still running after this is stopped.
const CHAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// `%LOCALAPPDATA%\UsageQuotaTray`.
pub fn app_data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("UsageQuotaTray"))
}

/// The statusline installation for Claude Code's `folder`, offered only when the
/// helper ships in `exe_dir`.
pub fn statusline_installer(
    data_dir: &Path,
    exe_dir: &Path,
    folder: Option<ConfigFolder>,
) -> Option<Installer> {
    let helper = exe_dir.join(HELPER_EXE);
    if !helper.is_file() {
        return None;
    }
    Some(Installer::new(&folder?, data_dir, helper))
}

/// What the statusline helper does with one input from Claude Code, returning what
/// to print: push the rate limits to the tray of `data_dir` (if one is listening),
/// then print the chained command's output, or a short summary when nothing is
/// chained. Never waits for the tray.
pub fn run_statusline_helper(
    input: &[u8],
    data_dir: Option<&Path>,
    folder: Option<&ConfigFolder>,
    shell: &Shell,
) -> Vec<u8> {
    if let (Some(dir), Some(folder)) = (data_dir, folder)
        && let Some(message) = statusline::push_message(input, folder)
    {
        // No tray running is normal; nothing to report.
        let _ = push::send(&push::pipe_name(dir), &message);
    }
    match data_dir.and_then(|d| chained_command(&d.join(STATE_FILE))) {
        Some(command) => chain::run(shell, &command, input, CHAIN_TIMEOUT).unwrap_or_default(),
        None => {
            let mut line = statusline::summary(input).into_bytes();
            if !line.is_empty() {
                let _ = line.write_all(b"\n");
            }
            line
        }
    }
}

/// Which modules run, decided at startup.
#[derive(Clone, Debug, Default)]
pub struct ModuleOptions {
    /// On unless `UQT_CLAUDE_CODE=0`.
    pub claude_code: bool,
    /// On in debug builds, or with `UQT_FAKE_ASSISTANT=1`.
    pub fake_assistant: bool,
    /// Every Fake Source fails with this, to see failure states (`UQT_FAKE_FAIL`).
    pub fake_failure: Option<FetchError>,
}

impl ModuleOptions {
    pub fn from_env() -> Self {
        Self {
            claude_code: std::env::var("UQT_CLAUDE_CODE").map_or(true, |v| v != "0"),
            fake_assistant: cfg!(debug_assertions)
                || std::env::var("UQT_FAKE_ASSISTANT").is_ok_and(|v| v == "1"),
            fake_failure: std::env::var("UQT_FAKE_FAIL")
                .ok()
                .and_then(|v| fake_failure(&v)),
        }
    }
}

/// Every module compiled in, enabled or not. Register new Assistants here.
pub fn all_modules() -> Vec<Box<dyn Assistant>> {
    vec![
        Box::new(ClaudeCodeAssistant::from_env()),
        Box::new(FakeAssistant::new()),
    ]
}

/// The modules to run.
pub fn enabled_modules(options: &ModuleOptions) -> Vec<Box<dyn Assistant>> {
    enabled_modules_with(options, None)
}

/// The modules to run, Claude Code offering the statusline install button when an
/// installation is given.
pub fn enabled_modules_with(
    options: &ModuleOptions,
    statusline: Option<Installer>,
) -> Vec<Box<dyn Assistant>> {
    let mut modules: Vec<Box<dyn Assistant>> = Vec::new();
    if options.claude_code {
        let mut claude = ClaudeCodeAssistant::from_env();
        if let Some(installer) = statusline {
            claude = claude.with_statusline(installer);
        }
        modules.push(Box::new(claude));
    }
    if options.fake_assistant {
        let control = FakeControl::new();
        if let Some(error) = &options.fake_failure {
            control.fail(sources::API, Some(error.clone()));
            control.fail(sources::LOCAL, Some(error.clone()));
        }
        modules.push(Box::new(FakeAssistant::with_control(control)));
    }
    modules
}

/// Parses a `UQT_FAKE_FAIL` value.
pub fn fake_failure(name: &str) -> Option<FetchError> {
    Some(match name {
        "offline" => FetchError::Offline,
        "expired" => FetchError::Expired,
        "not-logged-in" => FetchError::NotLoggedIn,
        "changed" => FetchError::EndpointChanged,
        "rate-limited" => FetchError::RateLimited { retry_after: None },
        _ => return None,
    })
}
