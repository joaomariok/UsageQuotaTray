# UsageQuotaTray

A small Windows 11 tray app that shows how much of your AI coding assistant's usage allowance you have consumed. One tray icon shows up to two Featured Meters as rings plus the Assistant's Badge; a popup lists every Meter with reset countdowns and pace, and toasts warn you at 75%, 90% and 100%.

The first Assistant is Claude Code. The core is Assistant-neutral, so other Assistants can be added as modules (see [docs/adding-an-assistant.md](docs/adding-an-assistant.md)).

It runs as one per-user process: no service, no admin rights, no installer, no .NET or WebView2. Built in Rust with Slint's software renderer. See [ADR-0001](docs/adr/0001-per-user-tray-process-not-windows-service.md) and [ADR-0002](docs/adr/0002-rust-and-slint-software-renderer.md). Vocabulary is in [GLOSSARY.md](GLOSSARY.md).

> **Not affiliated with Anthropic.** UsageQuotaTray is an independent project. "Claude" and "Anthropic" belong to their owners. The Claude Code Badge is a placeholder (a plain orange "C") until the Claude logo is added; this is still pending.

## Privacy and security

Read this before you run it. Everything below is what the code does today.

**What leaves your PC.** One thing: the Claude Code access token, as a bearer token, in a `GET` to `https://api.anthropic.com/api/oauth/usage`. Nothing else is sent anywhere. There is no telemetry and no update check. Every module can reach only the hosts it declares (for Claude Code: `api.anthropic.com`); the host client refuses any other host, non-HTTPS URLs and URLs with credentials in them, and does not follow redirects. This is a guard against mistakes, not a sandbox: modules run inside the same process.

**What it never does.** It never writes, refreshes or rotates Claude Code's credentials. Tokens are never logged, stored or printed; request headers are redacted when formatted, and every log line goes through a redaction filter.

**What the Claude Code module reads, and from where** (all read-only unless stated):

| What | Where | Used for |
| --- | --- | --- |
| Access token, expiry, plan fields (subscription type, rate-limit tier) | `.credentials.json` in each Claude config folder | The request, and plan detection |
| Account and organization ids, email, plan and billing fields, whether usage credits are enabled | The account file `.claude.json` (for the default folder, `%USERPROFILE%\.claude.json`) | Telling accounts apart, the Account name, plan detection |
| Whether `apiKeyHelper` or an `env` entry for `ANTHROPIC_API_KEY` exists | `settings.json` in each folder (key names only, values skipped) | A notice that Claude Code will bill an API key |
| Whether `ANTHROPIC_API_KEY` exists | This process's environment, and the user and machine `Environment` registry keys (existence only, the value is not read) | The same notice |

Config folders: `CLAUDE_CONFIG_DIR` if set, otherwise `%USERPROFILE%\.claude`, plus folders you add in Settings. WSL paths are only read if you add them by hand; reading one starts the distro.

**The statusline helper** (`uqt-statusline.exe`, optional) is run by Claude Code after each response. It reads the JSON Claude Code pipes to it and forwards only the 5-hour and 7-day used percentage and reset time to the running tray, over a named pipe that is local to the current user. Nothing from it goes over the network. It then runs your previous statusline command with the same input and prints its output (or a short `5h 24% · 7d 41%` line if you had none).

**Installing the helper** is a button in Settings. It asks first, writes a timestamped backup of Claude Code's `settings.json` next to it, keeps every other setting in order, records your previous `statusLine` command in `statusline.json` so it can be chained, and puts the helper in. Uninstall restores your original command and then deletes the backup. If you replaced the `statusLine` yourself in the meantime, it is detected and not overwritten. Nothing edits Claude Code's settings except this button.

**Local files**, all under `%LOCALAPPDATA%\UsageQuotaTray`:

- `settings.json`: your settings (a corrupt file is kept as `settings.json.bad`).
- `logs\usagequotatray-<date>.log`: daily logs, newest 7 kept, each capped at 5 MB, secrets redacted.
- `badges\<assistant id>.svg`: optional Badge overrides you create yourself.
- `statusline.json`: the statusline installation record.
- `notification.png`: the icon used for toasts.

**Windows registry** (current user only):

- `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\UsageQuotaTray`, only if you turn on "Start with Windows".
- `HKCU\Software\Classes\AppUserModelId\UsageQuotaTray`: the notification identity (display name and icon) so toasts show as UsageQuotaTray. Written at startup.

**Removing it.** Settings has "Remove UsageQuotaTray from this PC". After you confirm, it first uninstalls the statusline helper if it is installed, putting your original `statusLine` back in Claude Code's settings (the confirmation says so), then deletes both registry entries above and, if you choose, the whole `%LOCALAPPDATA%\UsageQuotaTray` folder, and quits. If the `statusLine` cannot be put back, nothing is removed and the app keeps running. Then delete the zip contents.

## Known limitations

- **Undocumented endpoint.** The usage endpoint is not a documented, supported API. It is the one Claude Code's own usage view uses. It can change or disappear without notice; the app then says the endpoint changed instead of showing wrong numbers.
- **Shared rate limit.** Its rate limit is shared with Claude Code itself. The app polls every 5 minutes while you are present and 15 minutes when idle, pauses while the PC is locked or asleep, backs off on errors and honors `Retry-After`. The shortest interval you can set is 2 minutes, with a warning in Settings for anything under 5 minutes. The statusline helper gives real-time numbers without extra requests.
- **Terms of use.** Using this endpoint with your own Claude Code login may not be covered by Anthropic's terms for the API or for consumer subscriptions. I have not verified that it is allowed. Decide for yourself; if in doubt, don't use the Claude Code module (switch Claude Code off in Settings).
- **Shared quotas.** On a Claude subscription, included usage is shared across Claude Code, Claude chat and Cowork, so your weekly Meter can move when you are not coding. The app says so, and shows the weekly split (the Breakdown) when the endpoint provides it.
- **Estimates.** Values are labeled **Exact** when the Assistant's maker reports them and **Estimate** when the app derives them from partial data. The Claude Code module currently reports only Exact values. Never read an Estimate as the maker's number.
- Alert memory is not kept across restarts, and some Windows integration (toast click, Explorer restart, light/dark taskbar) has not been verified on every setup.

## Build and test

Rust stable (MSVC) on Windows. The deep Slint paths can hit `MAX_PATH`, so use a short target directory.

```powershell
$env:CARGO_TARGET_DIR = "C:/t/uqt"
cargo build --release
```

The gate, run each separately; all four must pass:

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

Tests use fake transports and recorded fixtures; they never call a real host.

Development switches are compiled in only with the `dev-hooks` feature (`cargo build --features dev-hooks`); a normal build ignores every `UQT_*` variable. Debug builds always add the Fake Assistant. With the feature: `UQT_CLAUDE_CODE=0` leaves out the Claude Code module, `UQT_FAKE_ASSISTANT=1` adds the Fake Assistant, `UQT_FAKE_FAIL=offline|expired|not-logged-in|changed|rate-limited` makes it fail, `UQT_NOTIFICATIONS=off` skips the notification registration and toasts, and `UQT_READY_FILE`, `UQT_ICON_CHURN=1` and `UQT_SHOT=popup|settings` drive measurement and screenshot runs. For a safe manual run, use a `dev-hooks` build with `UQT_CLAUDE_CODE=0` and `UQT_NOTIFICATIONS=off`, and point `LOCALAPPDATA` at a throwaway folder. `tools/measure/measure.ps1` builds such an exe when given no `-Exe`.

## Packaging

Ship `usagequotatray.exe` and `uqt-statusline.exe` side by side in one folder (both in `target\release`). The statusline install button only appears when the helper sits next to the app.

## License and attribution

UsageQuotaTray is MIT licensed; see [LICENSE](LICENSE). The UI uses [Slint](https://slint.dev) under its royalty-free license, which requires this attribution badge on a public page:

[![Made with Slint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-whitebg.png)](https://slint.dev)
