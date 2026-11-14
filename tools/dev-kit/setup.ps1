<#
.SYNOPSIS
  Set up the amazon-order-sync dev kit. Downloads nothing.

.DESCRIPTION
  Run it from the unzipped kit:
      powershell -ExecutionPolicy Bypass -File offline\setup.ps1
  It:
    1. Checks the toolchains the kit leaves out (Rust, Node.js and Python;
       Git, PowerShell 7 and ShellCheck are optional) and says what is
       missing.
    2. Loads PSScriptAnalyzer from offline\powershell and checks its version.
    3. Points cargo at offline\cargo\vendor (.cargo\config.toml) and checks
       that every crate in Cargo.lock resolves from it.
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
$env:CARGO_NET_OFFLINE = 'true'
$env:PIP_NO_INDEX = '1'
$env:PIP_DISABLE_PIP_VERSION_CHECK = '1'

# Run a program, return its exit code and what it wrote to stdout. stderr is
# dropped: a warning there must not be read as a version. Windows PowerShell
# 5.1 turns redirected stderr into errors, so relax that here.
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
$rustPin = $Pins.rust
$rustUrl = "https://static.rust-lang.org/dist/rust-$rustPin-x86_64-pc-windows-msvc.msi"
$cargo = Find-Exe 'cargo.exe'
$rustc = Find-Exe 'rustc.exe'
$cargoOk = $false
if (-not $cargo -or -not $rustc) {
    $Missing.Add("Rust is not installed (no cargo.exe/rustc.exe on PATH). CI uses $rustPin. Install it from $rustUrl, and the Visual Studio Build Tools with the C++ workload for the linker.")
} else {
    $rv = "$((Invoke-Tool $rustc @('--version')).Out | Select-Object -First 1)".Trim()
    $host_ = "$((Invoke-Tool $rustc @('-vV')).Out | Where-Object { $_ -like 'host:*' } | Select-Object -First 1)" -replace '^host:\s*', ''
    $cargoOk = $true
    if ($rv -match "^rustc $([regex]::Escape($rustPin))\b") { Write-Ok "$rv ($host_)" }
    else { Write-Ok "$rv ($host_; CI uses $rustPin)" }
    if ($host_ -notlike '*-msvc') { Write-Note "Rust's host is $host_; the release is built for x86_64-pc-windows-msvc." }
}

$nodePin = $Pins.node
$nodeUrl = "https://nodejs.org/dist/v$nodePin/node-v$nodePin-x64.msi"
$node = Find-Exe 'node.exe'
$nodeMajor = 0
if (-not $node) {
    $Missing.Add("Node.js is not installed (no node.exe on PATH). CI uses $nodePin; any 18 or newer runs the tests. Install it from $nodeUrl")
} else {
    $nv = "$((Invoke-Tool $node @('-p', 'process.versions.node')).Out | Select-Object -First 1)".Trim()
    if ($nv -match '^(\d+)\.') { $nodeMajor = [int]$Matches[1] }
    if ($nodeMajor -lt 18) {
        $Missing.Add("Node.js $nv is too old: the tests need 18 or newer, and CI uses $nodePin. Install it from $nodeUrl")
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
$j = Get-Content (Join-Path $Kit 'tests\package.json') -Raw -Encoding UTF8 | ConvertFrom-Json
foreach ($k in @('dependencies', 'devDependencies', 'optionalDependencies')) {
    if ($j.PSObject.Properties[$k] -and @($j.$k.PSObject.Properties).Count) {
        $Missing.Add("tests\package.json lists npm packages under $k, and this kit has no npm cache for them.")
    }
}
Write-Ok "npm: nothing to install (tests\ uses only Node.js built-ins)"
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

# --- 3. cargo ------------------------------------------------------------------------
# The source has .cargo\config.toml (it links the C runtime in); add the kit's crate
# source to it once.
$cargoDir = Join-Path $Kit '.cargo'
$cargoConfig = Join-Path $cargoDir 'config.toml'
$kitConfig = (Get-Content (Join-Path $Offline 'cargo\config.toml') -Raw -Encoding UTF8) -replace "`r`n", "`n"
New-Item -ItemType Directory -Force $cargoDir | Out-Null
$current = if (Test-Path $cargoConfig) { (Get-Content $cargoConfig -Raw -Encoding UTF8) -replace "`r`n", "`n" } else { '' }
if ($current -match '\[source\.dev-kit\]') {
    Write-Ok ".cargo\config.toml already builds from offline\cargo\vendor"
} else {
    $sep = if ($current -and -not $current.EndsWith("`n")) { "`n`n" } elseif ($current) { "`n" } else { '' }
    [System.IO.File]::WriteAllText($cargoConfig, $current + $sep + $kitConfig, (New-Object System.Text.UTF8Encoding $false))
    Write-Ok ".cargo\config.toml now builds from offline\cargo\vendor"
}
if ($cargoOk) {
    Push-Location $Kit
    try {
        $r = Invoke-Tool $cargo @('metadata', '--offline', '--locked', '--format-version', '1')
        if ($r.Code) { throw "cargo metadata --offline failed (exit $($r.Code)): a crate in Cargo.lock is not in offline\cargo\vendor." }
        Write-Ok "every crate in Cargo.lock resolves from offline\cargo\vendor"
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

  # the program: Rust tests, then the build (target\release\amazon-order-sync.exe)
  cargo test --offline --locked
  cargo build --release --offline --locked

  # the extension's syntax, and the HTTP tests against the built program
  node --check browser-extension\background.js
  node --check browser-extension\content.js
  node --check browser-extension\popup.js
  node --test$forceExit tests\server.test.js

  # PowerShell lint, with the kit's PSScriptAnalyzer
  Import-Module .\offline\powershell\PSScriptAnalyzer\$pssa\PSScriptAnalyzer.psd1
  'install.ps1', 'import-old-notes.ps1', 'tools\build-dev-kit.ps1', 'tools\dev-kit\setup.ps1' | ForEach-Object { Invoke-ScriptAnalyzer -Path `$_ -Settings .\PSScriptAnalyzerSettings.psd1 -Severity Error,Warning }
  shellcheck install.sh                  # if ShellCheck is installed

  # the samples and the README picture: CI fails if these change any file
  node tools\make-samples.mjs
  .venv\Scripts\python.exe tools\make-slip.py

To see what the last two changed, run "git init; git add -A; git commit -m kit" here first, then "git status" after.
"@
