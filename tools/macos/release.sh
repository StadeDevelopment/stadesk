#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
secrets="${STADESK_SIGNING_ENV:-$HOME/.stadesk/signing.env}"

if [ -f "$secrets" ]; then
    mode="$(stat -f '%Lp' "$secrets")"
    if [ "$mode" != "600" ]; then
        echo "refusing to read $secrets: its permissions are $mode, run chmod 600 on it" >&2
        exit 1
    fi
    set -a
    . "$secrets"
    set +a
    for name in APPLE_SIGNING_IDENTITY APPLE_API_ISSUER APPLE_API_KEY APPLE_API_KEY_PATH APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
        if [ -z "${!name:-}" ]; then
            unset "$name"
        fi
    done
else
    echo "no signing settings at $secrets, building an unsigned app" >&2
fi

cd "$root/app"
npm run tauri build -- --bundles app,dmg
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    codesign --verify --deep --strict --verbose=2 "$root/target/release/bundle/macos/StaDesk.app"
    spctl --assess --type execute --verbose "$root/target/release/bundle/macos/StaDesk.app" || true
fi
ls -la "$root/target/release/bundle/dmg"
