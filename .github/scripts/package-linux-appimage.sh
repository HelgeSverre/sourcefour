#!/usr/bin/env bash
# Builds the Linux installers: an AppImage for portable use and a Debian package
# with desktop integration. cargo-dist supports neither format, so these are
# maintained alongside the generated jobs, in the same shape as
# package-macos-pkg.sh.
set -euo pipefail

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(cd "$script_directory/../.." && pwd)"
cd "$repository_root"

output_directory="target/distrib"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output-dir)
      if [[ $# -lt 2 ]]; then
        echo "error: --output-dir requires a path" >&2
        exit 2
      fi
      output_directory="$2"
      shift 2
      ;;
    -h|--help)
      echo "Usage: .github/scripts/package-linux-appimage.sh [--output-dir PATH]"
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

required_commands=(cargo dpkg-deb sha256sum)
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

echo "Building Sourcefour $version for $(uname -m)"
cargo build --release --locked --package sourcefour

# appimagetool mounts its own runtime through FUSE, which no CI container has.
export APPIMAGE_EXTRACT_AND_RUN=1
cargo packager --packages sourcefour --release --formats appimage,deb

built_appimage="$(find target/release -maxdepth 1 -name '*.AppImage' -print -quit)"
if [[ -z "$built_appimage" ]]; then
  echo "error: cargo-packager did not produce an AppImage in target/release" >&2
  exit 1
fi

built_deb="$(find target/release -maxdepth 1 -name '*.deb' -print -quit)"
if [[ -z "$built_deb" ]]; then
  echo "error: cargo-packager did not produce a Debian package in target/release" >&2
  exit 1
fi

mkdir -p "$output_directory"
# Version-less on purpose; see the same note in package-macos-pkg.sh.
package_name="sourcefour-x86_64-unknown-linux-gnu.AppImage"
package_path="$output_directory/$package_name"
deb_name="sourcefour-x86_64-unknown-linux-gnu.deb"
deb_path="$output_directory/$deb_name"
rm -f "$package_path" "$package_path.sha256" "$deb_path" "$deb_path.sha256"
mv "$built_appimage" "$package_path"
mv "$built_deb" "$deb_path"
chmod +x "$package_path"

# An AppImage that cannot list its own contents is not one a user can run.
if ! "$package_path" --appimage-offset >/dev/null; then
  echo "error: $package_path is not a runnable AppImage" >&2
  exit 1
fi

# Check both the control metadata and archive payload. A malformed package can
# otherwise make it all the way to a release before a user discovers it.
dpkg-deb --info "$deb_path" >/dev/null
deb_contents="$(dpkg-deb --fsys-tarfile "$deb_path" | tar -tf -)"
if ! grep -Eq '^\.?/?usr/bin/sourcefour$' <<<"$deb_contents"; then
  echo "error: Debian package does not contain usr/bin/sourcefour" >&2
  exit 1
fi
if ! grep -Eq '^\.?/?usr/share/applications/.*\.desktop$' <<<"$deb_contents"; then
  echo "error: Debian package does not contain a desktop entry" >&2
  exit 1
fi
(
  cd "$output_directory"
  sha256sum "$package_name" > "$package_name.sha256"
  sha256sum "$deb_name" > "$deb_name.sha256"
)

echo "Created installer: $package_path"
echo "Created checksum:  $package_path.sha256"
echo "Created installer: $deb_path"
echo "Created checksum:  $deb_path.sha256"
