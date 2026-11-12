# Releasing

This repository is also the Homebrew tap: `Formula/rusting.rb` lives here, so
users tap the repo URL directly (see the README's Install section).

## Make a release

1. Fill in `release-notes.md` (see [Release notes](#release-notes) below).
   It is easiest to add a line whenever a user-facing PR lands.
2. Set the new version in `Cargo.toml` (`[workspace.package] version`), run
   `cargo build` so `Cargo.lock` updates, and merge that to `main`.
3. Tag the merged commit and push the tag:

   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```

The `Release` workflow (`.github/workflows/release.yml`) then:

- fails early if the tag does not match the `Cargo.toml` version, or if
  `release-notes.md` is still the empty template
  (`scripts/check-release-notes.sh`),
- builds `rusting` for macOS (one universal binary for Apple Silicon and
  Intel), Windows and Linux,
- wraps the macOS binary in a signed and notarized `Rusting.app` inside
  `rusting-<version>-universal-apple-darwin.dmg` (`scripts/macos-dmg.sh`,
  see [macOS signing and notarization](#macos-signing-and-notarization)),
- publishes a GitHub Release whose text is `release-notes.md` (without
  comments and empty sections, `scripts/clean-release-notes.sh`), followed by
  GitHub's generated list of merged PRs, with `rusting-<version>-<target>.tar.gz` (or
  `.zip` on Windows) and `SHA256SUMS.txt`,
- regenerates `Formula/rusting.rb` with the new URLs and checksums
  (`scripts/homebrew-formula.sh`), resets `release-notes.md` to
  `scripts/release-notes-template.md`, and commits both to `main`.

Users get the new version with `brew update && brew upgrade rusting`.

## Release notes

`release-notes.md` only holds the notes for the next release. After a
release it is reset, so old notes live only on the GitHub releases. The
website (rusting-animation.org) reads them from there for the Getting
started callout and the What's new page, so keep this exact shape:

```markdown
Adds frame tags, sprite sheet export and MOV video export.

### New
- **Frame tags**: name a range of frames, such as "walk" or "jump". [Frame tags](https://rusting-animation.org/docs/frame-tags)

### Improved
- **Key icon buttons**: the timeline's key actions are now icon buttons with hover descriptions.

### Fixed
- **Onion skin colours**: earlier frames no longer show in the wrong tint.
```

- The first line is one sentence that starts with a verb. The website shows
  it as "Rusting X is out. <sentence>".
- Use only `### New`, `### Improved` and `### Fixed`, in that order. Leave
  out empty sections (the workflow removes them anyway).
- Every bullet is `- **Feature name**: one sentence.`, optionally followed by
  one link to the docs page. Use full `https://rusting-animation.org/docs/...`
  URLs so the link works on GitHub as well.
- Write for users. Leave out internal changes (CI, refactors, CLAUDE.md);
  they still appear in GitHub's generated PR list below the notes.

## macOS signing and notarization

The release workflow signs `Rusting.app` with the maintainer's "Developer ID
Application" certificate and notarizes the DMG with an App Store Connect API
key (ADR-014). It needs these repository secrets (Settings → Secrets and
variables → Actions):

| Secret | Value |
|---|---|
| `MACOS_CERT_P12` | The certificate exported from Keychain Access as `.p12`, base64: `base64 -i DeveloperID.p12 \| pbcopy` |
| `MACOS_CERT_PASSWORD` | The password chosen when exporting the `.p12` |
| `MACOS_SIGN_IDENTITY` | `Developer ID Application: <name> (<team id>)`, as listed by `security find-identity -v -p codesigning` |
| `NOTARY_KEY_P8` | The App Store Connect API key, base64: `base64 -i AuthKey_<key id>.p8 \| pbcopy` |
| `NOTARY_KEY_ID` | The key ID (the part after `AuthKey_` in the file name) |
| `NOTARY_ISSUER_ID` | The issuer ID shown above the Team Keys list. Leave it unset for an individual key |

If a secret is missing or Apple rejects the DMG, the release fails before
anything is published; the notary log is printed in the job output.

To build the DMG locally, build the universal binary as the workflow does,
then run:

```bash
MACOS_SIGN_IDENTITY="Developer ID Application: <name> (<team id>)" NOTARY_PROFILE=rusting-notary scripts/macos-dmg.sh <version>
```

`NOTARY_PROFILE` is a profile saved with `xcrun notarytool store-credentials`.
Pass `--no-notarize` to only sign. The DMG lands in `dist/`.

`assets/Rusting.icns` is made from `assets/logo.svg`. Regenerate it with
`rsvg-convert` (`brew install librsvg`), `sips` and `iconutil` when the logo
changes. The Developer ID certificate is valid for five years; renew it in
the Apple Developer account and replace the two certificate secrets.

## Rules

- Never move or delete a published tag or replace release files. Homebrew
  checks each download against its SHA-256, so changed files break installs.
- Keep the file names in the same `rusting-<version>-<target>` shape. The
  formula script and any future winget or Homebrew automation rely on them.
- Do not edit `Formula/rusting.rb` by hand; change the script instead.

## Later

- **winget:** the Windows zip is ready for a winget manifest
  (`HalfdanIsaksen.Rusting`), submitted to `microsoft/winget-pkgs`.
- **Homebrew cask:** the notarized DMG can back a cask, and is required for
  the official `homebrew/cask` repository.
- **Signed Windows build:** Authenticode signing (for example Azure Artifact
  Signing) to avoid SmartScreen warnings.
- **Official Homebrew:** when accepted, delete `Formula/rusting.rb` and add a
  `tap_migrations.json` here pointing `rusting` at the official repository, so
  existing users move over on their next `brew update`.
