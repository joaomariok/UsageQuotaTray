//! UsageQuotaTray UI. Depends on core only, never on an Assistant crate.
//!
//! [`present`], [`badge`] and [`icon`] are pure and tested; [`shell`], [`platform`] and
//! [`session`] are the Win32 and Slint glue.

pub mod badge;
pub mod icon;
mod platform;
pub mod present;
mod session;
mod shell;

pub use shell::{ShellOptions, run};

slint::include_modules!();
