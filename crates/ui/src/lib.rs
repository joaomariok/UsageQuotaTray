//! UsageQuotaTray UI. Depends on core only, never on an Assistant crate.
//!
//! [`present`], [`badge`] and [`icon`] are pure and tested; [`lifecycle`] is tested over
//! its registry seam; [`shell`], [`settings_window`], [`notify`], [`platform`] and
//! [`session`] are the Win32 and Slint glue.

pub mod badge;
pub mod icon;
pub mod lifecycle;
pub mod nav;
mod notify;
mod platform;
pub mod present;
mod session;
mod settings_window;
mod shell;

pub use shell::{ShellOptions, run};

slint::include_modules!();
