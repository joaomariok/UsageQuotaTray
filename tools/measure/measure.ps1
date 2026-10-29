# Measures one UsageQuotaTray run, ported from the UI prototype's measure.ps1.
# Counts the whole process tree and the root's GDI/USER objects.
# Normal run: samples CPU at 10 s and everything at -Seconds (default 20); the app shows
# the popup once at start and hides it after 2 s (UQT_READY_FILE turns that on).
# -Churn: sets UQT_ICON_CHURN=1 (icon redrawn every 500 ms) and samples at 5 s and at
# -Seconds, to expose GDI/USER leaks.
# The app runs with the Fake Assistant and a throwaway LOCALAPPDATA, so the real app-data
# folder is never touched.
param(
    [Parameter(Mandatory)][string]$Exe,
    [string]$Name = 'usagequotatray',
    [int]$Seconds = 20,
    [switch]$Churn
)
$ErrorActionPreference = 'Stop'
$Exe = (Resolve-Path $Exe).Path
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
$env:UQT_READY_FILE = $ready
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
    Remove-Item Env:UQT_READY_FILE, Env:UQT_ICON_CHURN, Env:UQT_FAKE_ASSISTANT -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 300
    Remove-Item $sandbox -Recurse -Force -ErrorAction SilentlyContinue
}
