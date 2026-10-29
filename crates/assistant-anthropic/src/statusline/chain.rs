//! Runs the user's previous statusline command for the helper, the way Claude Code
//! runs statusline commands on Windows: through Git Bash when it is installed, else
//! through PowerShell (https://code.claude.com/docs/en/statusline, "Windows
//! configuration"). Git Bash is found like Claude Code does: `CLAUDE_CODE_GIT_BASH_PATH`
//! (https://code.claude.com/docs/en/setup), else next to `git.exe` on the PATH.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The command the helper chains, recorded at install time; `None` when there is none
/// or the helper is not installed.
pub fn chained_command(state_file: &Path) -> Option<String> {
    super::installer::recorded_chain(state_file)
}

/// A shell that runs one command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shell {
    pub program: PathBuf,
    /// Arguments before the command line.
    pub args: Vec<String>,
}

impl Shell {
    pub fn bash(path: PathBuf) -> Self {
        Self {
            program: path,
            args: vec!["-c".into()],
        }
    }

    /// Whether this is Git Bash (else PowerShell).
    pub fn is_bash(&self) -> bool {
        self.args.first().is_some_and(|a| a == "-c")
    }

    pub fn powershell() -> Self {
        Self {
            program: PathBuf::from("powershell.exe"),
            args: vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
            ],
        }
    }

    /// Git Bash from `git_bash_path` (`CLAUDE_CODE_GIT_BASH_PATH`) or from a
    /// `git.exe` on `path` (Git for Windows keeps `bash.exe` in `bin` beside `cmd`);
    /// PowerShell otherwise.
    pub fn resolve(
        git_bash_path: Option<OsString>,
        path: Option<OsString>,
        exists: impl Fn(&Path) -> bool,
    ) -> Self {
        if let Some(p) = git_bash_path.filter(|p| !p.is_empty()).map(PathBuf::from)
            && exists(&p)
        {
            return Self::bash(p);
        }
        for dir in path.iter().flat_map(std::env::split_paths) {
            if !exists(&dir.join("git.exe")) {
                continue;
            }
            // `Git\cmd`, `Git\bin` or `Git\mingw64\bin`.
            let candidates = [
                Some(dir.join("bash.exe")),
                dir.parent().map(|g| g.join("bin").join("bash.exe")),
                dir.parent()
                    .and_then(Path::parent)
                    .map(|g| g.join("bin").join("bash.exe")),
            ];
            if let Some(bash) = candidates.into_iter().flatten().find(|c| exists(c)) {
                return Self::bash(bash);
            }
        }
        Self::powershell()
    }

    /// [`Shell::resolve`] for this process's environment.
    pub fn from_env() -> Self {
        Self::resolve(
            std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH"),
            std::env::var_os("PATH"),
            Path::is_file,
        )
    }
}

/// Runs `command` with `input` on stdin and returns what it printed, or `None` when it
/// could not start. A command still running after `timeout` is killed and whatever
/// it printed so far is returned.
pub fn run(shell: &Shell, command: &str, input: &[u8], timeout: Duration) -> Option<Vec<u8>> {
    let mut child = Command::new(&shell.program)
        .args(&shell.args)
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    let _ = writer.join();
    reader.join().ok()
}
