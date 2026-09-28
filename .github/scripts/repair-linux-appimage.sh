#!/usr/bin/env bash
# cargo-packager 0.11.8 ships AppRun with owner-only execute permission.
# Repair the AppDir, preserving the original runtime and library layout.
set -euo pipefail

image="$(realpath "${1:?Usage: repair-linux-appimage.sh IMAGE}")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

chmod +x "$image"
offset="$(env -u APPIMAGE_EXTRACT_AND_RUN "$image" --appimage-offset)"
[[ "$offset" =~ ^[0-9]+$ && "$offset" -gt 0 ]]
head -c "$offset" "$image" > "$work/runtime"
(
  cd "$work"
  env -u APPIMAGE_EXTRACT_AND_RUN "$image" --appimage-extract >/dev/null
)
chmod 0755 "$work/squashfs-root/AppRun" "$work/squashfs-root/usr/bin/sourcefour"

# Pin both the tool release and its digest. Reuse our runtime so appimagetool
# cannot silently download a newer one while rebuilding the filesystem.
curl --fail --location --retry 3 \
  https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage \
  --output "$work/appimagetool"
echo "ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0  $work/appimagetool" | sha256sum --check
chmod 0755 "$work/appimagetool"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$work/appimagetool" \
  --runtime-file "$work/runtime" "$work/squashfs-root" "$work/repaired.AppImage"
chmod 0755 "$work/repaired.AppImage"
mv "$work/repaired.AppImage" "$image"
