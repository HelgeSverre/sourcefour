#!/usr/bin/env bash
# Run under xvfb-run with Openbox and Mesa's software Vulkan driver installed.
# A build alone cannot catch missing GPUI display backends (issue #4).
set -euo pipefail

binary="${1:-target/release/sourcefour}"
temporary_directory="$(mktemp -d)"
app_pid=""
wm_pid=""
trap '
  if [[ -n "$app_pid" ]]; then
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  if [[ -n "$wm_pid" ]]; then
    kill "$wm_pid" 2>/dev/null || true
    wait "$wm_pid" 2>/dev/null || true
  fi
  rm -rf "$temporary_directory"
' EXIT

# Force the X11 path and keep the smoke run's settings isolated.
unset WAYLAND_DISPLAY ZED_HEADLESS SOURCEFOUR_STARTUP_LOG
export XDG_RUNTIME_DIR="$temporary_directory/runtime"
export XDG_CONFIG_HOME="$temporary_directory/config"
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME"
chmod 700 "$XDG_RUNTIME_DIR"
openbox > "$temporary_directory/wm.log" 2>&1 &
wm_pid=$!
"$binary" --demo > "$temporary_directory/app.log" 2>&1 &
app_pid=$!

for ((attempt = 0; attempt < 60; attempt++)); do
  if ! kill -0 "$app_pid" 2>/dev/null; then
    cat "$temporary_directory/app.log" >&2
    echo "Sourcefour exited before opening its window" >&2
    exit 1
  fi
  if xwininfo -name Sourcefour 2>/dev/null | grep 'Map State: IsViewable' >/dev/null; then
    # Give rendering a chance to fail after the native window is created.
    sleep 3
    if kill -0 "$app_pid" 2>/dev/null; then
      echo "Sourcefour opened an X11 window and stayed running"
      exit 0
    fi
  fi
  sleep 1
done

cat "$temporary_directory/app.log" >&2
echo "Sourcefour did not open a window within 60 seconds" >&2
exit 1
