# Per-user Inno Setup installer next to the portable zip

UsageQuotaTray ships two ways: a per-user installer built with Inno Setup 6 and the portable zip. The installer runs without admin rights (`PrivilegesRequired=lowest`, no all-users option), installs to `%LOCALAPPDATA%\Programs\UsageQuotaTray` by default, adds a Start menu shortcut and an Apps & features entry, and its uninstaller runs the app's own removal (`usagequotatray.exe --remove-from-pc`) so Claude Code's `statusLine` is put back before any file goes. We rejected MSIX: it needs a trusted signature to install, and packaged identity replaces the app's own AppUserModelID, which changes how toasts are attributed. We rejected WiX/MSI: per-user MSI installs are awkward (ICE rules, `ALLUSERS` and the per-user folder conventions) for no gain over Inno for a single-folder app. We rejected shipping only the zip: an installer gives a Start menu entry, an uninstall entry and upgrades in place, which most users expect.

## Consequences

- The installer's AppId and default folder are fixed for good. The Start-with-Windows value and Claude Code's `statusLine` point at the installed exe's path, so a new AppId would install a second copy beside the first, and a new default folder would leave both pointing at the old one.
- The installer writes the Run value in exactly the app's format; `crates/app/tests/installer.rs` checks the script against the app (Run value, `AppMutex`, the shortcut's AppUserModelID, the removal command).
- Every running tray holds the `UsageQuotaTray.Running` mutex so Setup and Uninstall can ask the user to close it.
- The installer is unsigned, so SmartScreen warns on first run. The zip keeps today's behaviour, including the in-app removal; an installed copy hands removal to its uninstaller.
