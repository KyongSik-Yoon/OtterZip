# Sprint 6 — One-shot dev install: trust the cert, sign the MSIX, install it.
#
# `dev-cert.ps1`이 만든 자기서명 인증서는 CurrentUser\TrustedPeople 만으로는
# Add-AppxPackage가 통과되지 않는다 (0x800B0109). MSIX sideload는 LocalMachine
# 의 Root + TrustedPeople 양쪽에 인증서가 있어야 한다 — 따라서 관리자 권한이 필요.
#
# 사용법 (관리자 PowerShell):
#   pwsh tools/install-msix.ps1
#   pwsh tools/install-msix.ps1 -Msix D:\path\to\OtterZip.msix
#
# 필수 사전: tools/dev-cert.ps1 으로 build/dev-cert.pfx + .cer 생성.

[CmdletBinding()]
param(
    [string]$Msix,
    [string]$Pfx,
    [string]$Cer,
    [string]$Password = 'otterzip-dev-only'
)

$ErrorActionPreference = 'Stop'

# `$PSScriptRoot` is empty when the script is invoked in some hosts (e.g.
# `powershell -File ... ` from another script). Resolve from MyInvocation
# as a fallback so the default cert / msix paths stay stable.
if (-not $PSScriptRoot -or $PSScriptRoot.Length -eq 0) {
    $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
} else {
    $scriptDir = $PSScriptRoot
}
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $scriptDir '..'))

if (-not $Pfx) { $Pfx = Join-Path $repoRoot 'build\dev-cert.pfx' }
if (-not $Cer) { $Cer = Join-Path $repoRoot 'build\dev-cert.cer' }

# Resolve a default MSIX if none was given — newest under AppPackages\VER_*\
if (-not $Msix) {
    $appPackages = Join-Path $repoRoot 'AppPackages'
    $candidate = Get-ChildItem -Path $appPackages -Recurse -Filter '*.msix' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    if (-not $candidate) {
        throw "No .msix found under $appPackages. Run build-msix.bat first."
    }
    $Msix = $candidate.FullName
}

if (-not (Test-Path $Pfx))  { throw "PFX not found: $Pfx — run tools\dev-cert.ps1 first." }
if (-not (Test-Path $Msix)) { throw "MSIX not found: $Msix" }

# The cert we trust below MUST be the one the PFX signs with further down.
# Deriving it from the PFX is the only way to guarantee that. A separate
# build\dev-cert.cer used to be the default here and silently went stale
# (CN=SpanZIP) once the pfx was reissued as CN=A55D3041-... : trusting it
# authorised nothing, so a fresh machine's install failed with 0x800B0109
# while this one worked off the already-trusted cert. -Cer still overrides,
# but is verified against the PFX rather than taken on faith.
$pfxPublic = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2(
    $Pfx, $Password, [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::DefaultKeySet)
if ($Cer) {
    if (-not (Test-Path $Cer)) { throw "CER not found: $Cer" }
    $given = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2($Cer)
    if ($given.Thumbprint -ne $pfxPublic.Thumbprint) {
        throw ("-Cer does not match the signing PFX and would trust the wrong certificate.`n" +
               "  cer: $($given.Subject) [$($given.Thumbprint)]`n" +
               "  pfx: $($pfxPublic.Subject) [$($pfxPublic.Thumbprint)]")
    }
} else {
    $Cer = Join-Path ([System.IO.Path]::GetTempPath()) ("otterzip-signer-" + $pfxPublic.Thumbprint + ".cer")
    [IO.File]::WriteAllBytes($Cer, $pfxPublic.Export('Cert'))
}

# 0. Admin check — adding to LocalMachine stores is privileged.
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "This script must be run as Administrator (need to write to LocalMachine cert stores)."
}

Write-Host "MSIX:  $Msix"
Write-Host "PFX:   $Pfx"
Write-Host "CER:   $Cer"
Write-Host "SIGNER: $($pfxPublic.Subject) [$($pfxPublic.Thumbprint)]"
Write-Host ""

# 1. Trust the public cert — both Root (chain authority) and TrustedPeople
#    (MSIX sideload requires it). Idempotent.
Write-Host '[1/3] Trusting cert in LocalMachine\Root + LocalMachine\TrustedPeople...'
& certutil -addstore -f Root          $Cer | Out-Null
& certutil -addstore -f TrustedPeople $Cer | Out-Null
Write-Host '[OK] Cert trusted.'
Write-Host ''

# 2. Sign the MSIX with the PFX. Idempotent — re-signing overwrites the
#    existing signature in-place.
Write-Host '[2/3] Signing MSIX with self-signed cert...'
$signtool = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin' -Recurse -Filter 'signtool.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\x64\\' } |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1
if (-not $signtool) {
    throw "signtool.exe not found. Install Windows 10/11 SDK."
}
& $signtool.FullName sign /fd SHA256 /a /f $Pfx /p $Password $Msix
if ($LASTEXITCODE -ne 0) {
    throw "signtool exited with $LASTEXITCODE."
}
Write-Host '[OK] MSIX signed.'
Write-Host ''

# 3. Install. If a previous install exists we replace it.
Write-Host '[3/3] Installing MSIX...'
$existing = Get-AppxPackage -Name 'OtterZip*' -ErrorAction SilentlyContinue
if ($existing) {
    Write-Host "  Removing previous install: $($existing.PackageFullName)"
    Remove-AppxPackage -Package $existing.PackageFullName
}
Add-AppxPackage -Path $Msix
Write-Host '[OK] MSIX installed.'
Write-Host ''

Get-AppxPackage OtterZip* | Format-List Name, Version, PackageFullName, InstallLocation
Write-Host '=== Done. Right-click any ZIP in Explorer to test. ===' -ForegroundColor Green
