# amazon-order-sync {{VERSION}} dev kit, Windows x64

This folder builds and tests amazon-order-sync {{VERSION}} with no network. It is the source at tag `v{{VERSION}}` plus `offline\`:

| Path | What |
|---|---|
| `offline\nuget\` | Every NuGet package the tray app's self-contained win-x64 restore downloads: the .NET {{DOTNET_RUNTIME}} runtime packs (the SDK fetches the ASP.NET Core one too, though the app doesn't use it) and the ILLink tasks |
| `offline\nuget.config` | A NuGet config whose only source is `offline\nuget` |
| `offline\python\` | Wheels for `tools\requirements-art.txt`, for CPython {{PYTHON_MINOR}} on 64-bit Windows |
| `offline\powershell\` | PSScriptAnalyzer {{PSSA}}, as `Save-Module` writes it |
| `offline\licences\` | The licence and notices of every package above |
| `offline\setup.ps1` | Sets it all up from this folder; downloads nothing |

The server and the tests use only Node.js built-ins, so there are no npm packages.

## Toolchains

These are not in the kit. Each link is the official installer for that version, which runs without a network once downloaded.

| Toolchain | Version | Needed for | Download |
|---|---|---|---|
| .NET SDK | {{DOTNET_SDK}}, exactly | The tray app | {{DOTNET_URL}} |
| Node.js | {{NODE}} (18 or newer works) | The server, the tests, the samples | {{NODE_URL}} |
| Python | {{PYTHON}} (any {{PYTHON_MINOR}}, 64-bit) | The README picture, `tools\make-slip.py` | {{PYTHON_URL}} |
| Windows PowerShell | 5.1, part of Windows | `install.ps1`, `setup.ps1`, PSScriptAnalyzer | |
| PowerShell 7 | {{PWSH}}, optional | CI runs PSScriptAnalyzer in it | {{PWSH_URL}} |
| Git | {{GIT}}, optional | Seeing what the sample generators changed | {{GIT_URL}} |
| ShellCheck | {{SHELLCHECK}}, optional | Linting `install.sh` | {{SHELLCHECK_URL}} |

The versions are the ones CI used to build this kit. The .NET SDK has to be {{DOTNET_SDK}}: the packages in `offline\nuget` are the ones that SDK asks for, and another SDK asks for other versions. It installs alongside any other .NET SDK, and `setup.ps1` writes a `global.json` here that selects it.

## Steps

1. Install the toolchains.
2. In this folder, run:

   ```powershell
   powershell -ExecutionPolicy Bypass -File offline\setup.ps1
   ```

   It checks the toolchains and says what is missing, loads PSScriptAnalyzer from `offline\powershell` and checks its version, writes `global.json`, restores the tray app from `offline\nuget` only, makes `.venv` and installs the wheels into it. It works in Windows PowerShell 5.1 and PowerShell 7, and can be run again.
3. Run the commands it prints: the syntax checks and the tests, the tray app build, PSScriptAnalyzer, and the two generators for the samples and the README picture.

Before publishing {{VERSION}}, the release workflow unzipped this kit on a fresh Windows runner, cut off the network and the package registries, ran `setup.ps1` and then those commands.

## Bundled packages

{{PACKAGES}}
