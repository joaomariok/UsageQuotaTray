# Builds the release exes and packages them into dist\: the per-user installer
# (UsageQuotaTray-Setup-<version>.exe, Inno Setup 6) and the portable zip
# (UsageQuotaTray-<version>.zip). The version comes from crates/app/Cargo.toml.
#
#   pwsh tools/installer/build.ps1 [-Iscc <path to ISCC.exe>]
#
# ISCC.exe is looked up as given, then on PATH, then from the Inno Setup 6 install
# recorded in the registry. Honors CARGO_TARGET_DIR.
[CmdletBinding()]
param(
    [string]$Iscc
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$dist = Join-Path $root 'dist'

function Find-Iscc {
    if ($Iscc) {
        if (-not (Test-Path -LiteralPath $Iscc -PathType Leaf)) { throw "ISCC.exe not found at $Iscc" }
        return (Resolve-Path -LiteralPath $Iscc).Path
    }
    $onPath = Get-Command 'ISCC.exe' -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    $key = 'Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1'
    foreach ($path in "HKCU:\Software\$key", "HKLM:\Software\$key", "HKLM:\Software\WOW6432Node\$key") {
        $item = Get-ItemProperty -LiteralPath $path -ErrorAction SilentlyContinue
        if ($item -and $item.PSObject.Properties['InstallLocation']) {
            $exe = Join-Path $item.InstallLocation 'ISCC.exe'
            if (Test-Path -LiteralPath $exe -PathType Leaf) { return $exe }
        }
    }
    throw 'ISCC.exe (Inno Setup 6) not found: pass -Iscc, put it on PATH, or install Inno Setup 6.'
}

function Invoke-Checked([string]$exe, [string[]]$arguments) {
    & $exe @arguments
    if ($LASTEXITCODE -ne 0) { throw "$exe failed with exit code $LASTEXITCODE" }
}

$iscc = Find-Iscc

Push-Location $root
try {
    $metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed with exit code $LASTEXITCODE" }
    $version = ($metadata.packages | Where-Object name -eq 'uqt-app').version
    if (-not $version) { throw 'uqt-app not found in cargo metadata' }

    Invoke-Checked 'cargo' @('build', '--release', '-p', 'uqt-app')
}
finally {
    Pop-Location
}

$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$bin = (Resolve-Path (Join-Path $target 'release')).Path
foreach ($name in 'usagequotatray.exe', 'uqt-statusline.exe') {
    if (-not (Test-Path -LiteralPath (Join-Path $bin $name))) { throw "$name not found in $bin" }
}

New-Item -ItemType Directory -Force -Path $dist | Out-Null

# The portable zip: both exes side by side, plus the license and the README.
$zip = Join-Path $dist "UsageQuotaTray-$version.zip"
$stage = Join-Path ([System.IO.Path]::GetTempPath()) ("uqt-zip-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    Copy-Item -LiteralPath (Join-Path $bin 'usagequotatray.exe'), (Join-Path $bin 'uqt-statusline.exe') -Destination $stage
    Copy-Item -LiteralPath (Join-Path $root 'LICENSE'), (Join-Path $root 'README.md') -Destination $stage
    if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip }
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
}
finally {
    Remove-Item -LiteralPath $stage -Recurse -Force
}

# The installer.
$script = Join-Path $PSScriptRoot 'UsageQuotaTray.iss'
Invoke-Checked $iscc @('/Q', "/DAppVersion=$version", "/DBinDir=$bin", "/O$dist", $script)
$setup = Join-Path $dist "UsageQuotaTray-Setup-$version.exe"
if (-not (Test-Path -LiteralPath $setup)) { throw "ISCC did not produce $setup" }

foreach ($file in $setup, $zip) {
    $item = Get-Item -LiteralPath $file
    $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash
    '{0}  {1:N0} bytes  SHA256 {2}' -f $item.FullName, $item.Length, $hash
}
