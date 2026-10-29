# Rust with Slint's software renderer, not .NET

UsageQuotaTray is written in Rust with its UI in Slint (fluent style, software renderer), although the original brief assumed C#/.NET. An always-on tray app has to stay light, and side-by-side prototypes implementing the same tray icon, Fluent-styled popup and settings window measured Slint at 4.7 MiB private memory (5.0 MiB with screen-reader support). The alternatives measured: .NET WinForms 11 MiB, plus the .NET runtime or a 117 MiB self-contained bundle; WPF with its Fluent theme 59 MiB; WinUI 3 80–85 MiB; raw Win32 with Direct2D 6.5 MiB, but with every control hand-written; Tauri with WebView2 151 MiB across 7 processes. The prototypes and raw numbers are on branch `chore/rust_ui_prototype` (`tools/rust-ui-prototype/results/RESULTS.md`).

## Consequences

- The software renderer is deliberate: Slint's GPU renderer cost about 60 MiB for the same UI. Do not switch renderers without measuring.
- Slint is used under its royalty-free license while our code stays MIT. Attribution is the Slint badge in the README, not the in-app `AboutSlint` widget.
- Slint's standard widgets fall short of Windows 11 Settings (top tabs, plain group boxes), so the UI crate carries a few components of its own, such as a left navigation list and a card.
- Slint's build scripts exceed the Windows path limit in deeply nested folders; build there with a short `CARGO_TARGET_DIR`.
