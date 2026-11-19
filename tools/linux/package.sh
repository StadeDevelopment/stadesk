#!/usr/bin/env bash
set -euo pipefail

multiarch="$(dpkg-architecture -qDEB_HOST_MULTIARCH)"
export VPX_LIB_DIR="/usr/lib/$multiarch"
export VPX_INCLUDE_DIR=/usr/include
export VPX_VERSION="$(pkg-config --modversion vpx)"
export VPX_STATIC=1

cd /work/app
cargo tauri build --bundles "${1:-deb}" --config '{"build":{"beforeBuildCommand":""}}'
mkdir -p /work/target/dist/linux
find "$CARGO_TARGET_DIR/release/bundle" \( -name '*.deb' -o -name '*.AppImage' \) -exec cp {} /work/target/dist/linux/ \;
ls -la /work/target/dist/linux
