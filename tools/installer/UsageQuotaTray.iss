; UsageQuotaTray per-user installer (Inno Setup 6). Build with tools/installer/build.ps1,
; which passes AppVersion (from crates/app/Cargo.toml) and BinDir (the release build).
; crates/app/tests/installer.rs checks this script against the app: the Run value, the
; AppMutex, the shortcut's AppUserModelID, the --quit before upgrading and uninstalling,
; and the uninstaller's removal command and the exit codes it accepts.

[Setup]
; The AppId names the Apps & features entry and lets upgrades find the installed copy.
; Generated once; never change it, or upgrades install a second copy beside the first.
; lifecycle::UNINSTALL_KEY and UninstallKey below name the entry Inno Setup writes for it.
AppId={{9C02A488-5E56-4016-835D-BE4F735473FD}
AppName=UsageQuotaTray
AppVersion={#AppVersion}
UninstallDisplayName=UsageQuotaTray
UninstallDisplayIcon={app}\usagequotatray.exe
; Per user, no elevation, no all-users option (ADR-0003).
PrivilegesRequired=lowest
; Fixed for good: the Run value and Claude Code's statusLine point at the exe here.
DefaultDirName={localappdata}\Programs\UsageQuotaTray
DisableProgramGroupPage=yes
DisableReadyPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Every running tray holds this mutex (lifecycle::RUNNING_MUTEX); Setup and Uninstall
; first ask it to exit (--quit), then ask the user to close it if it is still there.
AppMutex=UsageQuotaTray.Running
CloseApplications=yes
; After the first install the app owns Start with Windows; the tasks are offered on a
; fresh install only (Check: IsFreshInstall), never restored from the last install.
UsePreviousTasks=no
OutputBaseFilename=UsageQuotaTray-Setup-{#AppVersion}

[Tasks]
Name: "startwithwindows"; Description: "Start with Windows"; Check: IsFreshInstall
Name: "launchnow"; Description: "Launch UsageQuotaTray now"; Check: IsFreshInstall

[Files]
Source: "{#BinDir}\usagequotatray.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\uqt-statusline.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; The app's AppUserModelID (lifecycle::APP_ID), so toasts and the shortcut match.
Name: "{autoprograms}\UsageQuotaTray"; Filename: "{app}\usagequotatray.exe"; WorkingDir: "{app}"; AppUserModelID: "UsageQuotaTray"

[Registry]
; Exactly what the app's Start with Windows toggle writes (lifecycle::Autostart: the
; quoted exe path), so the app shows the toggle on and can turn it off. Written on a
; fresh install when the task is chosen; on an upgrade only when the installed copy's
; value was there, to point it at this folder.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "UsageQuotaTray"; ValueData: """{app}\usagequotatray.exe"""; Flags: uninsdeletevalue; Check: ShouldWriteRunValue

[Run]
Filename: "{app}\usagequotatray.exe"; Flags: nowait; Tasks: launchnow
Filename: "{app}\usagequotatray.exe"; Flags: nowait; Check: ShouldRelaunch

[Code]
const
  // The Apps & features entry Inno Setup writes for the AppId above.
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{9C02A488-5E56-4016-835D-BE4F735473FD}_is1';
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  RunValueName = 'UsageQuotaTray';
  // usagequotatray.exe --remove-from-pc exit codes (crates/app/src/cli.rs). Only these
  // two let the uninstall go on; any other code, a panic's included, stops it.
  ExitRemoved = 0;
  ExitPartly = 2;
  // Claude Code's statusLine could not be put back, and nothing was removed.
  ExitNotUndone = 1;

var
  IsUpgrade: Boolean;
  WasRunning: Boolean;
  RunValueWasOurs: Boolean;

// The installed copy's exe, from its Apps entry; '' when not installed.
function InstalledExe: String;
var
  Dir: String;
begin
  Result := '';
  if RegQueryStringValue(HKCU, UninstallKey, 'InstallLocation', Dir) and (Dir <> '') then
    Result := AddBackslash(Dir) + 'usagequotatray.exe';
end;

// Asks every tray in this session to exit and waits for it (up to 10 s).
procedure QuitRunningTray(const Exe: String);
var
  ResultCode: Integer;
begin
  if (Exe = '') or not FileExists(Exe) then
    Exit;
  if Exec(Exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
    Log(Format('%s --quit exited with %d', [Exe, ResultCode]))
  else
    Log('Could not run ' + Exe + ' --quit: ' + SysErrorMessage(ResultCode));
end;

function InitializeSetup: Boolean;
var
  OldExe, RunData: String;
begin
  // Runs before Setup's AppMutex check, which waits until the tray is closed.
  IsUpgrade := RegKeyExists(HKCU, UninstallKey);
  OldExe := InstalledExe;
  RunValueWasOurs := IsUpgrade and (OldExe <> '') and
    RegQueryStringValue(HKCU, RunKey, RunValueName, RunData) and
    SameText(RunData, '"' + OldExe + '"');
  WasRunning := CheckForMutexes('UsageQuotaTray.Running');
  if WasRunning and IsUpgrade then
    QuitRunningTray(OldExe);
  Result := True;
end;

function IsFreshInstall: Boolean;
begin
  Result := not IsUpgrade;
end;

// A fresh install writes the Run value when asked; an upgrade rewrites it only when it
// started the installed copy, and leaves it absent when the user turned it off.
function ShouldWriteRunValue: Boolean;
begin
  if IsUpgrade then
    Result := RunValueWasOurs
  else
    Result := WizardIsTaskSelected('startwithwindows');
end;

// An upgrade starts the tray again only when it was running before.
function ShouldRelaunch: Boolean;
begin
  Result := IsUpgrade and WasRunning;
end;

// The Ready page is off, so the Tasks page is the last before installing; with no page
// before it (an upgrade) Setup shows the Ready page anyway.
procedure CurPageChanged(CurPageID: Integer);
begin
  if (CurPageID = wpSelectTasks) or (CurPageID = wpReady) then
    WizardForm.NextButton.Caption := SetupMessage(msgButtonInstall)
  else if CurPageID = wpFinished then
    WizardForm.NextButton.Caption := SetupMessage(msgButtonFinish)
  else
    WizardForm.NextButton.Caption := SetupMessage(msgButtonNext);
end;

// Stops the uninstall with a message; the uninstaller has removed nothing yet.
procedure StopUninstall(const Why: String);
begin
  Log('Uninstall stopped: ' + Why);
  SuppressibleMsgBox(Why + #13#10#13#10 + 'Nothing was removed.', mbError, MB_OK, IDOK);
  Abort;
end;

// The only --remove-from-pc exit codes that let the uninstall go on.
function RemovalAccepted(ResultCode: Integer): Boolean;
begin
  Result := (ResultCode = ExitRemoved) or (ResultCode = ExitPartly);
end;

function RemovalFailure(ResultCode: Integer): String;
begin
  if ResultCode = ExitNotUndone then
    Result := 'UsageQuotaTray could not put Claude Code''s original statusLine setting ' +
      'back. Make sure Claude Code''s settings.json can be written, then uninstall again.'
  else
    Result := Format('UsageQuotaTray''s removal stopped unexpectedly (exit code %d), so ' +
      'Claude Code''s statusLine setting may not be back. Make sure Claude Code''s ' +
      'settings.json can be written, then uninstall again.', [ResultCode]);
end;

// After the user confirmed and the tray is closed, before any file is removed: the
// app's own removal (Claude Code's statusLine back first, then the Run value and the
// notification registration, and the data folder if asked). Fails closed: unless the
// removal ran and exited with ExitRemoved or ExitPartly, stop and remove nothing.
procedure RemoveFromPc;
var
  Exe, Params: String;
  ResultCode: Integer;
begin
  Exe := ExpandConstant('{app}\usagequotatray.exe');
  Params := '--remove-from-pc';
  if SuppressibleMsgBox('Also delete your UsageQuotaTray settings and logs?' + #13#10#13#10 +
      ExpandConstant('{localappdata}\UsageQuotaTray'),
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then
    Params := Params + ' ' + '--delete-data';
  if not Exec(Exe, Params, '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
    StopUninstall('Could not run ' + Exe + ' ' + Params + ' (' +
      SysErrorMessage(ResultCode) + '), so Claude Code''s statusLine setting may still ' +
      'run the statusline helper. Reinstall UsageQuotaTray, then uninstall again.');
  Log(Format('%s %s exited with %d', [Exe, Params, ResultCode]));
  if not RemovalAccepted(ResultCode) then
    StopUninstall(RemovalFailure(ResultCode));
  if ResultCode = ExitPartly then
    SuppressibleMsgBox('Claude Code''s statusLine setting is back, but not all of ' +
      'UsageQuotaTray''s own entries could be removed (for example, its statusline.json ' +
      'stays while another UsageQuotaTray copy''s statusline helper uses it). The program ' +
      'files are removed now.', mbInformation, MB_OK, IDOK);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  // usAppMutexCheck: after the user confirmed, before the AppMutex check
  // (Setup.Uninstall.pas), so cancelling the confirmation leaves the tray running.
  if CurUninstallStep = usAppMutexCheck then begin
    if CheckForMutexes('UsageQuotaTray.Running') then
      QuitRunningTray(ExpandConstant('{app}\usagequotatray.exe'));
  end
  else if CurUninstallStep = usUninstall then
    RemoveFromPc;
end;
