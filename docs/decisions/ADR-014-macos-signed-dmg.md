# ADR-014: Signed and notarized macOS DMG

**Status:** Proposed (feat/macos-dmg branch)

## Context
Until 0.1.7, macOS users could only install Rusting through the Homebrew formula, which installs a bare, unsigned universal binary. A file downloaded in a browser gets the quarantine flag, and Gatekeeper blocks unsigned, unnotarized apps with a "cannot be verified" error. A website download and the official `homebrew/cask` repository both need an app that is signed with a Developer ID certificate and notarized by Apple.

## Decision
- **Package.** Each release also ships `rusting-<version>-universal-apple-darwin.dmg`, a DMG holding `Rusting.app` and an Applications shortcut. The name keeps the `rusting-<version>-<target>` shape from `docs/releasing.md`.
- **Bundle.** `Rusting.app` wraps the same universal binary as the tarball, with `assets/Rusting.icns` (made from `assets/logo.svg`) and an `Info.plist` written by `scripts/macos-dmg.sh`. The bundle identifier is `org.rusting-animation.rusting` and must never change, because macOS ties permissions and settings to it. The minimum system version is macOS 11.
- **Signing.** The app and the DMG are signed with the maintainer's individual "Developer ID Application" certificate, with the hardened runtime and a secure timestamp, and without entitlements (wgpu/Metal, file dialogs and openh264 need none). Users see the maintainer's name as the developer.
- **Notarization.** The DMG is notarized with an App Store Connect API key and the ticket is stapled to it, so Gatekeeper accepts it offline.
- **Automation.** `scripts/macos-dmg.sh` does all of the above. The release workflow imports the certificate into a temporary keychain from repository secrets and runs the script. A release fails if the secrets are missing or Apple rejects the DMG, instead of shipping without a DMG.
- **Homebrew.** The formula keeps installing the unsigned tarball. Homebrew does not quarantine formula installs, so it needs no signing.

## Consequences
- Releasing needs the Developer ID certificate (`.p12`) and the API key (`.p8`) as GitHub secrets. The certificate expires after five years and must then be renewed and the secret replaced.
- The DMG is signed by one person's certificate. Moving to an organization account later changes the developer name users see, and needs a new certificate, but the bundle identifier stays.
- A notarized app makes a `homebrew/cask` submission and a direct download on the website possible.
- Windows signing is a separate decision.
