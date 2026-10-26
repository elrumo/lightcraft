#!/bin/sh
# Build and run tools/apple_raw_compare.swift (macOS 27 SDK for CIRAWFilter decoder 9). Arguments pass through.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
bin="${APPLE_RAW_COMPARE_BIN:-${TMPDIR:-/tmp}/apple_raw_compare}"
if [ ! -x "$bin" ] || [ "$here/apple_raw_compare.swift" -nt "$bin" ]; then
  swiftc -O -swift-version 5 -o "$bin" "$here/apple_raw_compare.swift"
fi
exec "$bin" "$@"
