# Sprint 6 RC — Bench corpus downloader.
#
# Downloads & stages the standard performance corpora into build\corpora\
# so cargo bench can pick them up via SPANZIP_BENCH_CORPUS. Idempotent:
# already-downloaded files are skipped.
#
# Usage (no admin required):
#   pwsh tools/fetch-corpora.ps1                # all default corpora
#   pwsh tools/fetch-corpora.ps1 -Only silesia
#   pwsh tools/fetch-corpora.ps1 -Force         # re-download
#
# Disk: ~75 MB silesia, ~322 MB enwik9, ~3 MB canterbury. Defaults to
# silesia + canterbury only — enwik9 is opt-in because of the size.

[CmdletBinding()]
param(
    [string[]]$Only = @('silesia', 'canterbury'),
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'  # speed up Invoke-WebRequest

if (-not $PSScriptRoot -or $PSScriptRoot.Length -eq 0) {
    $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
} else {
    $scriptDir = $PSScriptRoot
}
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $scriptDir '..'))
$dest = Join-Path $repoRoot 'build\corpora'
New-Item -ItemType Directory -Force -Path $dest | Out-Null

# Source list. URLs validated 2026-04-27 — if any 404, update here.
$Corpora = @(
    @{
        Name      = 'silesia'
        Url       = 'http://mattmahoney.net/dc/silesia.zip'
        FileName  = 'silesia.zip'
        ApproxMB  = 67
        UseAsZip  = $true
    },
    @{
        Name      = 'canterbury'
        Url       = 'http://corpus.canterbury.ac.nz/resources/cantrbry.zip'
        FileName  = 'canterbury.zip'
        ApproxMB  = 1
        UseAsZip  = $true
    },
    @{
        Name      = 'enwik9'
        Url       = 'https://mattmahoney.net/dc/enwik9.zip'
        FileName  = 'enwik9.zip'
        ApproxMB  = 322
        UseAsZip  = $true
    }
)

$selected = $Corpora | Where-Object { $Only -contains $_.Name }
if (-not $selected) {
    Write-Host "No corpora matched -Only $($Only -join ',')." -ForegroundColor Yellow
    Write-Host "Available: $($Corpora.Name -join ', ')"
    exit 1
}

foreach ($c in $selected) {
    $out = Join-Path $dest $c.FileName
    if ((Test-Path $out) -and -not $Force) {
        $size = (Get-Item $out).Length / 1MB
        Write-Host ("[SKIP] {0,-12} already at {1} ({2:N1} MB)" -f $c.Name, $out, $size) -ForegroundColor DarkGray
        continue
    }
    Write-Host ("[FETCH] {0,-12} ~{1} MB from {2}" -f $c.Name, $c.ApproxMB, $c.Url)
    try {
        Invoke-WebRequest -Uri $c.Url -OutFile $out -UseBasicParsing -TimeoutSec 600
        $size = (Get-Item $out).Length / 1MB
        Write-Host ("  -> {0:N1} MB saved" -f $size) -ForegroundColor Green
    } catch {
        Write-Host "[ERROR] $($c.Name): $($_.Exception.Message)" -ForegroundColor Red
        Remove-Item -Force $out -ErrorAction SilentlyContinue
    }
}

Write-Host ""
Write-Host "=== Available corpora in $dest ==="
Get-ChildItem $dest -Filter '*.zip' | ForEach-Object {
    Write-Host ("  {0,-25} {1,8:N1} MB" -f $_.Name, ($_.Length / 1MB))
}
Write-Host ""
Write-Host "Run benchmarks against one with:" -ForegroundColor Cyan
Write-Host "  `$Env:SPANZIP_BENCH_CORPUS = '$($dest -replace '\\','\\')\silesia.zip'"
Write-Host "  cargo bench -p spanzip-core --bench extract -- --warm-up-time 2 --measurement-time 10"
