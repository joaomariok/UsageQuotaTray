# The gate: every check a change must pass before it is committed. CI runs this same
# script, so a green run here is a green run there.
#
#   pwsh tools/gate.ps1 [-Jobs <n>]
#
# Runs fmt, clippy, the tests and cargo deny in that order and stops at the first
# failure with its exit code. -Jobs limits cargo's parallel jobs (clippy and tests).
[CmdletBinding()]
param(
    [int]$Jobs = 0
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$jobArgs = if ($Jobs -gt 0) { @('-j', "$Jobs") } else { @() }

$steps = @(
    @('fmt', '--all', '--check'),
    (@('clippy', '--workspace', '--all-targets') + $jobArgs + @('--', '-D', 'warnings')),
    (@('test', '--workspace') + $jobArgs),
    @('deny', 'check')
)

Push-Location $root
try {
    foreach ($step in $steps) {
        Write-Host "==> cargo $($step -join ' ')"
        & cargo @step
        if ($LASTEXITCODE -ne 0) {
            Write-Host "gate failed at: cargo $($step -join ' ')"
            exit $LASTEXITCODE
        }
    }
}
finally {
    Pop-Location
}
Write-Host 'gate passed'
