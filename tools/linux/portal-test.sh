#!/usr/bin/env bash
set -euo pipefail

examples="$1"
output="$2"

export XDG_RUNTIME_DIR="$(mktemp -d)"
chmod 700 "$XDG_RUNTIME_DIR"
export XDG_CURRENT_DESKTOP=stadesktest
export XDG_SESSION_TYPE=wayland
export WAYLAND_DISPLAY=wayland-mock
export MOCK_PORTAL_LOG="$output/mock-portal.jsonl"
rm -f "$MOCK_PORTAL_LOG"

portals="$(mktemp -d)"
printf '[portal]\nDBusName=org.freedesktop.impl.portal.desktop.stadesktest\nInterfaces=org.freedesktop.impl.portal.ScreenCast;org.freedesktop.impl.portal.RemoteDesktop;\nUseIn=stadesktest\n' > "$portals/stadesktest.portal"
export XDG_DESKTOP_PORTAL_DIR="$portals"
config="$(mktemp -d)"
mkdir -p "$config/xdg-desktop-portal"
printf '[preferred]\ndefault=stadesktest\n' > "$config/xdg-desktop-portal/portals.conf"
export XDG_CONFIG_HOME="$config"

pipewire >/tmp/pipewire.log 2>&1 &
sleep 1
wireplumber >/tmp/wireplumber.log 2>&1 &
python3 /work/tools/linux/mock-portal.py >/tmp/mock-portal.log 2>&1 &
sleep 1
/usr/libexec/xdg-desktop-portal -r >/tmp/portal.log 2>&1 &
sleep 2

status=0
"$examples/capture_check" --save "$output/portal.bmp" || status=$?
echo "capture exit=$status"
if [ "$status" -eq 0 ]; then
    "$examples/input_check" || status=$?
    echo "input exit=$status"
fi
echo "== mock portal log"
cat "$MOCK_PORTAL_LOG" || true
if [ "$status" -ne 0 ]; then
    for log in /tmp/mock-portal.log /tmp/portal.log /tmp/pipewire.log /tmp/wireplumber.log; do
        echo "== $log"
        tail -n 30 "$log"
    done
fi
exit "$status"
