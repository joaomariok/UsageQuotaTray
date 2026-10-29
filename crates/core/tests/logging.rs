//! The rotating, redacting file logger, driven through the `log::Log` interface with a
//! fake clock and a temporary folder.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use log::{Level, Log, Record};
use uqt_core::logging::FileLogger;
use uqt_core::time::{FakeClock, Timestamp};

const DAY: i64 = 86_400;

fn info(logger: &FileLogger, message: &str) {
    logger.log(
        &Record::builder()
            .args(format_args!("{message}"))
            .level(Level::Info)
            .target("test")
            .build(),
    );
}

fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn writes_one_file_per_day_with_secrets_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new(Timestamp::from_utc(2026, 10, 29, 12, 0, 0));
    let logger = FileLogger::new(dir.path(), Arc::new(clock.clone()));

    info(
        &logger,
        "fetched with Bearer sk-ant-oat01-AbC_123-xyz for jane@example.com",
    );
    clock.advance(DAY);
    info(&logger, "next day");

    assert_eq!(
        files(dir.path()),
        vec![
            "usagequotatray-2026-10-29.log",
            "usagequotatray-2026-10-30.log"
        ]
    );
    let first = fs::read_to_string(dir.path().join("usagequotatray-2026-10-29.log")).unwrap();
    assert!(
        first.contains("2026-10-29 12:00:00 INFO test: fetched with Bearer <redacted> for <email>")
    );
    assert!(!first.contains("sk-ant"));
    assert!(!first.contains("jane@"));
}

#[test]
fn keeps_only_the_seven_newest_daily_files() {
    let dir = tempfile::tempdir().unwrap();
    // A file that is not ours must survive pruning.
    fs::write(dir.path().join("notes.txt"), "keep me").unwrap();
    let clock = FakeClock::new(Timestamp::from_utc(2026, 10, 1, 8, 0, 0));
    let logger = FileLogger::new(dir.path(), Arc::new(clock.clone()));
    for _ in 0..10 {
        info(&logger, "hello");
        clock.advance(DAY);
    }
    let names = files(dir.path());
    assert_eq!(names.len(), 8);
    assert_eq!(names[0], "notes.txt");
    assert_eq!(names[1], "usagequotatray-2026-10-04.log");
    assert_eq!(names[7], "usagequotatray-2026-10-10.log");
}

#[test]
fn stops_writing_a_day_at_the_size_cap() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new(Timestamp::from_utc(2026, 10, 29, 12, 0, 0));
    let logger = FileLogger::with_limits(dir.path(), Arc::new(clock.clone()), 7, 1024);
    let line = "x".repeat(100);
    for _ in 0..50 {
        info(&logger, &line);
    }
    let path = dir.path().join("usagequotatray-2026-10-29.log");
    let text = fs::read_to_string(&path).unwrap();
    assert!(fs::metadata(&path).unwrap().len() <= 1024 + 200);
    assert_eq!(text.matches("log size cap reached").count(), 1);

    // A new day starts a new file.
    clock.advance(DAY);
    info(&logger, "fresh");
    let next = fs::read_to_string(dir.path().join("usagequotatray-2026-10-30.log")).unwrap();
    assert!(next.contains("fresh"));
}
