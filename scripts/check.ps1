# Runs every check a change has to pass: formatting, clippy (warnings are errors) and tests.
# Usage: pwsh scripts/check.ps1 [-Coverage]
param(
    [switch]$Coverage
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

function Invoke-Step([string]$Name, [scriptblock]$Command) {
    Write-Host "==> $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAILED: $Name" -ForegroundColor Red
        exit $LASTEXITCODE
    }
}

Invoke-Step 'cargo fmt --check' { cargo fmt --check }
Invoke-Step 'cargo clippy' { cargo clippy --all-targets -q -- -D warnings }
Invoke-Step 'cargo test' { cargo test -q }
if ($Coverage) {
    Invoke-Step 'cargo llvm-cov' { cargo llvm-cov --summary-only }
}
Write-Host 'All checks passed.' -ForegroundColor Green
