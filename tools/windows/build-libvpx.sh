#!/usr/bin/env bash
set -euo pipefail

version="1.17.0"
sha256="1020f184046187baa2985dbde38e0691f49c44088bca7a1842b0236c6081dc0a"
root="$(cd "$(dirname "$0")/../.." && pwd)"
work="$root/target/windows-deps"
prefix="$work/libvpx"
archive="$work/libvpx-$version.tar.gz"

if [ -f "$prefix/lib/libvpx.a" ]; then
    echo "$prefix"
    exit 0
fi

mkdir -p "$work"
curl -sSfL -o "$archive" "https://codeload.github.com/webmproject/libvpx/tar.gz/refs/tags/v$version"
echo "$sha256  $archive" | shasum -a 256 -c - >/dev/null
rm -rf "$work/libvpx-$version" "$work/build"
tar -xzf "$archive" -C "$work"
mkdir -p "$work/build"
cd "$work/build"
CROSS=x86_64-w64-mingw32- "$work/libvpx-$version/configure" \
    --target=x86_64-win64-gcc \
    --prefix="$prefix" \
    --enable-static \
    --disable-shared \
    --disable-examples \
    --disable-tools \
    --disable-docs \
    --disable-unit-tests \
    --as=nasm >/dev/null
make -j"$(sysctl -n hw.ncpu 2>/dev/null || nproc)" >/dev/null
make install >/dev/null
echo "$prefix"
