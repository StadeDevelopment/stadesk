#!/usr/bin/env bash
set -euo pipefail

binary="$1"
shift

export XDG_RUNTIME_DIR="$(mktemp -d)"
chmod 700 "$XDG_RUNTIME_DIR"
export XDG_CURRENT_DESKTOP=sway
export XDG_SESSION_TYPE=wayland
export WLR_BACKENDS=headless
export WLR_RENDERER=pixman
export WLR_LIBINPUT_NO_DEVICES=1

config="$(mktemp -d)"
mkdir -p "$config/xdg-desktop-portal" "$config/xdg-desktop-portal-wlr"
printf '[preferred]\ndefault=wlr\n' > "$config/xdg-desktop-portal/sway-portals.conf"
printf '[screencast]\nchooser_type=none\noutput_name=HEADLESS-1\n' > "$config/xdg-desktop-portal-wlr/config"
printf 'output HEADLESS-1 resolution 1280x720 bg #203040 solid_color\n' > "$config/sway.conf"
export XDG_CONFIG_HOME="$config"

pipewire >/tmp/pipewire.log 2>&1 &
sleep 1
wireplumber >/tmp/wireplumber.log 2>&1 &
sway -c "$config/sway.conf" >/tmp/sway.log 2>&1 &
for _ in $(seq 1 50); do
    socket="$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -name 'wayland-*' ! -name '*.lock' | head -n 1)"
    [ -n "$socket" ] && break
    sleep 0.1
done
export WAYLAND_DISPLAY="$(basename "$socket")"
export SWAYSOCK="$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -name 'sway-ipc.*' | head -n 1)"
dbus-update-activation-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP XDG_CONFIG_HOME XDG_RUNTIME_DIR SWAYSOCK
/opt/xdpw/libexec/xdg-desktop-portal-wlr -l DEBUG >/tmp/portal-wlr.log 2>&1 &
/usr/libexec/xdg-desktop-portal -r >/tmp/portal.log 2>&1 &
sleep 2

(
    for step in $(seq 1 40); do
        swaymsg -q output HEADLESS-1 bg "#$(printf '%02x' $((step * 6)))3060" solid_color || true
        sleep 0.2
    done
) &

status=0
"$binary" "$@" || status=$?
echo "exit=$status"
if [ "$status" -ne 0 ]; then
    for log in /tmp/sway.log /tmp/portal.log /tmp/portal-wlr.log /tmp/pipewire.log; do
        echo "== $log"
        tail -n 40 "$log"
    done
fi
exit "$status"
