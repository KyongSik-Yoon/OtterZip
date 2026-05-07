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
    [string]$Subject = "CN=OtterZip",
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
if (-not $ExportPath) {
    $ExportPath = Join-Path (Split-Path -Parent $scriptDir) 'build/dev-cert.pfx'
}
$ExportPath = [System.IO.Path]::GetFullPath($ExportPath)

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

# Trust the cert locally so MSIX install doesn't require admin elevation
# beyond first-run.
$trustStore = "Cert:\CurrentUser\TrustedPeople"
if (-not (Get-ChildItem $trustStore | Where-Object { $_.Thumbprint -eq $cert.Thumbprint })) {
    $store = New-Object System.Security.Cryptography.X509Certificates.X509Store("TrustedPeople", "CurrentUser")
    $store.Open("ReadWrite")
    $store.Add($cert)
    $store.Close()
    Write-Host "Trusted in CurrentUser\TrustedPeople." -ForegroundColor Green
}

Write-Host ""
Write-Host "Thumbprint: $($cert.Thumbprint)"
Write-Host "PFX path:   $ExportPath"
Write-Host "Use this PFX with: signtool sign /fd SHA256 /a /f `"$ExportPath`" /p `"$Password`" OtterZip.msix"
