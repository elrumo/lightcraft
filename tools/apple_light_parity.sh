#!/bin/sh
# Build and run tools/apple_light_parity.swift (Core Image against the engine's Light controls). Arguments pass through.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
bin="${APPLE_LIGHT_PARITY_BIN:-${TMPDIR:-/tmp}/apple_light_parity}"
if [ ! -x "$bin" ] || [ "$here/apple_light_parity.swift" -nt "$bin" ]; then
  swiftc -O -swift-version 5 -o "$bin" "$here/apple_light_parity.swift"
fi
exec "$bin" "$@"
