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

if (-not $PSScriptRoot -or $PSScriptRoot.Length -eq 0) {
    $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
} else {
    $scriptDir = $PSScriptRoot
}
$repo = [System.IO.Path]::GetFullPath((Join-Path $scriptDir '..'))

$manifest = Join-Path $repo 'app\SpanZIP.App\Package.appxmanifest'
$cargo    = Join-Path $repo 'Cargo.toml'
$dirProps = Join-Path $repo 'Directory.Build.props'

# 1. Read current version from manifest.
$xml = [xml](Get-Content $manifest)
$current = $xml.Package.Identity.Version       # e.g. 0.1.0.0
$parts = $current.Split('.')
[int]$major = $parts[0]
[int]$minor = $parts[1]
[int]$patch = $parts[2]

# 2. Compute target.
if ($Version) {
    if ($Version -notmatch '^\d+\.\d+\.\d+$') {
        throw "Version must be MAJOR.MINOR.PATCH, got '$Version'"
    }
    $target = $Version
} elseif ($Patch) { $target = "$major.$minor.$($patch + 1)" }
elseif ($Minor)   { $target = "$major.$($minor + 1).0" }
elseif ($Major)   { $target = "$($major + 1).0.0" }
else {
    throw "Pick one of: -Version <ver> | -Patch | -Minor | -Major"
}
$semver4 = "$target.0"
Write-Host "Bumping $current -> $semver4 (semver: $target)" -ForegroundColor Cyan

# 3. Manifest (4-segment). Regex substitution preserves the original
#    formatting, comments, and any BOM — `xml.Save` would re-emit the
#    document on a single line and re-encode it.
$manifestText = Get-Content $manifest -Raw
$manifestText = [regex]::Replace(
    $manifestText,
    'Identity[^>]*?Version\s*=\s*"\d+\.\d+\.\d+\.\d+"',
    { param($m) [regex]::Replace($m.Value, '"\d+\.\d+\.\d+\.\d+"', "`"$semver4`"") })
Set-Content -Path $manifest -Value $manifestText -Encoding UTF8 -NoNewline
Write-Host "  [OK] Package.appxmanifest"

# 4. Cargo workspace package version.
$cargoText = Get-Content $cargo -Raw
$cargoText = [regex]::Replace(
    $cargoText,
    '(?m)^(version\s*=\s*)"\d+\.\d+\.\d+"',
    "`$1`"$target`"")
Set-Content -Path $cargo -Value $cargoText -Encoding UTF8 -NoNewline
Write-Host "  [OK] Cargo.toml"

# 5. Directory.Build.props.
$propsText = Get-Content $dirProps -Raw
$propsText = [regex]::Replace(
    $propsText,
    '(?m)<Version>\d+\.\d+\.\d+</Version>',
    "<Version>$target</Version>")
Set-Content -Path $dirProps -Value $propsText -Encoding UTF8 -NoNewline
Write-Host "  [OK] Directory.Build.props"

Write-Host ""
Write-Host "Next steps:" -ForegroundColor Yellow
Write-Host "  1. Update docs/CHANGELOG-ffi.md if ABI changed."
Write-Host "  2. cargo build --workspace; cargo test --workspace"
Write-Host "  3. .\build-msix.bat"
Write-Host "  4. (admin) pwsh tools\install-msix.ps1   # local sideload smoke test"
