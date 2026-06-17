# Sprint 6 RC — Developer self-signed certificate for local MSIX testing.
#
# The MSIX manifest declares Publisher="CN=OtterZip". Local installs need a
# matching cert in the Trusted People store. Run once per machine:
#
#   pwsh tools/dev-cert.ps1
#
# Refresh annually (default validity: 12 months) — mirrors the entry in
# docs/05-build/phase-6-plan.md §9.

[CmdletBinding()]
param(
    # Defaults to "auto" — read Publisher from Package.appxmanifest. Override
    # with an explicit `-Subject "CN=…"` only if you specifically need a
    # mismatched cert for a one-off experiment.
    [string]$Subject = "auto",
    [int]$ValidMonths = 12,
    [string]$ExportPath,
    [string]$Password = "otterzip-dev-only"
)

$ErrorActionPreference = "Stop"

# `$PSScriptRoot` is empty when the script is dot-sourced or invoked in a
# way that doesn't set the implicit variable. Resolve from the script's
# own MyInvocation as a fallback so the default export path is stable.
if (-not $PSScriptRoot -or $PSScriptRoot.Length -eq 0) {
    $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
} else {
    $scriptDir = $PSScriptRoot
}
$repoRoot = Split-Path -Parent $scriptDir
if (-not $ExportPath) {
    $ExportPath = Join-Path $repoRoot 'build/dev-cert.pfx'
}
$ExportPath = [System.IO.Path]::GetFullPath($ExportPath)

# Auto-derive the cert Subject from the MSIX manifest's Publisher field.
# signtool refuses to sign an MSIX whose manifest Publisher does not match
# the cert Subject byte-for-byte. Hard-coding "CN=OtterZip" here worked
# while the manifest also used CN=OtterZip; once it switched to the
# Partner Center publisher ID (CN=A55D3041-...), every dev-build sign
# attempt failed with SignerSign() 0x8007000b. Reading the manifest
# avoids that drift.
if ($Subject -eq 'auto') {
    $manifestPath = Join-Path $repoRoot 'app/OtterZip.App/Package.appxmanifest'
    if (-not (Test-Path -LiteralPath $manifestPath)) {
        throw "Auto Subject requested but manifest not found at $manifestPath"
    }
    [xml]$manifest = Get-Content -LiteralPath $manifestPath -Raw
    $publisher = $manifest.Package.Identity.Publisher
    if (-not $publisher) {
        throw "Could not read Identity/@Publisher from $manifestPath"
    }
    $Subject = $publisher
    Write-Host "Auto Subject = $Subject (from Package.appxmanifest)" -ForegroundColor DarkCyan
}

# Idempotent: if a non-expired cert with the same subject exists, reuse it.
$existing = Get-ChildItem Cert:\CurrentUser\My |
    Where-Object { $_.Subject -eq $Subject -and $_.NotAfter -gt (Get-Date).AddDays(30) } |
    Sort-Object NotAfter -Descending |
    Select-Object -First 1

if ($existing) {
    Write-Host "Reusing existing certificate: $($existing.Thumbprint)" -ForegroundColor Green
    $cert = $existing
}
else {
    Write-Host "Creating new self-signed cert: $Subject (valid $ValidMonths months)" -ForegroundColor Cyan
    $cert = New-SelfSignedCertificate `
        -Type CodeSigningCert `
        -Subject $Subject `
        -KeyUsage DigitalSignature `
        -FriendlyName "OtterZip Dev Cert" `
        -CertStoreLocation "Cert:\CurrentUser\My" `
        -NotAfter (Get-Date).AddMonths($ValidMonths) `
        -TextExtension @(
            "2.5.29.37={text}1.3.6.1.5.5.7.3.3",
            "2.5.29.19={text}"
        )
}

# Export PFX for `signtool` consumption + place into Trusted People so
# `Add-AppxPackage` will accept the resulting MSIX.
New-Item -ItemType Directory -Force -Path (Split-Path $ExportPath) | Out-Null
$secure = ConvertTo-SecureString -String $Password -Force -AsPlainText
Export-PfxCertificate -Cert $cert -FilePath $ExportPath -Password $secure | Out-Null

# Trust the cert across the four stores MSIX sideload policy needs.
#
# Diagnosed 2026-05-19: writing only to CurrentUser\TrustedPeople left
# v0.21 silently deregistered post-install with 0x80073CFF
# (ERROR_INSTALL_POLICY_FAILURE) — the AppX deployment service walks
# the cert chain to a *machine-wide* root and refused our publisher.
# Adding LocalMachine\Root closes that chain. We also keep the cert in
# LocalMachine\TrustedPeople (where packaged-COM expects sideloaded
# verbs) and CurrentUser\TrustedPeople (legacy compat for non-MSIX
# self-install flows).
#
# LocalMachine\* stores require admin. The script auto-elevates a child
# PowerShell process for just the cert-import step, then returns —
# avoids running the whole script as admin (which would create files
# owned by the elevated user).
function Add-CertToStore {
    param(
        [System.Security.Cryptography.X509Certificates.X509Certificate2]$Cert,
        [string]$StoreName,
        [string]$Scope  # 'CurrentUser' or 'LocalMachine'
    )
    $storePath = "Cert:\$Scope\$StoreName"
    $hit = Get-ChildItem $storePath -ErrorAction SilentlyContinue |
        Where-Object { $_.Thumbprint -eq $Cert.Thumbprint }
    if ($hit) {
        Write-Host "  already in $storePath" -ForegroundColor DarkGray
        return $true
    }
    try {
        $store = New-Object System.Security.Cryptography.X509Certificates.X509Store($StoreName, $Scope)
        $store.Open("ReadWrite")
        $store.Add($Cert)
        $store.Close()
        Write-Host "  added to $storePath" -ForegroundColor Green
        return $true
    } catch {
        Write-Host "  FAILED to add to $storePath : $($_.Exception.Message)" -ForegroundColor Yellow
        return $false
    }
}

function Test-IsAdmin {
    $id  = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $prc = New-Object System.Security.Principal.WindowsPrincipal($id)
    return $prc.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
}

Write-Host ""
Write-Host "Trust setup — CurrentUser scope (no admin needed):" -ForegroundColor Cyan
Add-CertToStore -Cert $cert -StoreName 'TrustedPeople' -Scope 'CurrentUser' | Out-Null

# LocalMachine writes need admin. Check if we already have it; if not,
# re-launch an elevated child that just imports the PFX into the two
# LocalMachine stores. The parent keeps running and reports the result.
$needsLocalMachine = -not (
    (Get-ChildItem 'Cert:\LocalMachine\Root' -ErrorAction SilentlyContinue |
        Where-Object { $_.Thumbprint -eq $cert.Thumbprint }) -and
    (Get-ChildItem 'Cert:\LocalMachine\TrustedPeople' -ErrorAction SilentlyContinue |
        Where-Object { $_.Thumbprint -eq $cert.Thumbprint })
)

if ($needsLocalMachine) {
    Write-Host ""
    Write-Host "Trust setup — LocalMachine scope (admin required):" -ForegroundColor Cyan
    if (Test-IsAdmin) {
        Add-CertToStore -Cert $cert -StoreName 'Root'          -Scope 'LocalMachine' | Out-Null
        Add-CertToStore -Cert $cert -StoreName 'TrustedPeople' -Scope 'LocalMachine' | Out-Null
    } else {
        Write-Host "  not elevated — launching admin child to import PFX into Root + TrustedPeople" -ForegroundColor Yellow
        # Build a one-liner that imports the PFX into both stores.
        # `Import-PfxCertificate` requires the PFX path + password — passing
        # the password as plain text through a one-shot child is OK here
        # because the PFX itself is committed to the repo (dev-only cert).
        $importScript = @"
`$ErrorActionPreference = 'Stop'
`$pwd = ConvertTo-SecureString -String '$Password' -Force -AsPlainText
try {
    Import-PfxCertificate -FilePath '$ExportPath' -CertStoreLocation 'Cert:\LocalMachine\Root' -Password `$pwd | Out-Null
    Import-PfxCertificate -FilePath '$ExportPath' -CertStoreLocation 'Cert:\LocalMachine\TrustedPeople' -Password `$pwd | Out-Null
    Write-Host 'Imported into LocalMachine\Root + TrustedPeople' -ForegroundColor Green
} catch {
    Write-Host 'Elevated import failed:' `$_.Exception.Message -ForegroundColor Red
    exit 1
}
"@
        $tmp = Join-Path $env:TEMP "otterzip-cert-import-$([guid]::NewGuid()).ps1"
        Set-Content -LiteralPath $tmp -Value $importScript -Encoding UTF8
        try {
            Start-Process -FilePath 'powershell.exe' `
                -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File',"`"$tmp`"") `
                -Verb RunAs -Wait
        } finally {
            Remove-Item -LiteralPath $tmp -ErrorAction SilentlyContinue
        }
        # Verify the import landed.
        $rootOk = Get-ChildItem 'Cert:\LocalMachine\Root' -ErrorAction SilentlyContinue |
            Where-Object { $_.Thumbprint -eq $cert.Thumbprint }
        if ($rootOk) {
            Write-Host "  verified: LocalMachine\Root has cert" -ForegroundColor Green
        } else {
            Write-Host "  WARNING: LocalMachine\Root still missing cert — UAC declined or import failed" -ForegroundColor Yellow
        }
    }
} else {
    Write-Host "Trust setup — LocalMachine: already present, no admin elevation needed." -ForegroundColor DarkGray
}

Write-Host ""
Write-Host "Thumbprint: $($cert.Thumbprint)"
Write-Host "PFX path:   $ExportPath"
Write-Host "Use this PFX with: signtool sign /fd SHA256 /a /f `"$ExportPath`" /p `"$Password`" OtterZip.msix"
