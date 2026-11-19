#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root/app"
npm run -s build
"$root/tools/windows/cargo.sh" build --release -p stadesk-app --features tauri/custom-protocol
release="$root/target/x86_64-pc-windows-gnu/release"
stage="$root/target/dist/StaDesk-windows-x64"
rm -rf "$stage"
mkdir -p "$stage"
x86_64-w64-mingw32-strip -o "$stage/StaDesk.exe" "$release/stadesk-app.exe"
cp "$release/WebView2Loader.dll" "$stage/"
cd "$root/target/dist"
rm -f StaDesk-windows-x64.zip
zip -qr StaDesk-windows-x64.zip StaDesk-windows-x64
echo "$root/target/dist/StaDesk-windows-x64.zip"
