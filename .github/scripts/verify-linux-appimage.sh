#!/usr/bin/env bash
# Inspect the finished filesystem, not just the outer AppImage runtime.
set -euo pipefail

image="$(realpath "${1:?Usage: verify-linux-appimage.sh IMAGE}")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
(
  cd "$work"
  env -u APPIMAGE_EXTRACT_AND_RUN "$image" --appimage-extract >/dev/null
)
root="$work/squashfs-root"
for entry in AppRun usr/bin/sourcefour; do
  target="$(realpath -e "$root/$entry")"
  if [[ "$target" != "$root/"* || ! -f "$target" ]]; then
    echo "error: $entry must resolve to a file inside the AppImage" >&2
    exit 1
  fi
  mode="$(stat -c '%a' "$target")"
  # test -x alone passes for 0744 when extraction makes us the file owner.
  if (( (8#$mode & 0555) != 0555 )); then
    echo "error: $entry has mode $mode; all users need read/execute permission" >&2
    exit 1
  fi
done

# Check every payload ELF, including libraries, against the supported baseline.
while IFS= read -r -d '' entry; do
  if file -b "$entry" | grep -q '^ELF '; then
    objdump -T "$entry" > "$work/symbols"
    grep '\*UND\*' "$work/symbols" | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' >> "$work/versions" || true
  fi
done < <(find "$root" -type f -print0)
max_glibc="$(sort -Vu "$work/versions" | tail -n 1)"
if [[ "$(printf '%s\n' GLIBC_2.35 "$max_glibc" | sort -V | tail -n 1)" != GLIBC_2.35 ]]; then
  echo "error: AppImage requires $max_glibc, newer than the GLIBC_2.35 baseline" >&2
  exit 1
fi

# Traverse AppRun and the dynamic loader without needing a GPU. GUI startup
# is tested separately; --help returns before GPUI initializes its renderer.
timeout 30 env APPIMAGE_EXTRACT_AND_RUN=1 "$image" --help
echo "AppImage launcher permissions and glibc baseline verified ($max_glibc)"
