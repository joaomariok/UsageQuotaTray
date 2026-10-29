//! Claude Code's statusLine command when the statusline push is installed. Claude Code
//! pipes it JSON after every response; it forwards the rate limits to the running
//! tray (if any), then prints the user's previous statusline command's output.
//! A console program on purpose: it talks over stdin and stdout.

use std::io::{Read, Write};

use uqt_app::{app_data_dir, run_statusline_helper};
use uqt_assistant_anthropic::ConfigFolder;
use uqt_assistant_anthropic::statusline::chain::Shell;

/// Claude Code's input is a few KiB; anything past this is ignored.
const MAX_INPUT: u64 = 1024 * 1024;

fn main() {
    let mut input = Vec::new();
    let _ = std::io::stdin().take(MAX_INPUT).read_to_end(&mut input);
    let data = app_data_dir();
    let folder = ConfigFolder::discover_default();
    let out = run_statusline_helper(&input, data.as_deref(), folder.as_ref(), &Shell::from_env());
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(&out);
    let _ = stdout.flush();
}
