#!/usr/bin/env bash
set -euo pipefail

required_variables=(
  APPLE_APPLICATION_CERTIFICATE_BASE64
  APPLE_APPLICATION_CERTIFICATE_PASSWORD
  APPLE_APPLICATION_SIGNING_IDENTITY
  APPLE_INSTALLER_CERTIFICATE_BASE64
  APPLE_INSTALLER_CERTIFICATE_PASSWORD
  APPLE_INSTALLER_SIGNING_IDENTITY
  APPLE_NOTARY_KEY_BASE64
  APPLE_SIGNING_KEYCHAIN
  APPLE_NOTARY_KEY_PATH
)

for variable_name in "${required_variables[@]}"; do
  if [[ -z "${!variable_name:-}" ]]; then
    echo "error: required environment variable $variable_name is not set" >&2
    exit 1
  fi
done

if [[ -e "$APPLE_SIGNING_KEYCHAIN" ]]; then
  echo "error: signing keychain already exists: $APPLE_SIGNING_KEYCHAIN" >&2
  exit 1
fi

umask 077
asset_directory="$(dirname "$APPLE_NOTARY_KEY_PATH")/sourcefour-signing-assets"
mkdir -p "$asset_directory"

application_certificate="$asset_directory/application.p12"
installer_certificate="$asset_directory/installer.p12"

cleanup_certificates() {
  rm -f "$application_certificate" "$installer_certificate"
  rmdir "$asset_directory" 2>/dev/null || true
}
trap cleanup_certificates EXIT

printf '%s' "$APPLE_APPLICATION_CERTIFICATE_BASE64" | /usr/bin/base64 -D > "$application_certificate"
printf '%s' "$APPLE_INSTALLER_CERTIFICATE_BASE64" | /usr/bin/base64 -D > "$installer_certificate"
printf '%s' "$APPLE_NOTARY_KEY_BASE64" | /usr/bin/base64 -D > "$APPLE_NOTARY_KEY_PATH"

if ! grep -Fq -- "-----BEGIN PRIVATE KEY-----" "$APPLE_NOTARY_KEY_PATH"; then
  echo "error: APPLE_NOTARY_KEY_BASE64 did not decode to an App Store Connect .p8 key" >&2
  exit 1
fi

keychain_password="$(openssl rand -base64 32)"
security create-keychain -p "$keychain_password" "$APPLE_SIGNING_KEYCHAIN"
security set-keychain-settings -lut 7200 "$APPLE_SIGNING_KEYCHAIN"
security unlock-keychain -p "$keychain_password" "$APPLE_SIGNING_KEYCHAIN"

existing_keychains=()
while read -r keychain_path; do
  keychain_path="${keychain_path//\"/}"
  if [[ -n "$keychain_path" ]]; then
    existing_keychains+=("$keychain_path")
  fi
done < <(security list-keychains -d user)
security list-keychains -d user -s "$APPLE_SIGNING_KEYCHAIN" "${existing_keychains[@]}"

security import "$application_certificate" \
  -k "$APPLE_SIGNING_KEYCHAIN" \
  -f pkcs12 \
  -P "$APPLE_APPLICATION_CERTIFICATE_PASSWORD" \
  -T /usr/bin/codesign \
  -T /usr/bin/security

security import "$installer_certificate" \
  -k "$APPLE_SIGNING_KEYCHAIN" \
  -f pkcs12 \
  -P "$APPLE_INSTALLER_CERTIFICATE_PASSWORD" \
  -T /usr/bin/productbuild \
  -T /usr/bin/productsign \
  -T /usr/bin/security

security set-key-partition-list \
  -S apple-tool:,apple:,codesign: \
  -s \
  -k "$keychain_password" \
  "$APPLE_SIGNING_KEYCHAIN"

if ! security find-certificate -c "$APPLE_APPLICATION_SIGNING_IDENTITY" "$APPLE_SIGNING_KEYCHAIN" >/dev/null; then
  echo "error: application certificate does not match APPLE_APPLICATION_SIGNING_IDENTITY" >&2
  exit 1
fi

if ! security find-certificate -c "$APPLE_INSTALLER_SIGNING_IDENTITY" "$APPLE_SIGNING_KEYCHAIN" >/dev/null; then
  echo "error: installer certificate does not match APPLE_INSTALLER_SIGNING_IDENTITY" >&2
  exit 1
fi

search_list_identities="$(security find-identity -v -p codesigning)"
if ! grep -Fq "\"$APPLE_APPLICATION_SIGNING_IDENTITY\"" <<<"$search_list_identities"; then
  echo "error: application signing identity is not available through the user keychain search list" >&2
  exit 1
fi

echo "Imported Apple signing identities:"
security find-identity -v "$APPLE_SIGNING_KEYCHAIN"
