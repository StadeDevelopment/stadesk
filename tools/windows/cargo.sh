#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
libvpx="$("$root/tools/windows/build-libvpx.sh")"
toolchain="$(x86_64-w64-mingw32-gcc -print-sysroot)"

export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
export VPX_LIB_DIR="$libvpx/lib"
export VPX_INCLUDE_DIR="$libvpx/include"
export VPX_VERSION="1.17.0"
export VPX_STATIC=1
export BINDGEN_EXTRA_CLANG_ARGS_x86_64_pc_windows_gnu="--target=x86_64-pc-windows-gnu -I$toolchain/x86_64-w64-mingw32/include"

cd "$root"
exec cargo "$@" --target x86_64-pc-windows-gnu
