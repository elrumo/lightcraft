#!/bin/sh
# Build LightCraft for iOS and run it with xtool (docs/ios.md → "Build and run with xtool").
#
#   ./run.sh                  build (Rust release), install and launch on the connected iPhone / iPad
#   ./run.sh --debug          a debug Rust build (assertions on; a much bigger, slower app)
#   ./run.sh --simulator      macOS only (xtool runs simulators only there): the booted simulator
#   ./run.sh --build-only     only the .app (xtool dev build), in ./xtool/
#   ./run.sh --ipa            an .ipa to install elsewhere (xtool dev build --ipa)
#
# Needs: rustup targets aarch64-apple-ios (device) / aarch64-apple-ios-sim (simulator), and xtool set
# up (`xtool setup`: an Apple ID and the iOS SDK from Xcode.xip). Extra arguments go to xtool.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
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
        -h | --help) sed -n '2,13p' "$0"; exit 0 ;;
        *) ;;
    esac
done
xtool_args=
for arg in "$@"; do
    case "$arg" in
        --debug | --simulator | --build-only | --ipa) ;;
        *) xtool_args="$xtool_args $arg" ;;
    esac
done

triple=aarch64-apple-ios
if [ -n "$simulator" ]; then
    case "$(uname -m)" in
        arm64 | aarch64) triple=aarch64-apple-ios-sim ;;
        *) triple=x86_64-apple-ios ;;
    esac
fi

# the Rust app as a static library, for the iOS version the package targets
export IPHONEOS_DEPLOYMENT_TARGET=16.0
echo "run.sh: cargo build -p lightcraft-ios --target $triple $cargo_flag"
(cd "$root" && cargo build -p lightcraft-ios --target "$triple" $cargo_flag)
mkdir -p "$here/.rust"
cp -f "${CARGO_TARGET_DIR:-$root/target}/$triple/$profile/liblightcraft_ios.a" "$here/.rust/liblightcraft_ios.a"

cd "$here"
# shellcheck disable=SC2086 # extra xtool arguments are split on purpose
case "$mode" in
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
