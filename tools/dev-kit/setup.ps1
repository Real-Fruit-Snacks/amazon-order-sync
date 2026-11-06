<#
.SYNOPSIS
  Set up the amazon-order-sync dev kit. Downloads nothing.

.DESCRIPTION
  Run it from the unzipped kit:
      powershell -ExecutionPolicy Bypass -File offline\setup.ps1
  It:
    1. Checks the toolchains the kit leaves out (the .NET SDK, Node.js and
       Python; Git, PowerShell 7 and ShellCheck are optional) and says what
       is missing.
    2. Loads PSScriptAnalyzer from offline\powershell and checks its version.
    3. Writes global.json so dotnet uses the SDK the kit's packages are for,
       and restores the tray app from offline\nuget only.
    4. Makes .venv and installs the art scripts' wheels from offline\python.
    5. Prints the commands that test and build.
  Safe to rerun. Works in Windows PowerShell 5.1 and PowerShell 7.
  The source of this script is tools\dev-kit\setup.ps1.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
# PowerShell 7.3+: native exit codes are checked by hand below, whatever the caller set.
$PSNativeCommandUseErrorActionPreference = $false
$Offline = $PSScriptRoot
$Kit = Split-Path -Parent $Offline
$Pins = Get-Content (Join-Path $Kit 'tools\versions.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$Missing = New-Object System.Collections.Generic.List[string]

# Nothing below may reach the network, and nothing should try.
$env:DOTNET_CLI_TELEMETRY_OPTOUT = '1'
$env:DOTNET_NOLOGO = '1'
$env:DOTNET_CLI_WORKLOAD_UPDATE_NOTIFY_DISABLE = '1'
$env:PIP_NO_INDEX = '1'
$env:PIP_DISABLE_PIP_VERSION_CHECK = '1'

# Run a program, return its exit code and what it wrote to stdout. stderr is
# dropped: a warning there (Node.js prints some) must not be read as a version.
# Windows PowerShell 5.1 turns redirected stderr into errors, so relax that here.
function Invoke-Tool([string]$File, [string[]]$Arguments) {
    $eap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = & $File @Arguments 2>$null | ForEach-Object { "$_" }
        return [pscustomobject]@{ Code = $LASTEXITCODE; Out = @($out) }
    } catch {
        return [pscustomobject]@{ Code = -1; Out = @("$_") }
    } finally {
        $ErrorActionPreference = $eap
    }
}

function Find-Exe([string]$Name) {
    $c = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($c) { return $c.Source }
    return $null
}

function Write-Ok([string]$Text) { Write-Host "OK      $Text" }
function Write-Note([string]$Text) { Write-Host "NOTE    $Text" -ForegroundColor Yellow }

Write-Host "amazon-order-sync dev kit: $Kit"
Write-Host ""

# --- 1. toolchains --------------------------------------------------------------
$sdk = $Pins.dotnetSdk
$sdkUrl = "https://builds.dotnet.microsoft.com/dotnet/Sdk/$sdk/dotnet-sdk-$sdk-win-x64.exe"
$dotnet = Find-Exe 'dotnet.exe'
$dotnetOk = $false
if (-not $dotnet) {
    $Missing.Add(".NET SDK $sdk is not installed (no dotnet.exe on PATH). Install it from $sdkUrl")
} else {
    $have = @((Invoke-Tool $dotnet @('--list-sdks')).Out | ForEach-Object { ($_ -split ' ')[0] } | Where-Object { $_ -match '^\d+\.\d+\.\d+' })
    if ($have -contains $sdk) {
        $dotnetOk = $true
        Write-Ok ".NET SDK $sdk"
    } else {
        $found = if ($have) { $have -join ', ' } else { 'none' }
        $Missing.Add(".NET SDK $sdk is not installed (found: $found). The packages in offline\nuget are the ones that SDK restores, so it has to be this version; it installs alongside others. Install it from $sdkUrl")
    }
}

$nodePin = $Pins.node
$nodeUrl = "https://nodejs.org/dist/v$nodePin/node-v$nodePin-x64.msi"
$node = Find-Exe 'node.exe'
$nodeMajor = 0
if (-not $node) {
    $Missing.Add("Node.js is not installed (no node.exe on PATH). CI uses $nodePin; any 18 or newer runs the server and the tests. Install it from $nodeUrl")
} else {
    $nv = "$((Invoke-Tool $node @('-p', 'process.versions.node')).Out | Select-Object -First 1)".Trim()
    if ($nv -match '^(\d+)\.') { $nodeMajor = [int]$Matches[1] }
    if ($nodeMajor -lt 18) {
        $Missing.Add("Node.js $nv is too old: the server needs 18 or newer, and CI uses $nodePin. Install it from $nodeUrl")
    } elseif ($nv -ne $nodePin) {
        Write-Ok "Node.js $nv ($node; CI uses $nodePin)"
    } else {
        Write-Ok "Node.js $nv ($node)"
    }
}

# Python: the wheels are for CPython 3.13 on 64-bit Windows. The py launcher can
# find a 3.13 that isn't first on PATH, so ask it first.
$pyPin = $Pins.python
$pyMinor = ($pyPin -split '\.')[0..1] -join '.'
$pyUrl = "https://www.python.org/ftp/python/$pyPin/python-$pyPin-amd64.exe"
# No double quotes in arguments: Windows PowerShell 5.1 passes them on mangled.
$probe = "import sys; print(sys.executable); print('.'.join(map(str, sys.version_info[:3]))); print(sys.implementation.name, sys.maxsize > 2**32)"
$python = $null
$seen = New-Object System.Collections.Generic.List[string]
$candidates = @()
$pyLauncher = Find-Exe 'py.exe'
if ($pyLauncher) { $candidates += , @($pyLauncher, "-$pyMinor") }
foreach ($n in @("python$pyMinor.exe", 'python.exe', 'python3.exe')) {
    $p = Find-Exe $n
    if ($p) { $candidates += , @($p) }
}
foreach ($c in $candidates) {
    $exe = $c[0]
    $pre = @($c | Select-Object -Skip 1)
    $r = Invoke-Tool $exe ($pre + @('-c', $probe))
    if ($r.Code -ne 0 -or $r.Out.Count -lt 3) { continue }
    $seen.Add("$($r.Out[1]) ($($r.Out[0]))")
    if ($r.Out[1] -like "$pyMinor.*" -and $r.Out[2] -eq 'cpython True') { $python = $r.Out[0]; $pyFound = $r.Out[1]; break }
}
if ($python) {
    if ($pyFound -ne $pyPin) { Write-Ok "Python $pyFound ($python; CI uses $pyPin)" }
    else { Write-Ok "Python $pyFound ($python)" }
} else {
    $found = if ($seen.Count) { $seen -join ', ' } else { 'none' }
    $Missing.Add("Python $pyMinor (64-bit) is not installed (found: $found). The wheels in offline\python are for CPython $pyMinor on 64-bit Windows; CI uses $pyPin. Install it from $pyUrl")
}

# Optional: report what is there; offline\README.md has the downloads.
$git = Find-Exe 'git.exe'
if ($git) { Write-Ok "$(((Invoke-Tool $git @('--version')).Out | Select-Object -First 1)) (optional)" }
else { Write-Note "Git is not installed (optional): without it, compare the sample files yourself after the art commands." }
$pwsh = Find-Exe 'pwsh.exe'
if ($pwsh) { Write-Ok "PowerShell $(((Invoke-Tool $pwsh @('-NoProfile', '-Command', '$PSVersionTable.PSVersion.ToString()')).Out | Select-Object -First 1)) (optional)" }
else { Write-Note "PowerShell 7 is not installed (optional): PSScriptAnalyzer runs in Windows PowerShell 5.1 too." }
$shellcheck = Find-Exe 'shellcheck.exe'
if ($shellcheck) { Write-Ok "ShellCheck $(((Invoke-Tool $shellcheck @('--version')).Out | Where-Object { $_ -like 'version:*' } | Select-Object -First 1) -replace '^version:\s*', '') (optional)" }
else { Write-Note "ShellCheck is not installed (optional): CI lints install.sh with ShellCheck $($Pins.shellcheck)." }

# npm: nothing to install. Say so if that ever stops being true.
foreach ($pj in @('server\package.json', 'tests\package.json')) {
    $j = Get-Content (Join-Path $Kit $pj) -Raw -Encoding UTF8 | ConvertFrom-Json
    foreach ($k in @('dependencies', 'devDependencies', 'optionalDependencies')) {
        if ($j.PSObject.Properties[$k] -and @($j.$k.PSObject.Properties).Count) {
            $Missing.Add("$pj lists npm packages under $k, and this kit has no npm cache for them.")
        }
    }
}
Write-Ok "npm: nothing to install (server\ and tests\ use only Node.js built-ins)"
Write-Host ""

# --- 2. PowerShell modules ------------------------------------------------------
$modules = Join-Path $Offline 'powershell'
if (($env:PSModulePath -split ';') -notcontains $modules) { $env:PSModulePath = "$modules;$env:PSModulePath" }
foreach ($name in @('PSScriptAnalyzer')) {
    $ver = $Pins.$name
    $psd1 = Join-Path $modules "$name\$ver\$name.psd1"
    if (-not (Test-Path $psd1)) { throw "The kit is incomplete: $psd1 is missing." }
    Get-Module $name | Remove-Module -Force
    Import-Module $psd1 -Force -Global
    $m = @(Get-Module $name)
    if ($m.Count -ne 1 -or $m[0].Version -ne [version]$ver -or -not $m[0].ModuleBase.StartsWith($modules, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Loaded $name $($m.Version -join ', ') from $($m.ModuleBase -join ', '), not $ver from $modules."
    }
    Write-Ok "$name $ver loaded from $($m[0].ModuleBase)"
}

# --- 3. NuGet ----------------------------------------------------------------------
if ($dotnetOk) {
    # dotnet picks the newest SDK unless global.json says otherwise, and another SDK
    # would ask for runtime packs this kit doesn't have.
    $globalJson = Join-Path $Kit 'global.json'
    $want = '{ "sdk": { "version": "' + $sdk + '", "rollForward": "disable" } }'
    if ((Test-Path $globalJson) -and ((Get-Content $globalJson -Raw).Trim() -ne $want)) {
        Write-Note "$globalJson exists and was not written by this script; left as it is."
    } else {
        [System.IO.File]::WriteAllText($globalJson, $want + "`r`n", (New-Object System.Text.UTF8Encoding $false))
        Write-Ok "global.json selects .NET SDK $sdk"
    }
    Push-Location $Kit
    try {
        $v = "$((Invoke-Tool $dotnet @('--version')).Out | Select-Object -First 1)".Trim()
        if ($v -ne $sdk) { throw "dotnet in $Kit runs SDK $v, not $sdk. Check $globalJson." }
        & $dotnet restore (Join-Path $Kit 'tray\AmazonOrderSync.Tray.csproj') -r win-x64 `
            --configfile (Join-Path $Offline 'nuget.config') --source (Join-Path $Offline 'nuget') -p:NuGetAudit=false
        if ($LASTEXITCODE) { throw "dotnet restore from offline\nuget failed (exit $LASTEXITCODE)." }
        Write-Ok "tray app restored from offline\nuget"
    } finally {
        Pop-Location
    }
}

# --- 4. Python -----------------------------------------------------------------------
$venvPython = Join-Path $Kit '.venv\Scripts\python.exe'
if ($python) {
    $fresh = $true
    if (Test-Path $venvPython) {
        $vv = "$((Invoke-Tool $venvPython @('-c', 'import sys; print(sys.version_info[0], sys.version_info[1])')).Out | Select-Object -First 1)".Trim()
        $fresh = ($vv -ne ($pyMinor -replace '\.', ' '))
    }
    if ($fresh) {
        & $python -m venv --clear (Join-Path $Kit '.venv')
        if ($LASTEXITCODE) { throw "python -m venv failed (exit $LASTEXITCODE)." }
    }
    & $venvPython -m pip install --no-index --find-links (Join-Path $Offline 'python') -r (Join-Path $Kit 'tools\requirements-art.txt') --disable-pip-version-check --quiet
    if ($LASTEXITCODE) { throw "pip install from offline\python failed (exit $LASTEXITCODE)." }
    $r = Invoke-Tool $venvPython @('-c', 'import fontTools, brotli; print(fontTools.version)')
    if ($r.Code) { throw "fonttools and brotli don't import in .venv (exit $($r.Code))." }
    Write-Ok ".venv has fonttools $($r.Out[0]) and brotli, from offline\python"
}

# --- 5. what's next ------------------------------------------------------------------
Write-Host ""
if ($Missing.Count) {
    Write-Host "Missing:" -ForegroundColor Red
    foreach ($m in $Missing) { Write-Host "  - $m" -ForegroundColor Red }
    Write-Host ""
    Write-Host "Install those (offline\README.md lists every download), then run this script again."
    exit 1
}

$forceExit = if ($nodeMajor -ge 20) { ' --test-force-exit' } else { '' }
$pssa = $Pins.PSScriptAnalyzer
Write-Host "Ready. From $Kit, in PowerShell:" -ForegroundColor Green
Write-Host @"

  # server and extension: syntax and tests
  node --check server\server.js
  node --check browser-extension\content.js
  node --check browser-extension\popup.js
  node --test$forceExit tests\server.test.js

  # tray app (-o . puts AmazonOrderSync.exe next to server\, where it looks)
  dotnet build tray -c Release -warnaserror --no-restore
  dotnet publish tray -c Release -o . --no-restore

  # PowerShell lint, with the kit's PSScriptAnalyzer
  Import-Module .\offline\powershell\PSScriptAnalyzer\$pssa\PSScriptAnalyzer.psd1
  'install.ps1', 'import-old-notes.ps1', 'tools\build-dev-kit.ps1', 'tools\dev-kit\setup.ps1' | ForEach-Object { Invoke-ScriptAnalyzer -Path `$_ -Settings .\PSScriptAnalyzerSettings.psd1 -Severity Error,Warning }
  shellcheck install.sh                  # if ShellCheck is installed

  # the samples and the README picture: CI fails if these change any file
  node tools\make-samples.mjs
  .venv\Scripts\python.exe tools\make-slip.py

To see what the last two changed, run "git init; git add -A; git commit -m kit" here first, then "git status" after.
"@
