<#
.SYNOPSIS
  Amazon Order Sync - Windows installer / upgrader.

.DESCRIPTION
  Run from the folder the release zip was expanded into (the one containing
  AmazonOrderSync.exe and browser-extension\). AmazonOrderSync.exe is the
  whole app, the tray and the server in one program, so nothing else is
  needed and nothing is downloaded. It:
    1. Creates %APPDATA%\amazon-order-sync\config.json on first run
       (never overwrites an existing config).
    2. Registers the tray app to start with Windows (HKCU Run key).
    3. Starts the tray app and waits for the server to answer.

  Idempotent - rerun after expanding a newer release over the same folder. Use -Uninstall to remove
  the Run entry and stop the tray (notes, config and browser profile are kept).

.PARAMETER NotesPath
  Where the Markdown notes go. Only used when creating a NEW config.

.PARAMETER NoAutostart
  Skip the start-with-Windows registration.
#>
[CmdletBinding()]
param(
    [string]$NotesPath = (Join-Path $env:USERPROFILE 'AmazonOrders'),
    [switch]$NoAutostart,
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'
$AppName   = 'amazon-order-sync'
$Root      = $PSScriptRoot
$Exe       = Join-Path $Root 'AmazonOrderSync.exe'
$ConfigDir = Join-Path $env:APPDATA $AppName
$Config    = Join-Path $ConfigDir 'config.json'
$RunKey    = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$RunValue  = 'AmazonOrderSync'

function Get-ConfiguredPort {
    if (Test-Path $Config) {
        $c = Get-Content $Config -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($c.port) { return [int]$c.port }
    }
    return 27483
}

function Stop-Tray {
    # Always ask a running server to shut down, tray or not: an orphan left by
    # a crashed tray would otherwise be adopted and keep serving old code.
    try {
        Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$(Get-ConfiguredPort)/api/shutdown" -TimeoutSec 3 | Out-Null
        Write-Host "Asked the running server to stop."
        Start-Sleep -Seconds 1
    } catch { }
    $procs = Get-Process -Name 'AmazonOrderSync' -ErrorAction SilentlyContinue
    if ($procs) {
        Write-Host "Stopping running tray app..."
        $procs | Stop-Process -Force -ErrorAction SilentlyContinue
        # Wait until the processes are really gone (file handles released).
        foreach ($p in $procs) { try { $p.WaitForExit(10000) | Out-Null } catch { } }
        Start-Sleep -Milliseconds 500
    }
}

# Open a file for writing, retrying while a dying process still holds it.
function Open-ForWrite([string]$Path) {
    for ($i = 0; $i -lt 20; $i++) {
        try { return [System.IO.File]::Create($Path) }
        catch [System.IO.IOException] { Start-Sleep -Milliseconds 500 }
    }
    throw "Could not write $Path - is AmazonOrderSync.exe still running? Quit it from the tray and rerun."
}

if ($Uninstall) {
    Stop-Tray
    Remove-ItemProperty -Path $RunKey -Name $RunValue -ErrorAction SilentlyContinue
    Write-Host "Removed the start-with-Windows entry."
    Write-Host "Left untouched: $Config, your notes, and $env:LOCALAPPDATA\$AppName (browser login profile, logs)."
    Write-Host "Delete this folder ($Root) yourself if you want the app gone."
    exit 0
}

# --- 0. sanity ---------------------------------------------------------------
foreach ($p in @($Exe, (Join-Path $Root 'browser-extension\manifest.json'))) {
    if (-not (Test-Path $p)) { throw "Missing $p - run install.ps1 from the folder the release zip was expanded into." }
}
# 2.3.0 and older shipped node\ and server\ next to the exe; nothing uses them now.
foreach ($old in @('node\node.exe', 'server\server.js')) {
    $leftover = Join-Path $Root $old
    if (Test-Path $leftover) {
        Write-Host "Note: $(Split-Path $leftover) is left over from an older version and is no longer used; you can delete it."
    }
}

# --- 1. config -----------------------------------------------------------------
if (-not (Test-Path $Config)) {
    New-Item -ItemType Directory -Force -Path $ConfigDir | Out-Null
    $NotesPath = [System.IO.Path]::GetFullPath($NotesPath)
    @{
        notesPath             = $NotesPath
        geminiApiKey          = ''
        port                  = 27483
        syncReminderDays      = 7
        autoSyncEnabled       = $false
        autoSyncIntervalHours = 24
        browserPath           = ''
        notificationsEnabled  = $true
    } | ConvertTo-Json | ForEach-Object { [System.IO.File]::WriteAllText($Config, $_ + "`r`n", (New-Object System.Text.UTF8Encoding $false)) }
    Write-Host "Created $Config (notes -> $NotesPath)"
} else {
    Write-Host "Config exists: $Config (left as is)"
}

# --- 2. autostart --------------------------------------------------------------
if (-not $NoAutostart) {
    # Never New-Item -Force here: on an existing registry key that recreates
    # it and deletes every other startup entry. The Run key always exists.
    if (-not (Test-Path $RunKey)) { New-Item -Path $RunKey | Out-Null }
    Set-ItemProperty -Path $RunKey -Name $RunValue -Value "`"$Exe`" --autostart"
    Write-Host "Registered to start with Windows."
}

# --- 2b. desktop shortcut (created by Windows itself, so it is always valid) ----
try {
    $desktop = [Environment]::GetFolderPath('Desktop')
    $lnk = Join-Path $desktop 'Amazon Order Sync.lnk'
    $wsh = New-Object -ComObject WScript.Shell
    $sc = $wsh.CreateShortcut($lnk)
    $sc.TargetPath = $Exe
    $sc.WorkingDirectory = $Root
    $sc.IconLocation = "$Exe,0"
    $sc.Description = 'Amazon Order Sync tray app'
    $sc.Save()
    Write-Host "Desktop shortcut: $lnk"
} catch { Write-Host "Could not create the desktop shortcut: $($_.Exception.Message)" -ForegroundColor Yellow }

# --- 3. (re)start the tray ------------------------------------------------------
Stop-Tray
Write-Host "Starting tray app..."
Start-Process -FilePath $Exe -WorkingDirectory $Root

$port = Get-ConfiguredPort
$ok = $false
for ($i = 0; $i -lt 30; $i++) {
    try {
        $h = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/health" -TimeoutSec 2
        if ($h.ok) { $ok = $true; break }
    } catch { Start-Sleep -Milliseconds 500 }
}

Write-Host ""
if ($ok) {
    Write-Host "Server status: running (v$($h.version))" -ForegroundColor Green
    Write-Host "Dashboard:     http://127.0.0.1:$port/"
    Write-Host "Notes path:    $($h.notesPath)"
    Write-Host "Browser found: $($h.browserFound)"
} else {
    Write-Host "Server did not answer within 15 s. Check $env:LOCALAPPDATA\$AppName\server.log and tray.log." -ForegroundColor Yellow
}
Write-Host "Config:        $Config"
Write-Host ""
Write-Host "Next: load the extension in Brave -> brave://extensions -> Developer mode -> Load unpacked ->"
Write-Host "      $(Join-Path $Root 'browser-extension')"
