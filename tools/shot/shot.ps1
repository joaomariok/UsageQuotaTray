# Screenshots the popup or the settings window of a debug dev-hooks build, plus the
# right end of the taskbar (where the tray icon sits) as <Out>.taskbar.png.
#
#   cargo build -p uqt-app --features dev-hooks
#   pwsh tools/shot/shot.ps1 [-Shot settings|popup] [-Out <png>] [-Width <px>] [-Height <px>]
#                            [-Drag <px>] [-Click "x,y;x,y"] [-NoScroll]
#
# The app runs with the Fake Assistant, UQT_NOTIFICATIONS=off, LOCALAPPDATA in
# target\dev-appdata (settings saved there persist between runs) and Claude Code reading
# an empty config folder, so neither the real app data nor real credentials are touched.
# -Width/-Height resize the window, -Drag drags its right edge left by that many pixels,
# -Click clicks points relative to its top-left corner, and the content is scrolled to
# the bottom unless -NoScroll. Output defaults to target\shots\<Shot>.png.
[CmdletBinding()]
param(
    [ValidateSet('settings', 'popup')][string]$Shot = 'settings',
    [string]$Out,
    [int]$Width = 0,
    [int]$Height = 0,
    [int]$Drag = 0,
    [string]$Click = '',
    [switch]$NoScroll
)
$ErrorActionPreference = 'Stop'

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$exe = Join-Path $target 'debug\usagequotatray.exe'
if (-not (Test-Path -LiteralPath $exe)) { throw "$exe not found: run cargo build -p uqt-app --features dev-hooks" }
# The switch names are compiled in only with the feature.
$text = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($exe))
if (-not $text.Contains('UQT_SHOT')) { throw "$exe was built without --features dev-hooks" }
if (-not $Out) {
    $shots = Join-Path $target 'shots'
    New-Item -ItemType Directory -Force $shots | Out-Null
    $Out = Join-Path $shots "$Shot.png"
}
# System.Drawing resolves relative paths against the process folder, not $PWD.
$Out = [IO.Path]::GetFullPath($Out, (Get-Location).Path)

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class ShotWin {
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rt, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, int d, IntPtr e);
  public delegate bool EP(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EP cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int ht, bool rp);
  // The largest visible window of the process, ignoring the tiny helper windows.
  public static IntPtr Find(uint pid) {
    IntPtr best = IntPtr.Zero; int area = 0;
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p);
      if (p == pid && IsWindowVisible(h)) { R r; GetWindowRect(h, out r); int a = (r.Rt-r.L)*(r.B-r.T); if (a > area && a > 100*100) { area = a; best = h; } }
      return true; }, IntPtr.Zero);
    return best;
  }
}
'@

function Click-At([int]$x, [int]$y) {
    [ShotWin]::SetCursorPos($x, $y) | Out-Null
    Start-Sleep -Milliseconds 300
    [ShotWin]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
    [ShotWin]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 900
}

$sandbox = Join-Path $target 'dev-appdata'
$emptyClaude = Join-Path $target 'dev-claude-empty'
New-Item -ItemType Directory -Force $sandbox, $emptyClaude | Out-Null
$env:LOCALAPPDATA = $sandbox
$env:CLAUDE_CONFIG_DIR = $emptyClaude
$env:UQT_FAKE_ASSISTANT = '1'
$env:UQT_NOTIFICATIONS = 'off'
$env:UQT_SHOT = $Shot

$p = Start-Process $exe -PassThru
try {
    $h = [IntPtr]::Zero
    for ($i = 0; $i -lt 120 -and $h -eq [IntPtr]::Zero; $i++) {
        Start-Sleep -Milliseconds 250
        if ($p.HasExited) { throw "the app exited with code $($p.ExitCode) before showing a window" }
        $h = [ShotWin]::Find([uint32]$p.Id)
    }
    if ($h -eq [IntPtr]::Zero) { throw "no $Shot window within 30 s (is another UsageQuotaTray running?)" }
    Start-Sleep -Seconds 2
    $r = New-Object ShotWin+R
    [ShotWin]::GetWindowRect($h, [ref]$r) | Out-Null
    if ($Height -gt 0 -or $Width -gt 0) {
        $w = if ($Width -gt 0) { $Width } else { $r.Rt - $r.L }
        $ht = if ($Height -gt 0) { $Height } else { $r.B - $r.T }
        [ShotWin]::MoveWindow($h, $r.L, $r.T, $w, $ht, $true) | Out-Null
        Start-Sleep -Seconds 1
        [ShotWin]::GetWindowRect($h, [ref]$r) | Out-Null
    }
    [ShotWin]::SetForegroundWindow($h) | Out-Null
    if ($Drag -gt 0) {
        $y = [int](($r.T + $r.B) / 2); $x = $r.Rt - 3
        [ShotWin]::SetCursorPos($x, $y) | Out-Null; Start-Sleep -Milliseconds 300
        [ShotWin]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 200
        for ($k = 1; $k -le 20; $k++) { [ShotWin]::SetCursorPos($x - [int]($Drag * $k / 20), $y) | Out-Null; Start-Sleep -Milliseconds 30 }
        [ShotWin]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 800
        [ShotWin]::GetWindowRect($h, [ref]$r) | Out-Null
    }
    Start-Sleep -Milliseconds 700
    foreach ($c in ($Click -split ';' | Where-Object { $_ })) {
        $xy = $c -split ','
        Click-At ($r.L + [int]$xy[0]) ($r.T + [int]$xy[1])
    }
    if (-not $NoScroll) {
        [ShotWin]::SetCursorPos([int](($r.L + $r.Rt) / 2), [int](($r.T + $r.B) / 2)) | Out-Null
        for ($k = 0; $k -lt 15; $k++) { [ShotWin]::mouse_event(0x0800, 0, 0, -120, [IntPtr]::Zero); Start-Sleep -Milliseconds 60 }
    }
    Start-Sleep -Milliseconds 800
    $w = $r.Rt - $r.L; $ht = $r.B - $r.T
    $bmp = New-Object System.Drawing.Bitmap $w, $ht
    [System.Drawing.Graphics]::FromImage($bmp).CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size)
    $bmp.Save($Out)
    $sc = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $tb = New-Object System.Drawing.Bitmap 600, 60
    [System.Drawing.Graphics]::FromImage($tb).CopyFromScreen($sc.Right - 600, $sc.Bottom - 60, 0, 0, $tb.Size)
    $tb.Save("$Out.taskbar.png")
    "$Out (${w}x${ht})"
    "$Out.taskbar.png"
}
finally {
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
}
