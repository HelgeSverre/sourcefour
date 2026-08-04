#!/bin/bash
# Builds dist/Sourcefour.app: a release build in a proper macOS bundle,
# ad-hoc signed. Distribution signing and notarization need a Developer ID
# and are wired up separately when shipping.
set -euo pipefail

cd "$(dirname "$0")/.."
version="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')"

echo "Building release…"
cargo build --release --locked

app="dist/Sourcefour.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

cp target/release/sourcefour "$app/Contents/MacOS/sourcefour"
# The interface assets are compiled into the binary; only the icon is a file.
cp apps/sourcefour/assets/icon/Sourcefour.icns "$app/Contents/Resources/Sourcefour.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>sourcefour</string>
    <key>CFBundleIdentifier</key><string>no.lisethsolutions.sourcefour</string>
    <key>CFBundleName</key><string>Sourcefour</string>
    <key>CFBundleDisplayName</key><string>Sourcefour</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>${version}</string>
    <key>CFBundleVersion</key><string>${version}</string>
    <key>CFBundleIconFile</key><string>Sourcefour</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
    <key>CFBundleSupportedPlatforms</key><array><string>MacOSX</string></array>
</dict>
</plist>
PLIST

codesign --force --sign - "$app"
echo "Packaged $app (version ${version}, ad-hoc signed)"
echo "Launch with: open $app --args <repository-path>"
