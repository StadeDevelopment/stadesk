#!/usr/bin/env bash
set -euo pipefail

. "$(dirname "$0")/env.sh"
cd "$root"
exec cargo "$@" --target x86_64-pc-windows-gnu
