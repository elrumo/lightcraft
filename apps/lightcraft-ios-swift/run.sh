#!/bin/sh
# Build LightCraft's SwiftUI app for iOS and run it with xtool (docs/ios-swift.md → Build and run).
#
#   ./run.sh                  build (Rust release), install and launch on the connected iPhone / iPad
#   ./run.sh --debug          a debug Rust build (assertions on; a much bigger, slower app)
#   ./run.sh --simulator      macOS only (xtool runs simulators only there): the booted simulator
#   ./run.sh --build-only     only the .app (xtool dev build), in LightCraft/xtool/
#   ./run.sh --ipa            an .ipa to install elsewhere (xtool dev build --ipa)
#   ./run.sh --test           macOS: the Rust library for this Mac, then `swift test` (LightCraftCoreTests)
#
# Needs: rustup targets aarch64-apple-ios (device) / aarch64-apple-ios-sim (simulator) /
# aarch64-apple-darwin (--test), and xtool set up (`xtool setup`). Extra arguments go to xtool.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
package="$here/LightCraft"
profile=release
cargo_flag=--release
simulator=
mode=run
for arg in "$@"; do
    case "$arg" in
        --debug) profile=debug; cargo_flag= ;;
        --simulator) simulator=1 ;;
        --build-only) mode=build ;;
        --ipa) mode=ipa ;;
        --test) mode=test ;;
        -h | --help) sed -n '2,13p' "$0"; exit 0 ;;
        *) ;;
    esac
done
xtool_args=
for arg in "$@"; do
    case "$arg" in
        --debug | --simulator | --build-only | --ipa | --test) ;;
        *) xtool_args="$xtool_args $arg" ;;
    esac
done

if [ "$mode" = test ]; then
    triple=aarch64-apple-darwin
    [ "$(uname -m)" = x86_64 ] && triple=x86_64-apple-darwin
    export MACOSX_DEPLOYMENT_TARGET=26.0
elif [ -n "$simulator" ]; then
    case "$(uname -m)" in
        arm64 | aarch64) triple=aarch64-apple-ios-sim ;;
        *) triple=x86_64-apple-ios ;;
    esac
    export IPHONEOS_DEPLOYMENT_TARGET=26.0
else
    triple=aarch64-apple-ios
    export IPHONEOS_DEPLOYMENT_TARGET=26.0
fi

# the engine and its C ABI as a static library, for the platform being built
echo "run.sh: cargo build -p lightcraft-ios-swift --target $triple $cargo_flag"
(cd "$root" && cargo build -p lightcraft-ios-swift --target "$triple" $cargo_flag)
mkdir -p "$package/.rust"
cp -f "${CARGO_TARGET_DIR:-$root/target}/$triple/$profile/liblightcraft_bridge.a" "$package/.rust/liblightcraft_bridge.a"

cd "$package"
# shellcheck disable=SC2086 # extra xtool arguments are split on purpose
case "$mode" in
    test) swift test ;;
    build) xtool dev build $xtool_args ;;
    ipa) xtool dev build --ipa $xtool_args ;;
    *)
        if [ -n "$simulator" ]; then
            xtool dev run --simulator $xtool_args
        else
            xtool dev run $xtool_args
        fi
        ;;
esac
