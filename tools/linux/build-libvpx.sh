#!/usr/bin/env bash
set -euo pipefail

version="1.17.0"
sha256="1020f184046187baa2985dbde38e0691f49c44088bca7a1842b0236c6081dc0a"
root="$(cd "$(dirname "$0")/../.." && pwd)"
work="$root/target/linux-deps"
prefix="$work/libvpx"
archive="$work/libvpx-$version.tar.gz"

if [ -f "$prefix/lib/libvpx.a" ]; then
    echo "$prefix"
    exit 0
fi

mkdir -p "$work"
curl -sSfL -o "$archive" "https://codeload.github.com/webmproject/libvpx/tar.gz/refs/tags/v$version"
echo "$sha256  $archive" | sha256sum -c - >/dev/null
rm -rf "$work/libvpx-$version" "$work/build"
tar -xzf "$archive" -C "$work"
mkdir -p "$work/build"
cd "$work/build"
"$work/libvpx-$version/configure" \
    --prefix="$prefix" \
    --enable-static \
    --disable-shared \
    --enable-pic \
    --disable-examples \
    --disable-tools \
    --disable-docs \
    --disable-unit-tests >/dev/null
make -j"$(nproc)" >/dev/null
make install >/dev/null
echo "$prefix"
