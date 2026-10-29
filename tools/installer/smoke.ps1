# Smoke test for the installer, all silent, in a throwaway folder:
#   1. a fresh install with "Start with Windows";
#   2. a tray started from that install, then an upgrade over it: Setup closes it with
#      --quit, keeps the Run value, and starts the installed tray again;
#   3. Start with Windows turned off (the Run value deleted, as the app's toggle does),
#      then another upgrade over the running tray: the value stays off;
#   4. an uninstall whose --remove-from-pc fails (the sandbox Claude Code settings run
#      the installed statusline helper and cannot be written): it stops, removes
#      nothing, and the statusLine is unchanged;
#   5. the uninstall again once the settings can be written: the statusLine is put
#      back and everything is gone.
#
#   pwsh tools/installer/smoke.ps1 [-Setup <path to UsageQuotaTray-Setup-<version>.exe>]
#
# Inno Setup writes to the real current user, so this touches, and puts back:
#   - HKCU\...\CurrentVersion\Run value UsageQuotaTray (created, then removed)
#   - HKCU\...\CurrentVersion\Uninstall\{9C02A488-...}_is1 (created, then removed)
#   - the Start menu shortcut <Programs>\UsageQuotaTray.lnk (created, then removed)
#   - HKCU\Software\Classes\AppUserModelId\UsageQuotaTray: the running tray registers
#     it and "--remove-from-pc" deletes it, so it is exported first and imported back.
# It refuses to run when the Run value, the Apps entry or the shortcut already exist,
# or when a tray is running. Setup, the uninstaller and the tray run with LOCALAPPDATA
# and CLAUDE_CONFIG_DIR pointed at the throwaway folder (the tray has no credentials
# there, so it calls no server; CLAUDE_CONFIG_DIR wins over USERPROFILE, which stays
# real because Windows expands the Start menu folder from it), so the real app data and
# Claude Code's settings.json are not read or written; both are fingerprinted before
# and after to prove it. Any tray started from the throwaway folder is stopped at the end.
[CmdletBinding()]
param(
    [string]$Setup
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$appId = '{9C02A488-5E56-4016-835D-BE4F735473FD}'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$appsKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$($appId)_is1"
$aumidKey = 'HKCU:\Software\Classes\AppUserModelId\UsageQuotaTray'
$aumidRegExe = 'HKCU\Software\Classes\AppUserModelId\UsageQuotaTray'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'UsageQuotaTray.lnk'
$realData = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'UsageQuotaTray'
$claudeDirs = @(Join-Path $env:USERPROFILE '.claude')
if ($env:CLAUDE_CONFIG_DIR) { $claudeDirs += $env:CLAUDE_CONFIG_DIR }
$claudeFiles = $claudeDirs | ForEach-Object { Join-Path $_ 'settings.json' }

$failures = [System.Collections.Generic.List[string]]::new()
function Check([bool]$ok, [string]$what) {
    if ($ok) { "  ok    $what" } else { "  FAIL  $what"; $failures.Add($what) }
}

function Get-RunValue {
    $item = Get-ItemProperty -LiteralPath $runKey -ErrorAction SilentlyContinue
    if ($item -and $item.PSObject.Properties['UsageQuotaTray']) { $item.UsageQuotaTray } else { $null }
}

function Get-KeyValues([string]$key) {
    if (-not (Test-Path -LiteralPath $key)) { return '(absent)' }
    $item = Get-Item -LiteralPath $key
    ($item.GetValueNames() | Sort-Object | ForEach-Object { "$_=$($item.GetValue($_))" }) -join '|'
}

function Get-Fingerprint([string[]]$paths) {
    foreach ($p in $paths) {
        if (Test-Path -LiteralPath $p -PathType Leaf) {
            "$p $((Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash) $((Get-Item -LiteralPath $p).LastWriteTimeUtc.Ticks)"
        }
        elseif (Test-Path -LiteralPath $p -PathType Container) {
            Get-ChildItem -LiteralPath $p -Recurse -File -Force |
                Sort-Object FullName |
                ForEach-Object { "$($_.FullName) $($_.Length) $($_.LastWriteTimeUtc.Ticks)" }
        }
        else { "$p (absent)" }
    }
}

function Test-TrayRunning {
    $m = $null
    if ([System.Threading.Mutex]::TryOpenExisting('UsageQuotaTray.Running', [ref]$m)) { $m.Dispose(); return $true }
    return $false
}

# The tray processes started from the throwaway folder.
function Get-SandboxTray {
    @(Get-Process -Name 'usagequotatray' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.StartsWith($work, [StringComparison]::OrdinalIgnoreCase) })
}

function Wait-Until([scriptblock]$condition, [int]$seconds) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while (-not (& $condition) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 250 }
    & $condition
}

# Runs $action with the sandbox environment, which child processes inherit.
function Invoke-Sandboxed([scriptblock]$action) {
    $saved = @{}
    foreach ($k in $sandbox.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $sandbox[$k]) }
    try { & $action }
    finally { foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) } }
}

# Starts $file and waits for it alone: Start-Process -Wait would also wait for the
# tray Setup starts again.
function Invoke-Waited([string]$file, [string[]]$arguments) {
    $info = [System.Diagnostics.ProcessStartInfo]::new($file, ($arguments -join ' '))
    $info.UseShellExecute = $false
    $p = Invoke-Sandboxed { [System.Diagnostics.Process]::Start($info) }
    $p.WaitForExit()
    $p.ExitCode
}

function Invoke-Setup([string]$log, [string[]]$extra) {
    Invoke-Waited $Setup (@('/VERYSILENT', '/SUPPRESSMSGBOXES', '/CURRENTUSER', '/NORESTART', "/DIR=`"$dir`"", "/LOG=`"$log`"") + $extra)
}

# The uninstaller returns before its copy in %TEMP% finishes; wait for its log to end.
function Invoke-Uninstall([string]$log) {
    $code = Invoke-Waited (Join-Path $dir 'unins000.exe') @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"")
    $null = Wait-Until { (Test-Path -LiteralPath $log) -and ((Get-Content -LiteralPath $log -Raw) -match 'Log closed\.') } 60
    $code
}

function Read-Log([string]$log) {
    if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log -Raw } else { '' }
}

if (-not $Setup) {
    $found = Get-ChildItem -LiteralPath (Join-Path $root 'dist') -Filter 'UsageQuotaTray-Setup-*.exe' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $found) { throw 'No installer in dist\; run tools/installer/build.ps1 first or pass -Setup.' }
    $Setup = $found.FullName
}
"Installer: $Setup"

# Preconditions: never overwrite anything the user already has.
$existing = Get-RunValue
if ($null -ne $existing) { throw "Aborting: the Run value UsageQuotaTray already exists ($existing)." }
if (Test-Path -LiteralPath $appsKey) { throw "Aborting: UsageQuotaTray is already installed ($appsKey)." }
if (Test-Path -LiteralPath $shortcut) { throw "Aborting: $shortcut already exists." }
# Any running copy (older ones hold no marker) would write the real app data meanwhile,
# and the upgrade's --quit would close it.
if ((Test-TrayRunning) -or (Get-Process -Name 'usagequotatray' -ErrorAction SilentlyContinue)) {
    throw 'Aborting: UsageQuotaTray is running; exit it first.'
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ('uqt-smoke-' + [guid]::NewGuid().ToString('N'))
$dir = Join-Path $work 'app'
$sandbox = @{
    LOCALAPPDATA      = Join-Path $work 'localappdata'
    CLAUDE_CONFIG_DIR = Join-Path $work 'claude'
}
New-Item -ItemType Directory -Path $work | Out-Null
$sandbox.Values | ForEach-Object { New-Item -ItemType Directory -Force -Path $_ | Out-Null }
$exe = Join-Path $dir 'usagequotatray.exe'
$sandboxSettings = Join-Path $sandbox.CLAUDE_CONFIG_DIR 'settings.json'

$aumidBefore = Get-KeyValues $aumidKey
$aumidBackup = Join-Path $work 'aumid.reg'
if ($aumidBefore -ne '(absent)') {
    & reg.exe export $aumidRegExe $aumidBackup /y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Aborting: could not back up $aumidRegExe." }
}
$dataBefore = Get-Fingerprint @($realData)
$claudeBefore = Get-Fingerprint $claudeFiles
"Before: AppUserModelId key: $aumidBefore"

try {
    '1. Fresh install with Start with Windows'
    $code = Invoke-Setup (Join-Path $work 'setup-1.log') @('/TASKS="startwithwindows"')
    Check ($code -eq 0) "setup exits 0 ($code)"
    foreach ($f in 'usagequotatray.exe', 'uqt-statusline.exe', 'LICENSE', 'unins000.exe') {
        Check (Test-Path -LiteralPath (Join-Path $dir $f)) "installed $f"
    }
    Check ((Get-RunValue) -eq "`"$exe`"") "Run value is the quoted exe path ($(Get-RunValue))"
    Check (Test-Path -LiteralPath $shortcut) "Start menu shortcut $shortcut"
    if (Test-Path -LiteralPath $shortcut) {
        $lnk = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
        Check ($lnk.TargetPath -eq $exe) "shortcut targets $exe"
    }
    Check (Test-Path -LiteralPath $appsKey) 'Apps & features entry'
    if (Test-Path -LiteralPath $appsKey) {
        Check ((Get-ItemProperty -LiteralPath $appsKey).DisplayName -eq 'UsageQuotaTray') 'Apps entry is named UsageQuotaTray'
    }
    Check (-not (Test-TrayRunning) -and @(Get-SandboxTray).Count -eq 0) 'the tray was not started'

    '2. Upgrade over a running tray'
    Invoke-Sandboxed { Start-Process -FilePath $exe | Out-Null }
    Check (Wait-Until { Test-TrayRunning } 15) 'tray started from the install holds UsageQuotaTray.Running'
    $before = @(Get-SandboxTray | ForEach-Object Id)
    $log = Join-Path $work 'setup-2.log'
    $code = Invoke-Setup $log @()
    Check ($code -eq 0) "upgrade exits 0 ($code)"
    Check ((Read-Log $log) -match 'usagequotatray\.exe --quit exited with 0') 'Setup closed the tray with --quit (exit 0)'
    Check (Wait-Until { @(Get-SandboxTray).Count -eq 1 -and (Test-TrayRunning) } 15) 'the installed tray runs again'
    Check (@(Get-SandboxTray | Where-Object { $before -contains $_.Id }).Count -eq 0) 'it is a new process'
    Check ((Get-RunValue) -eq "`"$exe`"") "Start with Windows still on ($(Get-RunValue))"

    '3. Start with Windows off, then upgrade'
    Remove-ItemProperty -LiteralPath $runKey -Name 'UsageQuotaTray'
    $log = Join-Path $work 'setup-3.log'
    $code = Invoke-Setup $log @()
    Check ($code -eq 0) "upgrade exits 0 ($code)"
    Check ((Read-Log $log) -match 'usagequotatray\.exe --quit exited with 0') 'Setup closed the tray with --quit (exit 0)'
    Check ($null -eq (Get-RunValue)) 'Start with Windows still off'
    Check (Wait-Until { @(Get-SandboxTray).Count -eq 1 } 15) 'the installed tray runs again'

    '4. Uninstall whose removal fails'
    $helper = (Join-Path $dir 'uqt-statusline.exe').Replace('\', '/')
    if ($helper -match '[^A-Za-z0-9/:._~-]') { throw "Aborting: the helper path needs quoting ($helper)." }
    $settingsText = "{`"statusLine`": {`"type`": `"command`", `"command`": `"$helper`"}}`n"
    [System.IO.File]::WriteAllText($sandboxSettings, $settingsText)
    # The installer writes settings.uqt-tmp and renames it over settings.json; a folder
    # by that name makes the write fail, so the statusLine cannot be put back.
    $blocker = Join-Path $sandbox.CLAUDE_CONFIG_DIR 'settings.uqt-tmp'
    New-Item -ItemType Directory -Path $blocker | Out-Null
    Remove-ItemProperty -LiteralPath $runKey -Name 'UsageQuotaTray' -ErrorAction SilentlyContinue
    Set-ItemProperty -LiteralPath $runKey -Name 'UsageQuotaTray' -Value "`"$exe`""
    $log = Join-Path $work 'uninstall-1.log'
    $code = Invoke-Uninstall $log
    "  uninstaller exit code: $code"
    $text = Read-Log $log
    Check ($text -match 'usagequotatray\.exe --quit exited with 0') 'the uninstaller closed the tray with --quit (exit 0)'
    Check ($text -match 'usagequotatray\.exe --remove-from-pc exited with 1') '--remove-from-pc exited 1'
    Check ($text -match 'Uninstall stopped: UsageQuotaTray could not put') 'the uninstaller stopped with the message'
    Check (Test-Path -LiteralPath $appsKey) 'Apps entry kept'
    foreach ($f in 'usagequotatray.exe', 'uqt-statusline.exe', 'LICENSE', 'unins000.exe') {
        Check (Test-Path -LiteralPath (Join-Path $dir $f)) "kept $f"
    }
    Check (Test-Path -LiteralPath $shortcut) 'shortcut kept'
    Check ((Get-RunValue) -eq "`"$exe`"") 'Run value kept'
    Check ([System.IO.File]::ReadAllText($sandboxSettings) -eq $settingsText) 'sandbox statusLine unchanged'
    Check (@(Get-SandboxTray).Count -eq 0) 'no tray running'

    '5. Uninstall again'
    Remove-Item -LiteralPath $blocker
    $log = Join-Path $work 'uninstall-2.log'
    $code = Invoke-Uninstall $log
    Check ($code -eq 0) "uninstaller exits 0 ($code)"
    $null = Wait-Until { -not (Test-Path -LiteralPath $appsKey) -and -not (Test-Path -LiteralPath (Join-Path $dir 'unins000.exe')) } 60
    Start-Sleep -Seconds 1
    Check ((Read-Log $log) -match 'usagequotatray\.exe --remove-from-pc exited with 0') 'uninstaller ran --remove-from-pc (exit 0)'
    Check (-not ([System.IO.File]::ReadAllText($sandboxSettings) -match 'statusLine')) 'sandbox statusLine put back (removed)'
    Check ($null -eq (Get-RunValue)) 'Run value removed'
    Check (-not (Test-Path -LiteralPath $aumidKey)) 'AppUserModelId key removed by --remove-from-pc'
    Check (-not (Test-Path -LiteralPath $appsKey)) 'Apps entry removed'
    Check (-not (Test-Path -LiteralPath $shortcut)) 'shortcut removed'
    $left = @(if (Test-Path -LiteralPath $dir) { Get-ChildItem -LiteralPath $dir -Recurse -Force })
    Check ($left.Count -eq 0) "installed files removed ($($left.Count) left)"
    Check (Test-Path -LiteralPath (Join-Path $sandbox.LOCALAPPDATA 'UsageQuotaTray')) 'sandbox app data kept (delete data defaults to No)'
}
finally {
    # Put back what this run changed, whatever happened above.
    foreach ($p in Get-SandboxTray) { Stop-Process -Id $p.Id -Force; "  cleanup: stopped tray $($p.Id)" }
    if ($null -ne (Get-RunValue)) { Remove-ItemProperty -LiteralPath $runKey -Name 'UsageQuotaTray'; '  cleanup: removed Run value' }
    if (Test-Path -LiteralPath $appsKey) { Remove-Item -LiteralPath $appsKey -Recurse; '  cleanup: removed Apps entry' }
    if (Test-Path -LiteralPath $shortcut) { Remove-Item -LiteralPath $shortcut; '  cleanup: removed shortcut' }
    if (Test-Path -LiteralPath $aumidBackup) {
        & reg.exe import $aumidBackup 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) { "  cleanup: FAILED to restore $aumidRegExe from $aumidBackup"; $failures.Add('restore AppUserModelId key') }
        else { '  cleanup: restored the AppUserModelId key' }
    }
    elseif (Test-Path -LiteralPath $aumidKey) { Remove-Item -LiteralPath $aumidKey -Recurse; '  cleanup: removed AppUserModelId key' }
}

'Final state'
Check ((Get-KeyValues $aumidKey) -eq $aumidBefore) "AppUserModelId key as before ($(Get-KeyValues $aumidKey))"
Check (((Get-Fingerprint @($realData)) -join "`n") -eq ($dataBefore -join "`n")) "real $realData untouched"
Check (((Get-Fingerprint $claudeFiles) -join "`n") -eq ($claudeBefore -join "`n")) "Claude Code settings.json untouched ($($claudeFiles -join ', '))"
Check ($null -eq (Get-RunValue)) 'Run value absent, as before'
Check (-not (Test-Path -LiteralPath $appsKey)) 'Apps entry absent, as before'
Check (-not (Test-Path -LiteralPath $shortcut)) 'shortcut absent, as before'
Check (-not (Test-TrayRunning)) 'no tray running, as before'

if ($failures.Count -eq 0) {
    Remove-Item -LiteralPath $work -Recurse -Force
    'Smoke test passed.'
}
else {
    "Smoke test FAILED ($($failures.Count)). Logs kept in $work"
    exit 1
}
