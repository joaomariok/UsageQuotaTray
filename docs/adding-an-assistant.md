# Adding an Assistant

An Assistant is one new crate plus one line in the app crate. Core and UI do not change. Terms (Assistant, Account, Source, Meter, Reading, Window, Breakdown, Exact, Estimate, Badge) are defined in [GLOSSARY.md](../GLOSSARY.md).

The template is [crates/assistant-template](../crates/assistant-template). It is a workspace member, so it always compiles and its tests run in the gate, but nothing registers it. (It sits in the workspace rather than under `templates/` because a crate outside the workspace would not be compiled or tested, and would rot. Nothing depends on it, so it needs no `deny.toml` entry.)

## 1. Copy the template

1. Copy `crates/assistant-template` to `crates/assistant-<name>`.
2. In its `Cargo.toml`, set `name = "uqt-assistant-<name>"`.
3. Add the new path to `members` and to `[workspace.dependencies]` in the root `Cargo.toml`.
4. Keep `clippy.toml`. It bans `TcpStream`, `TcpListener` and `UdpSocket` so a module cannot open sockets itself.
5. Depend only on `uqt-core`, `log` and small parsing crates. Never add an HTTP crate: `deny.toml` only lets `uqt-core` depend on `ureq`.

## 2. Implement the module

Implement `uqt_core::contract::Assistant`:

- `id`: stable, lowercase. It is also the Badge override file name and the settings key, so never change it later.
- `display_name`, `usage_page`.
- `allowed_hosts`: every host any of your Sources may call. The `HostClient` refuses anything else, plain `http`, URLs with credentials, and does not follow redirects. Declare the fewest hosts you can.
- `badge`: an SVG plus one or two fallback letters and a `#RRGGBB` color. Rules: square `viewBox`, filled paths only (`title` and `desc` are allowed), no text, strokes, gradients, filters, images or external references. Paths without a fill are drawn in `color`, so a monochrome logo can ship unaltered. The app test validates every registered Badge.
- `suggested_featured`: up to two Meter keys featured until the user chooses.
- `discover_accounts`: one `AccountInfo` per login. The id must be the maker's identity for the login (not the folder it was found in) and must be the same on every call. A new identity is a new Account.
- `sources`: the Sources for an Account. Called once per new Account.
- Optional: `settings_fields` and `configure` for module settings (rendered by the settings window), `run_action` for buttons.

## 3. Implement a Source

Implement `uqt_core::contract::Source`: `id`, `display_name` (shown next to each Reading), `exactness`, `priority` (lower fetches first), `intervals` (active and idle poll intervals; the user can override), and `fetch`.

`fetch(&FetchContext)` gets the time and the `HostClient`. It is the only way to the network. Return a `FetchOutput { values, notices }` or a typed `FetchError`:

| Situation | Error |
| --- | --- |
| Assistant not installed | `NotInstalled` |
| Never logged in | `NotLoggedIn` |
| Login or token expired (HTTP 401) | `Expired` |
| No connection | `Offline` |
| HTTP 429 | `RateLimited { retry_after }` |
| Response is not the shape you expect | `EndpointChanged` |
| Anything else | `Other(text)` |

Override `failure_hint` to say what the user can do ("run X once"). Read third-party credentials read-only and never log, store or print them.

Push Sources (data that the Assistant sends to you) use `push_only` and `accept_push` with the Core push channel; the Claude Code statusline is the example.

## 4. Map data to Meters

Each `MeterValue` has a stable `key`, a `label`, `used`, an optional `limit`, a `unit` (`Percent` with limit 100, `Currency(code)`, or `Count(name)`), an optional `window`, an optional `breakdown`, and `unrecognized`.

- **Window:** set `end` (the reset time). Add `length` from your own knowledge when the service does not give a start; pace needs it. A Meter with no Window gets no pace.
- **Breakdown:** when the service splits one Meter by surface or model, fill `Breakdown { parts: [{label, percent}] }` in the order given.
- **Unrecognized fields:** a Meter you cannot identify but that has a usage value must not be dropped. Emit it with `unrecognized: true`, the raw key as `label`, and a key such as `unrecognized:<raw key>`. It shows in a collapsed group, is never featured by default, and still alerts.
- Ignore `null` entries. If the whole response is not the expected shape, return `EndpointChanged` rather than guessing.
- Use `Exactness::Exact` only for values the maker reports. Values you compute from partial data are `Estimate`.

## 5. Fixtures and engine-seam tests

Test through the real `Engine`, as `crates/assistant-template/tests/engine.rs` does:

1. Put a recorded (or hand-written, same shape) response in `tests/fixtures/`. Scrub identifiers and tokens before committing.
2. Build an `Engine` with your module, a `FakeClock` and a `RecordingTransport` (`uqt_core::testing`). Queue responses with `push_response`.
3. Call `engine.tick()`, then assert on `engine.state()`: the Meters, Breakdown, `unrecognized`, and the requested URL.
4. Cover the failures: changed shape, 401, 429, offline.

Tests never use a real host, the real home folder, `%LOCALAPPDATA%` or the real registry. Use temp dirs and fakes.

## 6. Register it

All in the app and workspace config; no changes to `crates/core` or `crates/ui`:

1. `crates/app/Cargo.toml`: add `uqt-assistant-<name>.workspace = true`.
2. `crates/app/src/lib.rs`: add the module to `all_modules()` and to `enabled_modules_with()`, behind a `ModuleOptions` flag if it should be switchable.
3. `deny.toml`, under `[bans].deny`: add `{ crate = "uqt-assistant-<name>", wrappers = ["uqt-app"] }` so only the app crate can depend on it.
4. Confirm the app test that validates every Badge passes.

## 7. Run the gate

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

Slint's build paths are long. If the repo sits in a deeply nested folder and the build fails on `MAX_PATH`, set `CARGO_TARGET_DIR` to a short path.
