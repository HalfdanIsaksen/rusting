# ADR-024: winget package for Windows

**Status:** Proposed (feat/winget branch)

## Context
macOS users install Rusting with one command from the Homebrew tap in this repository, and update it with `brew upgrade`. Windows users had to download the zip from GitHub Releases, unpack it somewhere and update it by hand. winget ships with Windows 10 and 11 and is the usual command-line installer there. Its default source is the community repository `microsoft/winget-pkgs`, where every package version is a set of YAML manifests merged through a pull request.

## Decision
- **Identifier.** The package is `HalfdanIsaksen.Rusting` (publisher `HalfdanIsaksen`, moniker `rusting`). winget users type it to install and upgrade, so it must never change.
- **Installer.** The manifests point at the existing `rusting-<version>-x86_64-pc-windows-msvc.zip` release file. It is installed as a zip with a nested portable `rusting.exe` and the command alias `rusting`. No MSI or setup program is built.
- **Dependency.** The binary links the Visual C++ runtime dynamically, so the package depends on `Microsoft.VCRedist.2015+.x64`, which winget installs when it is missing.
- **Manifests.** `scripts/winget-manifest.sh` writes the version, installer and default locale manifests (schema 1.10.0) from a version and the zip's SHA-256. They are not committed here: `microsoft/winget-pkgs` is their home.
- **Automation.** After the GitHub Release is published, the release workflow writes the manifests and a Windows job submits them with Microsoft's `wingetcreate`, using a personal access token in the `WINGET_TOKEN` secret. Without the secret the job only warns. A failed submission does not undo the release; it can be repeated by hand (`docs/releasing.md`).

## Consequences
- Windows users install with `winget install HalfdanIsaksen.Rusting` and update with `winget upgrade`, from the first release after this change.
- A new version reaches winget only after Microsoft's validation merges the pull request, usually within a day or two. The first submission also needs a moderator.
- The portable install has no Start menu shortcut and no `.rusting` file association. An MSI or MSIX installer would add both and is a separate decision, as is Authenticode signing (SmartScreen).
- Release file names keep the `rusting-<version>-<target>` shape, which the script relies on.
