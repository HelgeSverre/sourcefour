#!/usr/bin/env bash
set -euo pipefail

readonly version="0.32.0"
readonly expected_sha256="b657cf8c04a8b7bc28f39d220f7e6dd11bbd2bdb072c552262bd9ccf597261b5"
readonly installer_url="https://github.com/axodotdev/cargo-dist/releases/download/v${version}/cargo-dist-installer.sh"
installer_path="$(mktemp "${TMPDIR:-/tmp}/cargo-dist-installer.XXXXXX")"
trap 'rm -f "$installer_path"' EXIT

curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
  "$installer_url" --output "$installer_path"

if command -v sha256sum >/dev/null 2>&1; then
  actual_sha256="$(sha256sum "$installer_path" | awk '{print $1}')"
else
  actual_sha256="$(shasum -a 256 "$installer_path" | awk '{print $1}')"
fi

if [[ "$actual_sha256" != "$expected_sha256" ]]; then
  echo "cargo-dist installer checksum mismatch" >&2
  exit 1
fi

sh "$installer_path"
