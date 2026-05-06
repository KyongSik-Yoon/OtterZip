# Sprint 6 RC — Local AOT publish smoke check.
#
# Verifies the WinUI3 host publishes cleanly under Native AOT. Required
# environment: Visual Studio 2022 17.8+ (Build Tools or full IDE) so ILC
# can locate `link.exe` via vswhere. CI's `windows-latest` runner has it
# preinstalled.
#
# Usage:
#   pwsh tools/aot-check.ps1            # Release win-x64 publish
#   pwsh tools/aot-check.ps1 -Quick     # restore + native-codegen only
#
# Exit codes:
#   0   AOT publish succeeded
#   1   ILC failed — diagnose with the printed error log
#   2   Toolchain missing — install VS Build Tools (Desktop development with C++)

[CmdletBinding()]
param(
    [switch]$Quick
)

$ErrorActionPreference = "Stop"

function Test-VsWhere {
    $candidate = "$Env:ProgramFiles (x86)\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $candidate) { return $true }
    $candidate = "$Env:ProgramFiles\Microsoft Visual Studio\Installer\vswhere.exe"
    return (Test-Path $candidate)
}

if (-not (Test-VsWhere)) {
    Write-Host "::error::Visual Studio Installer / vswhere.exe not found." -ForegroundColor Red
    Write-Host "Install: VS Build Tools 2022 with 'Desktop development with C++' workload." -ForegroundColor Yellow
    exit 2
}

Push-Location (Resolve-Path "$PSScriptRoot/..")
try {
    cargo build -p spanzip-ffi --release
    if ($LASTEXITCODE -ne 0) {
        Write-Host "::error::Rust release build failed." -ForegroundColor Red
        exit 1
    }

    $publishArgs = @(
        "publish",
        "app/SpanZIP.App/SpanZIP.App.csproj",
        "-c", "Release",
        "-r", "win-x64",
        "/p:Platform=x64",
        "/p:WarningsAsErrors=false"
    )
    if ($Quick) {
        $publishArgs += "/p:PublishAot=true"
        $publishArgs += "--no-restore"
    }
    & dotnet @publishArgs
    if ($LASTEXITCODE -ne 0) {
        Write-Host "::error::AOT publish failed (ILC). See errors above." -ForegroundColor Red
        exit 1
    }
    Write-Host "AOT publish OK." -ForegroundColor Green
}
finally {
    Pop-Location
}
