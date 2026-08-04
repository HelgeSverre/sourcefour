#!/usr/bin/env bash
set -euo pipefail

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(cd "$script_directory/../.." && pwd)"
cd "$repository_root"

unsigned=false
output_directory="target/distrib"

usage() {
  cat <<'EOF'
Usage: .github/scripts/package-macos-pkg.sh [--unsigned] [--output-dir PATH]

Build a universal Sourcefour.app and wrap it in a macOS installer package.

The default mode signs the app and installer, submits the installer to Apple's
notary service, staples the ticket, and verifies it with Gatekeeper. Use
--unsigned only for local packaging tests.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --unsigned)
      unsigned=true
      shift
      ;;
    --output-dir)
      if [[ $# -lt 2 ]]; then
        echo "error: --output-dir requires a path" >&2
        exit 2
      fi
      output_directory="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

required_commands=(cargo codesign jq lipo pkgutil plutil productbuild rustup shasum spctl xcrun)
for command_name in "${required_commands[@]}"; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "error: required command not found: $command_name" >&2
    exit 1
  fi
done

if ! cargo packager --version >/dev/null 2>&1; then
  echo "error: cargo-packager is not installed; run cargo install cargo-packager --version 0.11.8 --locked" >&2
  exit 1
fi

package_id="$(cargo pkgid --package sourcefour)"
# `cargo pkgid` prints either `...#0.1.0` or `...#name@0.1.0` depending on
# whether the package name matches its directory. Take the tail of both.
version="${package_id##*#}"
version="${version##*@}"
if [[ "$version" == "$package_id" || -z "$version" ]]; then
  echo "error: could not determine the sourcefour package version from: $package_id" >&2
  exit 1
fi

readonly minimum_macos_version="13.0"
export MACOSX_DEPLOYMENT_TARGET="$minimum_macos_version"

host_architecture="$(uname -m)"
case "$host_architecture" in
  arm64)
    other_target="x86_64-apple-darwin"
    arm_binary="target/release/sourcefour"
    intel_binary="target/$other_target/release/sourcefour"
    ;;
  x86_64)
    other_target="aarch64-apple-darwin"
    arm_binary="target/$other_target/release/sourcefour"
    intel_binary="target/release/sourcefour"
    ;;
  *)
    echo "error: unsupported macOS host architecture: $host_architecture" >&2
    exit 1
    ;;
esac

echo "Building Sourcefour $version for $host_architecture and $other_target"
rustup target add "$other_target"
cargo build --release --locked --package sourcefour
cargo build --release --locked --package sourcefour --target "$other_target"

universal_directory="target/universal-apple-darwin/release"
universal_binary="$universal_directory/sourcefour"
app_path="$universal_directory/Sourcefour.app"
mkdir -p "$universal_directory" "$output_directory"

lipo -create "$arm_binary" "$intel_binary" -output "$universal_binary"
universal_architectures="$(lipo -archs "$universal_binary")"
if [[ " $universal_architectures " != *" arm64 "* || " $universal_architectures " != *" x86_64 "* ]]; then
  echo "error: universal binary has unexpected architectures: $universal_architectures" >&2
  exit 1
fi

cargo packager --packages sourcefour --release --formats app --target universal-apple-darwin

if [[ ! -d "$app_path" ]]; then
  echo "error: cargo-packager did not produce $app_path" >&2
  exit 1
fi

bundle_version="$(plutil -extract CFBundleShortVersionString raw -o - "$app_path/Contents/Info.plist")"
bundle_minimum_macos="$(plutil -extract LSMinimumSystemVersion raw -o - "$app_path/Contents/Info.plist")"
if [[ "$bundle_version" != "$version" ]]; then
  echo "error: app version $bundle_version does not match Cargo version $version" >&2
  exit 1
fi
if [[ "$bundle_minimum_macos" != "$minimum_macos_version" ]]; then
  echo "error: app minimum macOS version is $bundle_minimum_macos, expected $minimum_macos_version" >&2
  exit 1
fi

# Deliberately version-less, and named the way cargo-dist names its own
# artifacts: the website's download button is a fixed
# `releases/latest/download/<name>` URL, which only resolves if the name does
# not move between releases.
package_name="sourcefour-universal-apple-darwin"
package_path="$output_directory/$package_name.pkg"
checksum_path="$package_path.sha256"
rm -f "$package_path" "$checksum_path"

if [[ "$unsigned" == true ]]; then
  echo "Creating unsigned installer for local testing"
  productbuild --component "$app_path" /Applications "$package_path"
else
  required_signing_variables=(
    APPLE_APPLICATION_SIGNING_IDENTITY
    APPLE_INSTALLER_SIGNING_IDENTITY
    APPLE_NOTARY_ISSUER_ID
    APPLE_NOTARY_KEY_ID
    APPLE_NOTARY_KEY_PATH
    APPLE_SIGNING_KEYCHAIN
    APPLE_TEAM_ID
  )
  for variable_name in "${required_signing_variables[@]}"; do
    if [[ -z "${!variable_name:-}" ]]; then
      echo "error: required environment variable $variable_name is not set" >&2
      exit 1
    fi
  done

  if [[ ! -f "$APPLE_SIGNING_KEYCHAIN" ]]; then
    echo "error: signing keychain not found: $APPLE_SIGNING_KEYCHAIN" >&2
    exit 1
  fi
  if [[ ! -f "$APPLE_NOTARY_KEY_PATH" ]]; then
    echo "error: App Store Connect API key not found: $APPLE_NOTARY_KEY_PATH" >&2
    exit 1
  fi

  echo "Signing Sourcefour.app with hardened runtime and a secure timestamp"
  codesign --force \
    --sign "$APPLE_APPLICATION_SIGNING_IDENTITY" \
    --keychain "$APPLE_SIGNING_KEYCHAIN" \
    --options runtime \
    --timestamp \
    "$app_path/Contents/MacOS/sourcefour"
  codesign --force \
    --sign "$APPLE_APPLICATION_SIGNING_IDENTITY" \
    --keychain "$APPLE_SIGNING_KEYCHAIN" \
    --options runtime \
    --timestamp \
    "$app_path"

  codesign --verify --deep --strict --verbose=2 "$app_path"
  signature_details="$(codesign --display --verbose=4 "$app_path" 2>&1)"
  if ! grep -Fq "TeamIdentifier=$APPLE_TEAM_ID" <<<"$signature_details"; then
    echo "error: signed app does not have expected TeamIdentifier $APPLE_TEAM_ID" >&2
    exit 1
  fi

  echo "Creating signed installer package"
  productbuild \
    --sign "$APPLE_INSTALLER_SIGNING_IDENTITY" \
    --keychain "$APPLE_SIGNING_KEYCHAIN" \
    --timestamp \
    --component "$app_path" /Applications \
    "$package_path"
  pkgutil --check-signature "$package_path"

  notary_arguments=(
    --key "$APPLE_NOTARY_KEY_PATH"
    --key-id "$APPLE_NOTARY_KEY_ID"
    --issuer "$APPLE_NOTARY_ISSUER_ID"
  )

  notarization_result="$output_directory/notarization-submit.json"
  notarization_log="$output_directory/notarization-log.json"
  rm -f "$notarization_result" "$notarization_log"

  echo "Submitting installer to Apple's notary service"
  set +e
  xcrun notarytool submit "$package_path" \
    "${notary_arguments[@]}" \
    --wait \
    --timeout "${APPLE_NOTARY_TIMEOUT:-45m}" \
    --output-format json > "$notarization_result"
  notarization_exit_code=$?
  set -e

  cat "$notarization_result"
  notarization_status="$(jq -r '.status // empty' "$notarization_result" 2>/dev/null || true)"
  submission_id="$(jq -r '.id // empty' "$notarization_result" 2>/dev/null || true)"

  if [[ $notarization_exit_code -ne 0 || "$notarization_status" != "Accepted" ]]; then
    if [[ -n "$submission_id" ]]; then
      xcrun notarytool log \
        "${notary_arguments[@]}" \
        "$submission_id" \
        "$notarization_log" || true
      if [[ -f "$notarization_log" ]]; then
        cat "$notarization_log"
      fi
    fi
    echo "error: notarization was not accepted (status: ${notarization_status:-unknown})" >&2
    exit 1
  fi

  xcrun stapler staple "$package_path"
  xcrun stapler validate "$package_path"
  pkgutil --check-signature "$package_path"
  spctl --assess --type install --verbose=4 "$package_path"
fi

if ! pkgutil --payload-files "$package_path" | grep -Fqx './Sourcefour.app/Contents/MacOS/sourcefour'; then
  echo "error: installer payload does not contain Sourcefour.app in the expected layout" >&2
  exit 1
fi

(
  cd "$output_directory"
  shasum -a 256 "$(basename "$package_path")" > "$(basename "$checksum_path")"
)

echo "Created installer: $package_path"
echo "Created checksum:  $checksum_path"
if [[ "$unsigned" == true ]]; then
  echo "warning: this local test installer is not signed or notarized and must not be distributed" >&2
fi
