# Releasing

This repository is also the Homebrew tap: `Formula/rusting.rb` lives here, so
users tap the repo URL directly (see the README's Install section).

## Make a release

1. Set the new version in `Cargo.toml` (`[workspace.package] version`), run
   `cargo build` so `Cargo.lock` updates, and merge that to `main`.
2. Tag the merged commit and push the tag:

   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```

The `Release` workflow (`.github/workflows/release.yml`) then:

- fails early if the tag does not match the `Cargo.toml` version,
- builds `rusting` for macOS (one universal binary for Apple Silicon and
  Intel), Windows and Linux,
- publishes a GitHub Release with `rusting-<version>-<target>.tar.gz` (or
  `.zip` on Windows) and `SHA256SUMS.txt`,
- regenerates `Formula/rusting.rb` with the new URLs and checksums
  (`scripts/homebrew-formula.sh`) and commits it to `main`.

Users get the new version with `brew update && brew upgrade rusting`.

## Rules

- Never move or delete a published tag or replace release files. Homebrew
  checks each download against its SHA-256, so changed files break installs.
- Keep the file names in the same `rusting-<version>-<target>` shape. The
  formula script and any future winget or Homebrew automation rely on them.
- Do not edit `Formula/rusting.rb` by hand; change the script instead.

## Later

- **winget:** the Windows zip is ready for a winget manifest
  (`HalfdanIsaksen.Rusting`), submitted to `microsoft/winget-pkgs`.
- **Signed macOS app:** a notarized `Rusting.app` / `.dmg` would allow a cask
  and a download on the website, and is required for the official
  `homebrew/cask` repository.
- **Official Homebrew:** when accepted, delete `Formula/rusting.rb` and add a
  `tap_migrations.json` here pointing `rusting` at the official repository, so
  existing users move over on their next `brew update`.
