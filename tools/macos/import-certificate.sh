#!/usr/bin/env bash
set -euo pipefail

certificate="${1:?usage: import-certificate.sh PATH/TO/developerID_application.cer}"
key="$HOME/.stadesk/developer-id.key"
keychain="$HOME/Library/Keychains/login.keychain-db"

security import "$key" -k "$keychain" -T /usr/bin/codesign
security import "$certificate" -k "$keychain"
identity="$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -1)"
if [ -z "$identity" ]; then
    echo "the certificate did not pair with the key; check that it was made from StaDesk-DeveloperID.certSigningRequest" >&2
    exit 1
fi
signing="$HOME/.stadesk/signing.env"
tmp="$(mktemp "$HOME/.stadesk/signing.XXXXXX")"
grep -v '^APPLE_SIGNING_IDENTITY=' "$signing" > "$tmp" || true
printf 'APPLE_SIGNING_IDENTITY="%s"\n' "$identity" >> "$tmp"
chmod 600 "$tmp"
mv "$tmp" "$signing"
echo "signing identity: $identity"
