<#
.SYNOPSIS
  Build the offline dev kit, amazon-order-sync-<version>-dev-windows-x64.zip.

.DESCRIPTION
  The zip holds one folder, amazon-order-sync-<version>-dev\: the source at -Ref
  (git archive, no .git) plus offline\ with
    cargo\vendor\  every Rust crate Cargo.lock names (cargo vendor), each with
                   its own licence files
    cargo\config.toml  the cargo settings that build from cargo\vendor only
    python\        wheels for tools\requirements-art.txt, CPython on win_amd64
    powershell\    PSScriptAnalyzer
    licences\      the licence and notices of the wheels and the module
    setup.ps1      from tools\dev-kit\setup.ps1
    README.md      from tools\dev-kit\README.md, with the versions filled in
  Versions come from tools\versions.json and tools\requirements-art.txt at -Ref,
  the files CI reads.

  Needs the network, git, cargo and Python with pip. The release workflow runs
  it on windows-latest:
      ./tools/build-dev-kit.ps1 -OutDir .
#>
[CmdletBinding()]
param(
    [string]$Ref = 'HEAD',
    [string]$Version,
    [string]$OutDir = 'dist',
    [string]$Python = 'python'
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$Root = Split-Path -Parent $PSScriptRoot
$OutDir = [System.IO.Path]::GetFullPath([System.IO.Path]::Combine((Get-Location).Path, $OutDir))
Add-Type -AssemblyName System.IO.Compression
try { Add-Type -AssemblyName System.IO.Compression.FileSystem } catch { Write-Verbose 'ZipFile is already loaded (PowerShell 7)' }

function Invoke-Native([string]$What, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE) { throw "$What failed (exit $LASTEXITCODE)." }
}

function Get-Output([string]$File, [string[]]$Arguments) {
    $eap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { return "$(& $File @Arguments 2>$null | Select-Object -First 1)".Trim() }
    catch { return '' }
    finally { $ErrorActionPreference = $eap }
}

# The URL must answer; a few tries, because one slow answer shouldn't fail a release.
function Test-Url([string]$Url) {
    for ($i = 1; $i -le 3; $i++) {
        try {
            Invoke-WebRequest -Uri $Url -Method Head -UseBasicParsing -TimeoutSec 30 | Out-Null
            return $true
        } catch { Start-Sleep -Seconds (5 * $i) }
    }
    return $false
}

# Copy the licence and notice files out of a zip (a wheel).
function Export-LicenceFile([string]$Archive, [string]$Pattern, [string]$Dest) {
    $files = @()
    $z = [System.IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        foreach ($e in $z.Entries) {
            if ($e.FullName -notmatch $Pattern -or $e.Length -eq 0) { continue }
            New-Item -ItemType Directory -Force $Dest | Out-Null
            $out = Join-Path $Dest ($e.FullName -replace '^.*/', '')
            [System.IO.Compression.ZipFileExtensions]::ExtractToFile($e, $out, $true)
            $files += Split-Path -Leaf $out
        }
        return , $files
    } finally { $z.Dispose() }
}

function Read-ZipText([string]$Archive, [string]$Pattern) {
    $z = [System.IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        $e = $z.Entries | Where-Object { $_.FullName -match $Pattern } | Select-Object -First 1
        if (-not $e) { return '' }
        $r = New-Object System.IO.StreamReader($e.Open())
        try { return $r.ReadToEnd() } finally { $r.Dispose() }
    } finally { $z.Dispose() }
}

# The [package] name, version and licence of a vendored crate's Cargo.toml.
function Read-CrateManifest([string]$Path) {
    $name = $ver = $lic = ''
    $inPackage = $false
    foreach ($line in [System.IO.File]::ReadAllLines($Path)) {
        if ($line -match '^\s*\[(.+)\]\s*$') { $inPackage = ($Matches[1] -eq 'package'); continue }
        if (-not $inPackage) { continue }
        if ($line -match '^\s*name\s*=\s*"(.*)"') { $name = $Matches[1] }
        elseif ($line -match '^\s*version\s*=\s*"(.*)"') { $ver = $Matches[1] }
        elseif ($line -match '^\s*license\s*=\s*"(.*)"') { $lic = $Matches[1] }
        elseif ($line -match '^\s*license-file\s*=\s*"(.*)"' -and -not $lic) { $lic = "see $($Matches[1])" }
    }
    return [pscustomobject]@{ Name = $name; Version = $ver; Licence = $lic }
}

$Work = Join-Path ([System.IO.Path]::GetTempPath()) ('aos-dev-kit-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $Work | Out-Null
try {
    # --- the source ---------------------------------------------------------------
    # LF line endings, as committed, whatever this machine's git does on checkout.
    $srcZip = Join-Path $Work 'src.zip'
    $sha = Get-Output 'git' @('-C', $Root, 'rev-parse', $Ref)
    if (-not $sha) { throw "git can't resolve $Ref in $Root." }
    if (-not $Version) {
        $manifest = (& git -C $Root show "${sha}:Cargo.toml") -join "`n"
        if ($manifest -notmatch '(?m)^version\s*=\s*"([^"]+)"') { throw "Cargo.toml at $Ref has no version." }
        $Version = $Matches[1]
    }
    $Name = "amazon-order-sync-$Version-dev"
    $Stage = Join-Path $Work $Name
    $Offline = Join-Path $Stage 'offline'
    Write-Host "Building $Name from $Ref ($sha)"
    Invoke-Native 'git archive' { git -C $Root -c core.autocrlf=false -c core.eol=lf archive --format=zip "--prefix=$Name/" -o $srcZip $sha }
    [System.IO.Compression.ZipFile]::ExtractToDirectory($srcZip, $Work)
    Remove-Item $srcZip
    $Pins = Get-Content (Join-Path $Stage 'tools\versions.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    foreach ($d in 'cargo', 'python', 'powershell', 'licences') { New-Item -ItemType Directory -Force (Join-Path $Offline $d) | Out-Null }
    $packages = New-Object System.Collections.Generic.List[object]

    # --- Rust crates: everything Cargo.lock names, for every platform ------------------
    $rust = $Pins.rust
    $have = Get-Output 'rustc' @('--version')
    if ($have -notmatch "^rustc $([regex]::Escape($rust))\b") { Write-Warning "tools\versions.json pins Rust $rust; this machine runs '$have'." }
    Push-Location $Stage
    try {
        Invoke-Native 'cargo vendor' { cargo vendor --locked --versioned-dirs (Join-Path 'offline' 'cargo\vendor') | Out-Null }
    } finally { Pop-Location }
    [System.IO.File]::WriteAllText((Join-Path $Offline 'cargo\config.toml'), @'
# Build from the crates in offline\cargo\vendor only. offline\setup.ps1 adds
# this to .cargo\config.toml; the path is relative to the kit's root.
[source.crates-io]
replace-with = "dev-kit"

[source.dev-kit]
directory = "offline/cargo/vendor"

[net]
offline = true
'@.Replace("`r`n", "`n"))
    foreach ($c in Get-ChildItem (Join-Path $Offline 'cargo\vendor') -Directory | Sort-Object Name) {
        $m = Read-CrateManifest (Join-Path $c.FullName 'Cargo.toml')
        $files = @(Get-ChildItem $c.FullName -File | Where-Object { $_.Name -match '^(?i)(LICEN[CS]E|COPYING|NOTICE|COPYRIGHT|UNLICENSE)' } | ForEach-Object Name)
        $licence = if ($m.Licence) { $m.Licence } else { 'see files' }
        $packages.Add([pscustomobject]@{ Kind = 'Rust crate'; Name = $m.Name; Version = $m.Version; Licence = $licence; Dir = "cargo\vendor\$($c.Name)\"; Files = $files })
    }

    # --- Python wheels: CPython <major.minor> on win_amd64, whatever runs this --------
    $pyMinor = ($Pins.python -split '\.')[0..1] -join '.'
    $abi = 'cp' + ($pyMinor -replace '\.', '')
    & $Python -m pip download --disable-pip-version-check --no-cache-dir --only-binary=:all: `
        --platform win_amd64 --python-version $pyMinor --implementation cp --abi $abi `
        -r (Join-Path $Stage 'tools\requirements-art.txt') -d (Join-Path $Offline 'python')
    if ($LASTEXITCODE) { throw "pip download failed (exit $LASTEXITCODE)." }
    foreach ($w in Get-ChildItem (Join-Path $Offline 'python') -Filter *.whl | Sort-Object Name) {
        $dist, $ver = $w.Name -split '-' | Select-Object -First 2
        $meta = Read-ZipText $w.FullName '^[^/]+\.dist-info/METADATA$'
        $licence = 'see files'
        if ($meta -match '(?m)^License-Expression:\s*(.+)$') { $licence = $Matches[1].Trim() }
        elseif ($meta -match '(?m)^License:\s*(\S.{0,60})$') { $licence = $Matches[1].Trim() }
        elseif ($meta -match '(?m)^Classifier: License :: OSI Approved :: (.+)$') { $licence = $Matches[1].Trim() }
        $files = Export-LicenceFile $w.FullName '^[^/]+\.dist-info/(licenses/.+|(?i)(LICEN[CS]E|COPYING|NOTICE|AUTHORS)[^/]*)$' (Join-Path $Offline "licences\python\$dist-$ver")
        if (-not $files.Count) { throw "$($w.Name) has no licence file to ship." }
        $packages.Add([pscustomobject]@{ Kind = 'Python wheel'; Name = $dist; Version = $ver; Licence = $licence; Dir = "licences\python\$dist-$ver\"; Files = $files })
    }

    # --- PowerShell modules -----------------------------------------------------------------
    # The runner image's copy when it has that exact version (the gallery is flaky from
    # hosted runners), otherwise Save-Module, 5 tries.
    foreach ($m in @('PSScriptAnalyzer')) {
        $ver = $Pins.$m
        $dest = Join-Path $Offline "powershell\$m\$ver"
        $local = Get-Module -ListAvailable -Name $m | Where-Object { $_.Version -eq [version]$ver } | Select-Object -First 1
        if ($local) {
            New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
            Copy-Item -Recurse $local.ModuleBase $dest
            Write-Host "$m ${ver}: copied from $($local.ModuleBase)"
        } else {
            for ($try = 1; $true; $try++) {
                try {
                    if (-not (Get-PSRepository -Name PSGallery -ErrorAction SilentlyContinue)) { Register-PSRepository -Default -ErrorAction Stop }
                    Save-Module -Name $m -RequiredVersion $ver -Repository PSGallery -Path (Join-Path $Offline 'powershell') -Force -ErrorAction Stop
                    Write-Host "$m ${ver}: saved from the PowerShell Gallery (try $try)"
                    break
                } catch {
                    if ($try -ge 5) { throw }
                    Write-Warning "Save-Module $m failed (try $try): $_"
                    Start-Sleep -Seconds (15 * $try)
                }
            }
        }
        $manifest = Join-Path $dest "$m.psd1"
        if ((Import-PowerShellDataFile $manifest).ModuleVersion -ne $ver) { throw "$manifest is not version $ver." }
        $sig = Get-AuthenticodeSignature $manifest
        if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') { throw "$manifest is not signed by Microsoft ($($sig.Status))." }
        $licDir = Join-Path $Offline "licences\powershell\$m.$ver"
        New-Item -ItemType Directory -Force $licDir | Out-Null
        $files = @(Get-ChildItem $dest -File | Where-Object { $_.Name -match '^(?i)(LICEN[CS]E|THIRDPARTYNOTICES|THIRD-PARTY-NOTICES|NOTICE)' })
        if (-not $files.Count) { throw "$m $ver has no licence file to ship." }
        $files | Copy-Item -Destination $licDir
        $packages.Add([pscustomobject]@{ Kind = 'PowerShell module'; Name = $m; Version = $ver; Licence = 'MIT'; Dir = "licences\powershell\$m.$ver\"; Files = @($files | ForEach-Object Name) })
    }

    # --- setup.ps1 and README.md ------------------------------------------------------------
    Copy-Item (Join-Path $Stage 'tools\dev-kit\setup.ps1') (Join-Path $Offline 'setup.ps1')

    $node = $Pins.node
    $py = $Pins.python
    $sc = $Pins.shellcheck
    $urls = [ordered]@{
        RUST_URL       = "https://static.rust-lang.org/dist/rust-$rust-x86_64-pc-windows-msvc.msi"
        NODE_URL       = "https://nodejs.org/dist/v$node/node-v$node-x64.msi"
        PYTHON_URL     = "https://www.python.org/ftp/python/$py/python-$py-amd64.exe"
        SHELLCHECK_URL = "https://github.com/koalaman/shellcheck/releases/download/v$sc/shellcheck-v$sc.zip"
    }
    foreach ($k in @($urls.Keys)) { if (-not (Test-Url $urls[$k])) { throw "$($urls[$k]) does not answer: check tools\versions.json." } }
    # The Visual Studio Build Tools aren't pinned: any 2019 or newer with the C++ workload links.
    $urls.MSVC_URL = 'https://visualstudio.microsoft.com/visual-cpp-build-tools/'

    # Git and PowerShell 7 aren't pinned: CI uses the runner's. Name the ones here.
    $gitVer = (Get-Output 'git' @('--version')) -replace '^git version\s*', ''
    $urls.GIT_URL = 'https://git-scm.com/downloads/win'
    if ($gitVer -match '^(\d+\.\d+\.\d+)\.windows\.(\d+)$') {
        $file = if ($Matches[2] -eq '1') { "Git-$($Matches[1])-64-bit.exe" } else { "Git-$($Matches[1]).$($Matches[2])-64-bit.exe" }
        $u = "https://github.com/git-for-windows/git/releases/download/v$gitVer/$file"
        if (Test-Url $u) { $urls.GIT_URL = $u }
    }
    $pwshVer = Get-Output 'pwsh' @('-NoProfile', '-Command', '$PSVersionTable.PSVersion.ToString()')
    $urls.PWSH_URL = 'https://github.com/PowerShell/PowerShell/releases/latest'
    if ($pwshVer -match '^\d+\.\d+\.\d+$') {
        $u = "https://github.com/PowerShell/PowerShell/releases/download/v$pwshVer/PowerShell-$pwshVer-win-x64.msi"
        if (Test-Url $u) { $urls.PWSH_URL = $u }
    } else { $pwshVer = '7' }
    if (-not $gitVer) { $gitVer = 'any' }

    $rows = @('| Package | Version | Kind | Licence | Licence files, in `offline\` |', '|---|---|---|---|---|')
    foreach ($p in $packages) {
        $files = if ($p.Files.Count) { ($p.Files | ForEach-Object { '`' + $p.Dir + $_ + '`' }) -join '<br>' } else { 'none in the package; the licence is its manifest''s expression' }
        $rows += "| $($p.Name) | $($p.Version) | $($p.Kind) | $($p.Licence) | $files |"
    }
    $crates = @($packages | Where-Object Kind -eq 'Rust crate').Count
    $tokens = @{
        VERSION = $Version; RUST = $rust; CRATES = $crates; NODE = $node; PYTHON = $py; PYTHON_MINOR = $pyMinor
        PSSA = $Pins.PSScriptAnalyzer; SHELLCHECK = $sc; GIT = $gitVer; PWSH = $pwshVer; PACKAGES = ($rows -join "`n")
    }
    foreach ($k in $urls.Keys) { $tokens[$k] = $urls[$k] }
    $readme = Get-Content (Join-Path $Stage 'tools\dev-kit\README.md') -Raw -Encoding UTF8
    foreach ($k in $tokens.Keys) { $readme = $readme.Replace("{{$k}}", [string]$tokens[$k]) }
    if ($readme -match '\{\{[A-Z_]+\}\}') { throw "offline\README.md still has a placeholder: $($Matches[0])" }
    [System.IO.File]::WriteAllText((Join-Path $Offline 'README.md'), $readme, (New-Object System.Text.UTF8Encoding $false))

    # --- the zip: forward slashes in entry names, on Windows PowerShell 5.1 too -------------
    New-Item -ItemType Directory -Force $OutDir | Out-Null
    $zip = Join-Path $OutDir "amazon-order-sync-$Version-dev-windows-x64.zip"
    if (Test-Path $zip) { Remove-Item $zip }
    $fs = [System.IO.File]::Open($zip, [System.IO.FileMode]::CreateNew)
    try {
        $archive = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
        try {
            foreach ($f in Get-ChildItem $Stage -Recurse -File -Force | Sort-Object FullName) {
                $entry = $Name + '/' + $f.FullName.Substring($Stage.Length + 1).Replace('\', '/')
                [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $f.FullName, $entry, [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
            }
        } finally { $archive.Dispose() }
    } finally { $fs.Dispose() }

    $mb = '{0:N1}' -f ((Get-Item $zip).Length / 1MB)
    Write-Host ""
    Write-Host "$zip ($mb MB)"
    Write-Host "SHA-256 $((Get-FileHash $zip -Algorithm SHA256).Hash.ToLower())"
    Write-Host "$crates Rust crates, $(@($packages | Where-Object Kind -ne 'Rust crate').Count) other packages"
    $packages | Where-Object Kind -ne 'Rust crate' | Format-Table Kind, Name, Version, Licence -AutoSize | Out-String -Width 200 | Write-Host
} finally {
    Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}
