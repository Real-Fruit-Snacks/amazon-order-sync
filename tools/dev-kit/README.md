# amazon-order-sync {{VERSION}} dev kit, Windows x64

This folder builds and tests amazon-order-sync {{VERSION}} with no network. It is the source at tag `v{{VERSION}}` plus `offline\`:

| Path | What |
|---|---|
| `offline\cargo\vendor\` | All {{CRATES}} Rust crates `Cargo.lock` names, for every platform, as `cargo vendor` writes them; each folder has the crate's own licence files |
| `offline\cargo\config.toml` | The cargo settings that build from `offline\cargo\vendor` only |
| `offline\python\` | Wheels for `tools\requirements-art.txt`, for CPython {{PYTHON_MINOR}} on 64-bit Windows |
| `offline\powershell\` | PSScriptAnalyzer {{PSSA}}, as `Save-Module` writes it |
| `offline\licences\` | The licence and notices of the wheels and the module |
| `offline\setup.ps1` | Sets it all up from this folder; downloads nothing |

The tests use only Node.js built-ins, so there are no npm packages.

## Toolchains

These are not in the kit. Each link is the official installer, which runs without a network once downloaded.

| Toolchain | Version | Needed for | Download |
|---|---|---|---|
| Rust | {{RUST}} (MSVC) | The program: `cargo build`, `cargo test` | {{RUST_URL}} |
| Visual Studio Build Tools | 2019 or newer, "Desktop development with C++" | The linker and the Windows SDK Rust uses | {{MSVC_URL}} |
| Node.js | {{NODE}} (18 or newer works) | The tests, the samples | {{NODE_URL}} |
| Python | {{PYTHON}} (any {{PYTHON_MINOR}}, 64-bit) | The README picture, `tools\make-slip.py` | {{PYTHON_URL}} |
| Windows PowerShell | 5.1, part of Windows | `install.ps1`, `setup.ps1`, PSScriptAnalyzer | |
| PowerShell 7 | {{PWSH}}, optional | CI runs PSScriptAnalyzer in it | {{PWSH_URL}} |
| Git | {{GIT}}, optional | Seeing what the sample generators changed | {{GIT_URL}} |
| ShellCheck | {{SHELLCHECK}}, optional | Linting `install.sh` | {{SHELLCHECK_URL}} |

The versions are the ones CI used to build this kit. Another Rust that builds the crates in `Cargo.lock` works too; `setup.ps1` says when yours differs.

## Steps

1. Install the toolchains.
2. In this folder, run:

   ```powershell
   powershell -ExecutionPolicy Bypass -File offline\setup.ps1
   ```

   It checks the toolchains and says what is missing, points cargo at `offline\cargo\vendor` (in `.cargo\config.toml`) and checks every crate resolves from it, loads PSScriptAnalyzer from `offline\powershell` and checks its version, makes `.venv` and installs the wheels into it. It works in Windows PowerShell 5.1 and PowerShell 7, and can be run again.
3. Run the commands it prints: the Rust tests and the build, the HTTP tests against the built program, PSScriptAnalyzer, and the two generators for the samples and the README picture.

Before publishing {{VERSION}}, the release workflow unzipped this kit on a fresh Windows runner, cut off the network, emptied cargo's cache, ran `setup.ps1` and then those commands.

## Bundled packages

{{PACKAGES}}
