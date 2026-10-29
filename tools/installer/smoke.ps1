# Smoke test for the installer: a silent install into a throwaway folder with
# "Start with Windows", checks, then a silent uninstall, checks, cleanup.
#
#   pwsh tools/installer/smoke.ps1 [-Setup <path to UsageQuotaTray-Setup-<version>.exe>]
#
# Inno Setup writes to the real current user, so this touches, and puts back:
#   - HKCU\...\CurrentVersion\Run value UsageQuotaTray (created, then removed)
#   - HKCU\...\CurrentVersion\Uninstall\{9C02A488-...}_is1 (created, then removed)
#   - the Start menu shortcut <Programs>\UsageQuotaTray.lnk (created, then removed)
#   - HKCU\Software\Classes\AppUserModelId\UsageQuotaTray: the uninstaller's
#     "--remove-from-pc" deletes it, so it is exported first and imported back.
# It refuses to run when the Run value, the Apps entry or the shortcut already exist,
# or when a tray is running. The tray is never started. The uninstaller runs with
# LOCALAPPDATA, USERPROFILE and CLAUDE_CONFIG_DIR pointed at the throwaway folder, so
# the real app data and Claude Code's settings.json are not read or written; both are
# fingerprinted before and after to prove it.
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
# Any running copy (older ones hold no marker) would write the real app data meanwhile.
if ((Test-TrayRunning) -or (Get-Process -Name 'usagequotatray' -ErrorAction SilentlyContinue)) {
    throw 'Aborting: UsageQuotaTray is running; exit it first.'
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ('uqt-smoke-' + [guid]::NewGuid().ToString('N'))
$dir = Join-Path $work 'app'
$sandbox = @{
    LOCALAPPDATA      = Join-Path $work 'localappdata'
    USERPROFILE       = Join-Path $work 'profile'
    CLAUDE_CONFIG_DIR = Join-Path $work 'profile\.claude'
}
New-Item -ItemType Directory -Path $work | Out-Null
$sandbox.Values | ForEach-Object { New-Item -ItemType Directory -Force -Path $_ | Out-Null }

$aumidBefore = Get-KeyValues $aumidKey
$aumidBackup = Join-Path $work 'aumid.reg'
if ($aumidBefore -ne '(absent)') {
    & reg.exe export $aumidRegExe $aumidBackup /y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Aborting: could not back up $aumidRegExe." }
}
$dataBefore = Get-Fingerprint @($realData)
$claudeBefore = Get-Fingerprint $claudeFiles
"Before: AppUserModelId key: $aumidBefore"

$setupLog = Join-Path $work 'setup.log'
$uninstallLog = Join-Path $work 'uninstall.log'
try {
    # Install.
    $p = Start-Process -FilePath $Setup -Wait -PassThru -ArgumentList @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/CURRENTUSER', '/NORESTART',
        '/TASKS="startwithwindows"', "/DIR=`"$dir`"", "/LOG=`"$setupLog`"")
    "Install exit code: $($p.ExitCode)"
    Check ($p.ExitCode -eq 0) 'setup exits 0'
    foreach ($f in 'usagequotatray.exe', 'uqt-statusline.exe', 'LICENSE', 'unins000.exe') {
        Check (Test-Path -LiteralPath (Join-Path $dir $f)) "installed $f"
    }
    $exe = Join-Path $dir 'usagequotatray.exe'
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
    Check (-not (Test-TrayRunning) -and -not (Get-Process -Name 'usagequotatray' -ErrorAction SilentlyContinue)) 'the tray was not started'

    # Uninstall, sandboxed. The exit code comes back before the uninstaller's clone
    # finishes, so wait for the Apps entry and the uninstaller to disappear.
    $saved = @{}
    foreach ($k in $sandbox.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $sandbox[$k]) }
    try {
        $p = Start-Process -FilePath (Join-Path $dir 'unins000.exe') -Wait -PassThru -ArgumentList @(
            '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$uninstallLog`"")
    }
    finally {
        foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
    }
    "Uninstall exit code: $($p.ExitCode)"
    Check ($p.ExitCode -eq 0) 'uninstaller exits 0'
    $deadline = (Get-Date).AddSeconds(60)
    while (((Test-Path -LiteralPath $appsKey) -or (Test-Path -LiteralPath (Join-Path $dir 'unins000.exe'))) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Seconds 1
    $log = if (Test-Path -LiteralPath $uninstallLog) { Get-Content -LiteralPath $uninstallLog -Raw } else { '' }
    Check ($log -match 'usagequotatray\.exe --remove-from-pc exited with 0') 'uninstaller ran --remove-from-pc (exit 0)'
    Check ($null -eq (Get-RunValue)) 'Run value removed'
    Check (-not (Test-Path -LiteralPath $aumidKey)) 'AppUserModelId key removed by --remove-from-pc'
    Check (-not (Test-Path -LiteralPath $appsKey)) 'Apps entry removed'
    Check (-not (Test-Path -LiteralPath $shortcut)) 'shortcut removed'
    $left = @(if (Test-Path -LiteralPath $dir) { Get-ChildItem -LiteralPath $dir -Recurse -Force })
    Check ($left.Count -eq 0) "installed files removed ($($left.Count) left)"
    Check (-not (Test-Path -LiteralPath (Join-Path $sandbox.LOCALAPPDATA 'UsageQuotaTray'))) 'sandbox app data untouched'
}
finally {
    # Put back what this run changed, whatever happened above.
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

Check ((Get-KeyValues $aumidKey) -eq $aumidBefore) "AppUserModelId key as before ($(Get-KeyValues $aumidKey))"
Check (((Get-Fingerprint @($realData)) -join "`n") -eq ($dataBefore -join "`n")) "real $realData untouched"
Check (((Get-Fingerprint $claudeFiles) -join "`n") -eq ($claudeBefore -join "`n")) "Claude Code settings.json untouched ($($claudeFiles -join ', '))"
Check ($null -eq (Get-RunValue)) 'Run value absent, as before'

if ($failures.Count -eq 0) {
    Remove-Item -LiteralPath $work -Recurse -Force
    'Smoke test passed.'
}
else {
    "Smoke test FAILED ($($failures.Count)). Logs kept in $work"
    exit 1
}
