#!/usr/bin/env bash
# Wraps the universal macOS binary in Rusting.app, signs it with a Developer ID
# certificate, puts it in a DMG, and notarizes and staples the DMG (ADR-014).
#
# Usage: scripts/macos-dmg.sh <version> [--no-notarize]
#
# Environment:
#   MACOS_SIGN_IDENTITY   "Developer ID Application: <name> (<team id>)" (required)
#   Notarization credentials, one of:
#     NOTARY_PROFILE      a profile saved with `xcrun notarytool store-credentials`
#     NOTARY_KEY_PATH, NOTARY_KEY_ID and optionally NOTARY_ISSUER_ID
#                         an App Store Connect API key (.p8); leave
#                         NOTARY_ISSUER_ID empty for an individual key
#
# Reads:  target/universal-apple-darwin/release/rusting, assets/Rusting.icns
# Writes: dist/rusting-<version>-universal-apple-darwin.dmg
set -euo pipefail

usage() {
  echo "usage: $0 <version> [--no-notarize]" >&2
  exit 1
}

[[ $# -ge 1 && $# -le 2 ]] || usage
version="$1"
notarize=1
if [[ $# -eq 2 ]]; then
  [[ "$2" == "--no-notarize" ]] || usage
  notarize=0
fi

: "${MACOS_SIGN_IDENTITY:?set MACOS_SIGN_IDENTITY to the Developer ID Application identity}"

root="$(cd "$(dirname "$0")/.." && pwd)"
binary="$root/target/universal-apple-darwin/release/rusting"
icon="$root/assets/Rusting.icns"
dist="$root/dist"
dmg="$dist/rusting-${version}-universal-apple-darwin.dmg"
# Never change this: macOS ties permissions and settings to it.
bundle_id="org.rusting-animation.rusting"

for file in "$binary" "$icon"; do
  if [[ ! -f "$file" ]]; then
    echo "missing $file" >&2
    exit 1
  fi
done

# Check the notarization credentials before the slow steps.
notary_args=()
if [[ $notarize -eq 1 ]]; then
  if [[ -n "${NOTARY_PROFILE:-}" ]]; then
    notary_args=(--keychain-profile "$NOTARY_PROFILE")
  elif [[ -n "${NOTARY_KEY_PATH:-}" && -n "${NOTARY_KEY_ID:-}" ]]; then
    notary_args=(--key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID")
    if [[ -n "${NOTARY_ISSUER_ID:-}" ]]; then
      notary_args+=(--issuer "$NOTARY_ISSUER_ID")
    fi
  else
    echo "set NOTARY_PROFILE, or NOTARY_KEY_PATH and NOTARY_KEY_ID, or pass --no-notarize" >&2
    exit 1
  fi
fi

# Build in a temporary folder so dist/ only ever gets the finished DMG.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
app="$work/dmg/Rusting.app"

echo "==> Assembling Rusting.app ${version}"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/rusting"
cp "$icon" "$app/Contents/Resources/Rusting.icns"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Rusting</string>
  <key>CFBundleDisplayName</key><string>Rusting</string>
  <key>CFBundleIdentifier</key><string>${bundle_id}</string>
  <key>CFBundleExecutable</key><string>rusting</string>
  <key>CFBundleIconFile</key><string>Rusting</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.graphics-design</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>MIT License</string>
</dict>
</plist>
EOF
plutil -lint "$app/Contents/Info.plist" >/dev/null

echo "==> Signing Rusting.app"
codesign --force --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$app"
codesign --verify --strict --verbose=2 "$app"

echo "==> Building $(basename "$dmg")"
ln -s /Applications "$work/dmg/Applications"
mkdir -p "$dist"
rm -f "$dmg"
hdiutil create -volname "Rusting" -srcfolder "$work/dmg" -ov -format UDZO "$dmg"
codesign --force --timestamp --sign "$MACOS_SIGN_IDENTITY" "$dmg"

if [[ $notarize -eq 0 ]]; then
  echo "==> Skipped notarization: $dmg is signed but not notarized"
  exit 0
fi

echo "==> Notarizing (usually a few minutes)"
result="$work/notary.json"
xcrun notarytool submit "$dmg" "${notary_args[@]}" --wait --output-format json > "$result" || true
status="$(plutil -extract status raw -o - "$result" 2>/dev/null || echo unknown)"
if [[ "$status" != "Accepted" ]]; then
  echo "Notarization status: $status" >&2
  cat "$result" >&2
  id="$(plutil -extract id raw -o - "$result" 2>/dev/null || true)"
  if [[ -n "$id" ]]; then
    xcrun notarytool log "$id" "${notary_args[@]}" >&2 || true
  fi
  exit 1
fi

echo "==> Stapling"
xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"
spctl --assess --type open --context context:primary-signature -vv "$dmg"
echo "==> Done: $dmg"
