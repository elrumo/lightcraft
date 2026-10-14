#!/bin/sh
# Build LightCraft's SwiftUI app and run it (docs/ios-swift.md → Build and run). iOS goes through
# xtool; the Mac app through the Xcode project (xtool builds iOS apps only).
#
#   ./run.sh                  build (Rust release), install and launch on the connected iPhone / iPad
#   ./run.sh --debug          a debug Rust build (assertions on; a much bigger, slower app)
#   ./run.sh --simulator      macOS only (xtool runs simulators only there): the booted simulator
#   ./run.sh --build-only     only the .app (xtool dev build), in LightCraft/xtool/
#   ./run.sh --ipa            an .ipa to install elsewhere (xtool dev build --ipa)
#   ./run.sh --test           macOS: the Rust library for this Mac, then `swift test` (LightCraftCoreTests)
#   ./run.sh --mac            macOS: build the Mac app (xcodebuild; its build phase runs cargo) and open it.
#                             With --build-only, only build it. LIGHTCRAFT_LIBRARY=<folder> or LIGHTCRAFT_DEMO=1
#                             in the environment picks the library it opens (the default is the egui app's,
#                             ~/Pictures/LightCraft Library)
#
# Needs: rustup targets aarch64-apple-ios (device) / aarch64-apple-ios-sim (simulator) /
# aarch64-apple-darwin (--test, --mac), and xtool set up (`xtool setup`). Extra arguments go to
# xtool (xcodebuild with --mac).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
package="$here/LightCraft"
profile=release
cargo_flag=--release
simulator=
mode=run
mac=
build_only=
for arg in "$@"; do
    case "$arg" in
        --debug) profile=debug; cargo_flag= ;;
        --simulator) simulator=1 ;;
        --build-only) mode=build; build_only=1 ;;
        --ipa) mode=ipa ;;
        --test) mode="test" ;;
        --mac) mac=1 ;;
        -h | --help) sed -n '2,19p' "$0"; exit 0 ;;
        *) ;;
    esac
done
xtool_args=
for arg in "$@"; do
    case "$arg" in
        --debug | --simulator | --build-only | --ipa | --test | --mac) ;;
        *) xtool_args="$xtool_args $arg" ;;
    esac
done

if [ -n "$mac" ]; then
    # the Xcode project's "Build Rust (cargo)" phase builds the library (xcode/build-rust.sh)
    config=Release
    [ "$profile" = debug ] && config=Debug
    derived="$package/.build/mac"
    echo "run.sh: xcodebuild -project xcode/LightCraft.xcodeproj -scheme LightCraft -destination platform=macOS -configuration $config"
    # shellcheck disable=SC2086 # extra xcodebuild arguments are split on purpose
    # (ARCHS: the package's targets don't see the app target's arm64-only setting and would build x86_64 too)
    xcodebuild -project "$here/xcode/LightCraft.xcodeproj" -scheme LightCraft -destination 'platform=macOS' \
        -configuration "$config" -derivedDataPath "$derived" ARCHS=arm64 $xtool_args build
    app="$derived/Build/Products/$config/LightCraft.app"
    echo "run.sh: $app"
    [ -n "$build_only" ] && exit 0
    # (`open` starts the app without this shell's environment: with a library chosen, run it directly)
    if [ -n "${LIGHTCRAFT_LIBRARY:-}${LIGHTCRAFT_DEMO:-}" ]; then
        exec "$app/Contents/MacOS/LightCraft"
    fi
    open "$app"
    exit 0
fi

slot=ios
if [ "$mode" = test ]; then
    triple=aarch64-apple-darwin
    [ "$(uname -m)" = x86_64 ] && triple=x86_64-apple-darwin
    export MACOSX_DEPLOYMENT_TARGET=26.0
    slot=macos
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
mkdir -p "$package/.rust/$slot"
cp -f "${CARGO_TARGET_DIR:-$root/target}/$triple/$profile/liblightcraft_bridge.a" "$package/.rust/$slot/liblightcraft_bridge.a"

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
