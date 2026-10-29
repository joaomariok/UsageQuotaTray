; UsageQuotaTray per-user installer (Inno Setup 6). Build with tools/installer/build.ps1,
; which passes AppVersion (from crates/app/Cargo.toml) and BinDir (the release build).
; crates/app/tests/installer.rs checks this script against the app: the Run value, the
; AppMutex, the shortcut's AppUserModelID and the uninstaller's removal command.

[Setup]
; The AppId names the Apps & features entry and lets upgrades find the installed copy.
; Generated once; never change it, or upgrades install a second copy beside the first.
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
; ask the user to close it first.
AppMutex=UsageQuotaTray.Running
CloseApplications=yes
OutputBaseFilename=UsageQuotaTray-Setup-{#AppVersion}

[Tasks]
Name: "startwithwindows"; Description: "Start with Windows"
Name: "launchnow"; Description: "Launch UsageQuotaTray now"

[Files]
Source: "{#BinDir}\usagequotatray.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\uqt-statusline.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; The app's AppUserModelID (lifecycle::APP_ID), so toasts and the shortcut match.
Name: "{autoprograms}\UsageQuotaTray"; Filename: "{app}\usagequotatray.exe"; WorkingDir: "{app}"; AppUserModelID: "UsageQuotaTray"

[Registry]
; Exactly what the app's Start with Windows toggle writes (lifecycle::Autostart: the
; quoted exe path), so the app shows the toggle on and can turn it off.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "UsageQuotaTray"; ValueData: """{app}\usagequotatray.exe"""; Flags: uninsdeletevalue; Tasks: startwithwindows

[Run]
Filename: "{app}\usagequotatray.exe"; Flags: nowait; Tasks: launchnow
Filename: "{app}\usagequotatray.exe"; Flags: nowait; Check: ShouldRelaunch

[Code]
const
  // usagequotatray.exe --remove-from-pc exit code (crates/app/src/cli.rs): Claude
  // Code's statusLine could not be put back, and nothing was removed.
  ExitNotUndone = 1;

var
  WasRunning: Boolean;

function InitializeSetup: Boolean;
begin
  // Runs before Setup's AppMutex check, which waits until the tray is closed.
  WasRunning := CheckForMutexes('UsageQuotaTray.Running');
  Result := True;
end;

// An upgrade starts the tray again when it was running before.
function ShouldRelaunch: Boolean;
begin
  Result := WasRunning and not WizardIsTaskSelected('launchnow');
end;

// The Ready page is off, so the Tasks page is the last before installing.
procedure CurPageChanged(CurPageID: Integer);
begin
  if CurPageID = wpSelectTasks then
    WizardForm.NextButton.Caption := SetupMessage(msgButtonInstall)
  else if CurPageID = wpFinished then
    WizardForm.NextButton.Caption := SetupMessage(msgButtonFinish)
  else
    WizardForm.NextButton.Caption := SetupMessage(msgButtonNext);
end;

// After the user confirmed and the tray is closed, before any file is removed: the
// app's own removal (Claude Code's statusLine back first, then the Run value and the
// notification registration, and the data folder if asked). When the statusLine
// cannot be put back, stop and remove nothing.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Params: String;
  ResultCode: Integer;
begin
  if CurUninstallStep <> usUninstall then
    Exit;
  Params := '--remove-from-pc';
  if SuppressibleMsgBox('Also delete your UsageQuotaTray settings and logs?' + #13#10#13#10 +
      ExpandConstant('{localappdata}\UsageQuotaTray'),
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then
    Params := Params + ' ' + '--delete-data';
  if not Exec(ExpandConstant('{app}\usagequotatray.exe'), Params, '', SW_HIDE,
      ewWaitUntilTerminated, ResultCode) then begin
    // A missing or broken exe has nothing left to undo; uninstall the files anyway.
    Log('Could not run usagequotatray.exe ' + Params + ': ' + SysErrorMessage(ResultCode));
    Exit;
  end;
  Log(Format('usagequotatray.exe %s exited with %d', [Params, ResultCode]));
  if ResultCode = ExitNotUndone then begin
    SuppressibleMsgBox('UsageQuotaTray could not put Claude Code''s original statusLine ' +
      'setting back, so nothing was removed.' + #13#10#13#10 +
      'Make sure Claude Code''s settings.json can be written, then uninstall again.',
      mbError, MB_OK, IDOK);
    Abort;
  end;
end;
