//! A `log` backend writing daily files, keeping the newest seven and capping each at
//! 5 MB. Every line is redacted with [`crate::redact::mask`] before it is written.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use log::{LevelFilter, Log, Metadata, Record};

use crate::redact::mask;
use crate::time::Clock;

const PREFIX: &str = "usagequotatray-";
const SUFFIX: &str = ".log";
pub const KEEP_FILES: usize = 7;
pub const CAP_BYTES: u64 = 5 * 1024 * 1024;

pub struct FileLogger {
    dir: PathBuf,
    clock: Arc<dyn Clock>,
    keep: usize,
    cap: u64,
    current: Mutex<Option<Current>>,
}

struct Current {
    date: String,
    file: File,
    written: u64,
    capped: bool,
}

impl FileLogger {
    pub fn new(dir: &Path, clock: Arc<dyn Clock>) -> Self {
        Self::with_limits(dir, clock, KEEP_FILES, CAP_BYTES)
    }

    pub fn with_limits(dir: &Path, clock: Arc<dyn Clock>, keep: usize, cap: u64) -> Self {
        Self {
            dir: dir.to_path_buf(),
            clock,
            keep,
            cap,
            current: Mutex::new(None),
        }
    }

    /// Installs this logger as the process-wide `log` backend at `level`.
    pub fn install(self, level: LevelFilter) -> Result<(), log::SetLoggerError> {
        log::set_boxed_logger(Box::new(self))?;
        log::set_max_level(level);
        Ok(())
    }

    fn open(&self, date: &str) -> std::io::Result<Current> {
        fs::create_dir_all(&self.dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(format!("{PREFIX}{date}{SUFFIX}")))?;
        let written = file.metadata().map_or(0, |m| m.len());
        self.prune();
        Ok(Current {
            date: date.to_string(),
            file,
            written,
            capped: written >= self.cap,
        })
    }

    /// Deletes our oldest daily files beyond `keep`. Other files are left alone.
    fn prune(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut ours: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(SUFFIX))
            })
            .collect();
        ours.sort();
        let excess = ours.len().saturating_sub(self.keep);
        for old in &ours[..excess] {
            let _ = fs::remove_file(old);
        }
    }
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        let c = self.clock.now().civil(0);
        let date = format!("{:04}-{:02}-{:02}", c.year, c.month, c.day);
        let line = mask(&format!(
            "{date} {:02}:{:02}:{:02} {} {}: {}\n",
            c.hour,
            c.minute,
            c.second,
            record.level(),
            record.target(),
            record.args()
        ));
        let mut guard = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().is_none_or(|cur| cur.date != date) {
            *guard = self.open(&date).ok();
        }
        let Some(cur) = guard.as_mut() else {
            return;
        };
        if cur.capped {
            return;
        }
        let bytes = line.as_bytes();
        if cur.written + bytes.len() as u64 > self.cap {
            let _ = cur
                .file
                .write_all(b"log size cap reached; further messages dropped today\n");
            cur.capped = true;
            return;
        }
        if cur.file.write_all(bytes).is_ok() {
            cur.written += bytes.len() as u64;
        }
    }

    fn flush(&self) {
        if let Some(cur) = self
            .current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            let _ = cur.file.flush();
        }
    }
}
