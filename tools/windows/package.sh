#!/usr/bin/env bash
set -euo pipefail

. "$(dirname "$0")/env.sh"
makensis="$(command -v makensis)"
nsis_data="$(cd "$(dirname "$(realpath "$makensis")")/../share/nsis" && pwd)"
wrappers="$root/target/windows-deps/bin"
mkdir -p "$wrappers"
printf '#!/bin/sh\nNSISDIR="%s" exec "%s" "$@"\n' "$nsis_data" "$makensis" > "$wrappers/makensis"
chmod +x "$wrappers/makensis"
export PATH="$wrappers:$PATH"
cd "$root/app"
npm run tauri build -- --target x86_64-pc-windows-gnu --bundles nsis
find "$root/target/x86_64-pc-windows-gnu/release/bundle/nsis" -name '*-setup.exe*'
