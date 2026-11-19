#!/bin/sh
# Builds LightBites Server: the LightCraft sync server with brand.env's name, links, icon and telemetry, saved as
# `lightbites-server` beside cargo's `lightcraft-server` (and, on a Mac, the menu-bar app).
#
#   packaging/lightbites-server/build.sh [--target TRIPLE] [other cargo build options]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
set -a
. "$here/brand.env"
set +a
target="" prev=""
for a in "$@"; do
  [ "$prev" = --target ] && target=$a
  prev=$a
done
cd "$root"
if [ "$(uname -s)" = Darwin ]; then
  cargo build --release --locked -p lightcraft-server -p "$LIGHTCRAFT_SERVER_MENUBAR" "$@"
else
  cargo build --release --locked -p lightcraft-server "$@"
fi
out="${CARGO_TARGET_DIR:-$root/target}/${target:+$target/}release"
ext=""
[ -f "$out/lightcraft-server.exe" ] && ext=.exe
cp -f "$out/lightcraft-server$ext" "$out/$LIGHTCRAFT_SERVER_COMMAND$ext"
echo "$out/$LIGHTCRAFT_SERVER_COMMAND$ext"
