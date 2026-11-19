#!/usr/bin/env bash
set -euo pipefail

bin="$1"
output="$2"
export XDG_RUNTIME_DIR="$(mktemp -d)"
chmod 700 "$XDG_RUNTIME_DIR"
export HOME="$(mktemp -d)"
export DISPLAY=:99
export WEBKIT_DISABLE_COMPOSITING_MODE=1

Xvfb :99 -screen 0 1600x1000x24 -nolisten tcp >/tmp/xvfb.log 2>&1 &
sleep 1
xsetroot -solid "#2a6f97"

data="$(mktemp -d)"
"$bin/stadesk-server" --listen 127.0.0.1:21300 --data-dir "$data" >/tmp/server.log 2>&1 &
for _ in $(seq 1 50); do
    grep -q "Server key" /tmp/server.log && break
    sleep 0.2
done
key="$(sed -n 's/^Server key: //p' /tmp/server.log)"
"$bin/examples/host_cli" 127.0.0.1:21300 "$key" >/tmp/host.log 2>&1 &
for _ in $(seq 1 50); do
    grep -q "online as" /tmp/host.log && break
    sleep 0.2
done
host="$(sed -n 's/^online as \([^ ]*\).*/\1/p' /tmp/host.log | head -n 1)"

config="$HOME/.local/share/dev.stade.stadesk"
mkdir -p "$config"
printf '{"serverAddress":"127.0.0.1:21300","serverKey":"%s","methods":{"acceptPrompt":true,"oneTimePassword":true,"permanentPassword":false,"trustedDevices":true},"permanentPassword":null,"trustedDevices":[],"language":"auto"}' "$key" > "$config/config.json"

"$bin/stadesk-app" --connect "$host" >/tmp/app.log 2>&1 &
sleep 15
"$bin/examples/capture_check" --save "$output/linux-app.bmp" >/tmp/capture.log 2>&1 || true
echo "== host"; cat /tmp/host.log
echo "== app"; tail -n 20 /tmp/app.log
echo "== capture"; tail -n 3 /tmp/capture.log
