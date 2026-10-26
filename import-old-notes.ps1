<#
.SYNOPSIS
  Merge an AmazonOrders archive (from the old machine) into the live notes folder.
  Run from the project folder: .\import-old-notes.ps1 [-Zip .\AmazonOrders-old.zip]

  - Adds notes and images that do not exist yet; never overwrites an existing note.
  - Merges .price-history.json (existing entries win).
  - Skips .headless-sync.json / .gemini-usage.json / .daemon.lock (stale per-machine state).
  - Stops the server first and tells you to restart it from the tray afterwards.
#>
[CmdletBinding()]
param(
    [string]$Zip = (Join-Path $PSScriptRoot 'AmazonOrders-old.zip')
)
$ErrorActionPreference = 'Stop'
$Config = Join-Path $env:APPDATA 'amazon-order-sync\config.json'
if (-not (Test-Path $Zip)) { throw "Archive not found: $Zip" }
$cfg = Get-Content $Config -Raw -Encoding UTF8 | ConvertFrom-Json
$port = if ($cfg.port) { [int]$cfg.port } else { 27483 }
$notes = $cfg.notesPath -replace '^~', $env:USERPROFILE
Write-Host "Notes folder: $notes"

# Stop the server so it does not scan a half-copied tree.
try { Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$port/api/shutdown" -TimeoutSec 3 | Out-Null; Start-Sleep -Seconds 2 } catch { }

$tmp = Join-Path $env:TEMP ("aos-import-" + [guid]::NewGuid().ToString('N'))
Expand-Archive -Path $Zip -DestinationPath $tmp -Force
$src = Get-ChildItem -Path $tmp -Directory -Recurse | Where-Object { Test-Path (Join-Path $_.FullName 'Orders') } | Select-Object -First 1
if (-not $src) { throw "No AmazonOrders\Orders folder inside the archive." }
$srcOrders = Join-Path $src.FullName 'Orders'
$dstOrders = Join-Path $notes 'Orders'
New-Item -ItemType Directory -Force -Path $dstOrders | Out-Null

$added = 0; $skipped = 0
Get-ChildItem -Path $srcOrders -Recurse -File | ForEach-Object {
    $rel = $_.FullName.Substring($srcOrders.Length).TrimStart('\')
    $dst = Join-Path $dstOrders $rel
    if (Test-Path $dst) { $skipped++ }
    else {
        New-Item -ItemType Directory -Force -Path (Split-Path $dst) | Out-Null
        Copy-Item $_.FullName $dst
        $added++
    }
}

# Merge price history: keep whatever the live file already has.
$srcPh = Join-Path $src.FullName '.price-history.json'
$dstPh = Join-Path $notes '.price-history.json'
$merged = 0
if (Test-Path $srcPh) {
    $old = Get-Content $srcPh -Raw -Encoding UTF8 | ConvertFrom-Json
    $live = if (Test-Path $dstPh) { Get-Content $dstPh -Raw -Encoding UTF8 | ConvertFrom-Json } else { [pscustomobject]@{} }
    foreach ($p in $old.PSObject.Properties) {
        if (-not $live.PSObject.Properties[$p.Name]) { $live | Add-Member -NotePropertyName $p.Name -NotePropertyValue $p.Value; $merged++ }
    }
    [System.IO.File]::WriteAllText($dstPh, ($live | ConvertTo-Json -Depth 10), (New-Object System.Text.UTF8Encoding $false))
}
Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "Imported: $added files added, $skipped already present, $merged price-history entries merged." -ForegroundColor Green
Write-Host "Now right-click the tray icon -> Restart server."
