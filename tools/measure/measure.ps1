# Measures one UsageQuotaTray run, ported from the UI prototype's measure.ps1.
# Counts the whole process tree and the root's GDI/USER objects.
# Normal run: samples CPU at 10 s and everything at -Seconds (default 20); the app shows
# the popup once at start and hides it after 2 s (UQT_READY_FILE turns that on).
# -Churn: sets UQT_ICON_CHURN=1 (icon redrawn every 500 ms) and samples at 5 s and at
# -Seconds, to expose GDI/USER leaks.
# The app runs with the Fake Assistant and a throwaway LOCALAPPDATA, so the real app-data
# folder is never touched, and with UQT_NOTIFICATIONS=off, so it neither registers the
# notification identity in the registry nor shows toasts.
# These UQT_* switches only work in a build with the `dev-hooks` feature; a normal build
# ignores them (and would register notifications), so the script refuses such an exe.
# Without -Exe it builds one: cargo build --release -p uqt-app --features dev-hooks.
param(
    [string]$Exe,
    [string]$Name = 'usagequotatray',
    [int]$Seconds = 20,
    [switch]$Churn,
    [switch]$WithClaudeCode
)
$ErrorActionPreference = 'Stop'
if (-not $Exe) {
    $repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
    cargo build --release -p uqt-app --features dev-hooks --manifest-path (Join-Path $repo 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
    $Exe = Join-Path $target 'release\usagequotatray.exe'
}
$Exe = (Resolve-Path $Exe).Path
# The switch names are compiled in only with the feature.
$text = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($Exe))
if (-not $text.Contains('UQT_NOTIFICATIONS')) {
    throw "$Exe was built without --features dev-hooks; it would ignore the measurement switches"
}
Add-Type -Namespace Native -Name User32 -MemberDefinition '[DllImport("user32.dll")] public static extern uint GetGuiResources(IntPtr hProcess, uint uiFlags);'

function Get-Tree([int]$RootId) {
    $all = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId)
    $ids = [System.Collections.Generic.List[int]]::new()
    $ids.Add($RootId)
    for ($i = 0; $i -lt $ids.Count; $i++) {
        foreach ($c in $all | Where-Object { $_.ParentProcessId -eq $ids[$i] }) { $ids.Add([int]$c.ProcessId) }
    }
    $ids | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }
}

function Get-Sample($Root) {
    $procs = @(Get-Tree $Root.Id)
    [pscustomobject]@{
        Procs   = $procs.Count
        PrivMB  = [math]::Round(($procs | Measure-Object PrivateMemorySize64 -Sum).Sum / 1MB, 1)
        WSMB    = [math]::Round(($procs | Measure-Object WorkingSet64 -Sum).Sum / 1MB, 1)
        CpuMs   = [math]::Round(($procs | ForEach-Object { $_.TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum, 0)
        Handles = ($procs | Measure-Object HandleCount -Sum).Sum
        Threads = ($procs | ForEach-Object { $_.Threads.Count } | Measure-Object -Sum).Sum
        Gdi     = [Native.User32]::GetGuiResources($Root.Handle, 0)
        User    = [Native.User32]::GetGuiResources($Root.Handle, 1)
    }
}

function Wait-Until([System.Diagnostics.Stopwatch]$Clock, [double]$At) {
    $left = $At - $Clock.Elapsed.TotalSeconds
    if ($left -gt 0) { Start-Sleep -Milliseconds ([int]($left * 1000)) }
}

$sandbox = Join-Path ([IO.Path]::GetTempPath()) "uqt-measure-$([guid]::NewGuid())"
New-Item -ItemType Directory $sandbox | Out-Null
$ready = Join-Path $sandbox 'ready.txt'
$saved = @{ LOCALAPPDATA = $env:LOCALAPPDATA }
$env:LOCALAPPDATA = $sandbox
$env:UQT_FAKE_ASSISTANT = '1'
# Never read the real Claude Code login or call the usage endpoint: by default the Claude
# Code Assistant is off; -WithClaudeCode loads it against an empty throwaway config folder
# ("not installed"), so it is measured without any network request.
$savedClaude = $env:CLAUDE_CONFIG_DIR
$env:CLAUDE_CONFIG_DIR = Join-Path $sandbox 'claude-config-missing'
if ($WithClaudeCode) { Remove-Item Env:UQT_CLAUDE_CODE -ErrorAction SilentlyContinue } else { $env:UQT_CLAUDE_CODE = '0' }
$env:UQT_READY_FILE = $ready
$env:UQT_NOTIFICATIONS = 'off'
if ($Churn) { $env:UQT_ICON_CHURN = '1' } else { Remove-Item Env:UQT_ICON_CHURN -ErrorAction SilentlyContinue }

$clock = [Diagnostics.Stopwatch]::StartNew()
$root = Start-Process $Exe -PassThru -WorkingDirectory (Split-Path $Exe)
$env:LOCALAPPDATA = $saved.LOCALAPPDATA
$readyMs = $null
while ($clock.Elapsed.TotalSeconds -lt 15) {
    if (Test-Path $ready) { $readyMs = $clock.ElapsedMilliseconds; break }
    Start-Sleep -Milliseconds 10
}

try {
    if ($Churn) {
        Wait-Until $clock 5
        $first = Get-Sample $root
        Wait-Until $clock $Seconds
        $last = Get-Sample $root
        [pscustomobject]@{
            V = $Name; ReadyMs = $readyMs
            PrivMB5 = $first.PrivMB; PrivMBEnd = $last.PrivMB
            Gdi5 = $first.Gdi; GdiEnd = $last.Gdi
            User5 = $first.User; UserEnd = $last.User
            Handles5 = $first.Handles; HandlesEnd = $last.Handles
            CpuMsPerMin = [math]::Round(($last.CpuMs - $first.CpuMs) * 60 / ($Seconds - 5), 0)
        } | ConvertTo-Json -Compress
    } else {
        Wait-Until $clock 10
        $mid = Get-Sample $root
        Wait-Until $clock $Seconds
        $last = Get-Sample $root
        [pscustomobject]@{
            V = $Name; ReadyMs = $readyMs; Procs = $last.Procs
            PrivMB = $last.PrivMB; WSMB = $last.WSMB
            IdleCpuMs = $last.CpuMs - $mid.CpuMs; TotalCpuMs = $last.CpuMs
            Threads = $last.Threads; Handles = $last.Handles; Gdi = $last.Gdi; User = $last.User
        } | ConvertTo-Json -Compress
    }
} finally {
    Get-Tree $root.Id | Stop-Process -Force -ErrorAction SilentlyContinue
    Remove-Item Env:UQT_READY_FILE, Env:UQT_ICON_CHURN, Env:UQT_FAKE_ASSISTANT, Env:UQT_CLAUDE_CODE, Env:UQT_NOTIFICATIONS -ErrorAction SilentlyContinue
    if ($null -eq $savedClaude) { Remove-Item Env:CLAUDE_CONFIG_DIR -ErrorAction SilentlyContinue } else { $env:CLAUDE_CONFIG_DIR = $savedClaude }
    Start-Sleep -Milliseconds 300
    Remove-Item $sandbox -Recurse -Force -ErrorAction SilentlyContinue
}
