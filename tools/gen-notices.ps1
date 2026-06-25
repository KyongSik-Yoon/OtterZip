# Regenerate THIRD-PARTY-NOTICES.md from the shipped Rust core dependency
# graph (otterzip-ffi.dll → otterzip-core → all bundled OSS crates).
# Run before each release. Requires: cargo install cargo-about --features cli
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$about = Join-Path $env:USERPROFILE '.cargo\bin\cargo-about.exe'
if (-not (Test-Path $about)) {
    throw "cargo-about not found. Run: cargo install cargo-about --locked --features cli"
}
Push-Location $root
try {
    & $about generate `
        --manifest-path 'crates\otterzip-ffi\Cargo.toml' `
        'tools\about.hbs' -o 'THIRD-PARTY-NOTICES.md'
    if ($LASTEXITCODE -ne 0) { throw "cargo-about generate failed ($LASTEXITCODE)" }
    Write-Output "THIRD-PARTY-NOTICES.md regenerated."
} finally {
    Pop-Location
}
