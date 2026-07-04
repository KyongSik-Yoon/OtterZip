# Phase 9 — version bump helper.
#
# Single source of truth: `Package.appxmanifest` Identity.Version.
# Mirrors into:
#   - Cargo.toml (workspace.package.version)
#   - Directory.Build.props (<Version>)
#   - docs/CHANGELOG-ffi.md (header anchor) — left manual
#
# Usage:
#   pwsh tools/bump-version.ps1 -Version 0.2.0
#   pwsh tools/bump-version.ps1 -Patch    # 0.1.0 -> 0.1.1
#   pwsh tools/bump-version.ps1 -Minor    # 0.1.0 -> 0.2.0
#   pwsh tools/bump-version.ps1 -Major    # 0.1.0 -> 1.0.0

[CmdletBinding(DefaultParameterSetName = 'Explicit')]
param(
    [Parameter(ParameterSetName = 'Explicit')]
    [string]$Version,
    [Parameter(ParameterSetName = 'Patch')] [switch]$Patch,
    [Parameter(ParameterSetName = 'Minor')] [switch]$Minor,
    [Parameter(ParameterSetName = 'Major')] [switch]$Major
)

$ErrorActionPreference = 'Stop'

# --- UTF-8-safe file I/O ----------------------------------------------------
# Windows PowerShell 5.1's `Get-Content -Raw` decodes with the ANSI codepage
# (CP949 on a Korean box), which silently corrupts any non-ASCII byte — e.g.
# the © in Directory.Build.props became "짤" on every previous bump. Read and
# write through .NET so UTF-8 is honoured, preserving each file's BOM state.
function Read-Utf8([string]$path) {
    return [System.IO.File]::ReadAllText($path)   # .NET: BOM-aware, UTF-8 default
}
function Write-Utf8([string]$path, [string]$text) {
    $bytes = [System.IO.File]::ReadAllBytes($path)
    $hasBom = ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF)
    $enc = New-Object System.Text.UTF8Encoding($hasBom)
    [System.IO.File]::WriteAllText($path, $text, $enc)
}

if (-not $PSScriptRoot -or $PSScriptRoot.Length -eq 0) {
    $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
} else {
    $scriptDir = $PSScriptRoot
}
$repo = [System.IO.Path]::GetFullPath((Join-Path $scriptDir '..'))

$manifest = Join-Path $repo 'app\OtterZip.App\Package.appxmanifest'
$cargo    = Join-Path $repo 'Cargo.toml'
$dirProps = Join-Path $repo 'Directory.Build.props'

# 1. Read current version from manifest.
$xml = [xml](Read-Utf8 $manifest)
$current = $xml.Package.Identity.Version       # e.g. 0.1.0.0
$parts = $current.Split('.')
# NOTE: these MUST NOT be named $major/$minor/$patch — PowerShell variable
# names are case-INSENSITIVE, so $major would be the SAME variable as the
# [switch]$Major parameter. Assigning the parsed component to it silently
# overwrites the switch (e.g. `$major = 1` makes `-Major` read as true),
# which made `-Patch` on 1.0.0 compute 2.0.0. Keep them distinct.
[int]$curMajor = $parts[0]
[int]$curMinor = $parts[1]
[int]$curPatch = $parts[2]

# 2. Compute target.
if ($Version) {
    if ($Version -notmatch '^\d+\.\d+\.\d+$') {
        throw "Version must be MAJOR.MINOR.PATCH, got '$Version'"
    }
    $target = $Version
} elseif ($Patch) { $target = "$curMajor.$curMinor.$($curPatch + 1)" }
elseif ($Minor)   { $target = "$curMajor.$($curMinor + 1).0" }
elseif ($Major)   { $target = "$($curMajor + 1).0.0" }
else {
    throw "Pick one of: -Version <ver> | -Patch | -Minor | -Major"
}
$semver4 = "$target.0"
Write-Host "Bumping $current -> $semver4 (semver: $target)" -ForegroundColor Cyan

# 3. Manifest (4-segment). Regex substitution preserves the original
#    formatting, comments, and BOM — `xml.Save` would re-emit the document
#    on a single line and re-encode it.
$manifestText = Read-Utf8 $manifest
$manifestText = [regex]::Replace(
    $manifestText,
    'Identity[^>]*?Version\s*=\s*"\d+\.\d+\.\d+\.\d+"',
    { param($m) [regex]::Replace($m.Value, '"\d+\.\d+\.\d+\.\d+"', "`"$semver4`"") })
Write-Utf8 $manifest $manifestText
Write-Host "  [OK] Package.appxmanifest"

# 4. Cargo workspace package version.
$cargoText = Read-Utf8 $cargo
$cargoText = [regex]::Replace(
    $cargoText,
    '(?m)^(version\s*=\s*)"\d+\.\d+\.\d+"',
    "`$1`"$target`"")
Write-Utf8 $cargo $cargoText
Write-Host "  [OK] Cargo.toml"

# 5. Directory.Build.props.
$propsText = Read-Utf8 $dirProps
$propsText = [regex]::Replace(
    $propsText,
    '(?m)<Version>\d+\.\d+\.\d+</Version>',
    "<Version>$target</Version>")
Write-Utf8 $dirProps $propsText
Write-Host "  [OK] Directory.Build.props"

Write-Host ""
Write-Host "Next steps:" -ForegroundColor Yellow
Write-Host "  1. Update docs/CHANGELOG-ffi.md if ABI changed."
Write-Host "  2. cargo build --workspace; cargo test --workspace"
Write-Host "  3. .\build-msix.bat"
Write-Host "  4. (admin) pwsh tools\install-msix.ps1   # local sideload smoke test"
