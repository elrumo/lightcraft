#!/bin/sh
# Xcode pre-build phase: builds the Rust static library (the engine and its C ABI) for the SDK and
# architecture being built and copies it where the app links it from: $BUILT_PRODUCTS_DIR and the
# Swift package's .rust/ (its linker search path). Release configurations use --release.
set -eu
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
ROOT="$SRCROOT/../../.."
case "$PLATFORM_NAME" in
  iphoneos) TRIPLE=aarch64-apple-ios ;;
  iphonesimulator) case "$NATIVE_ARCH_ACTUAL" in arm64) TRIPLE=aarch64-apple-ios-sim ;; *) TRIPLE=x86_64-apple-ios ;; esac ;;
  *) echo "unsupported platform $PLATFORM_NAME" >&2; exit 1 ;;
esac
export IPHONEOS_DEPLOYMENT_TARGET=26.0
PROFILE=debug; FLAG=
if [ "$CONFIGURATION" = Release ]; then PROFILE=release; FLAG=--release; fi
cd "$ROOT"
cargo build -p lightcraft-ios-swift --target "$TRIPLE" $FLAG
LIB="${CARGO_TARGET_DIR:-target}/$TRIPLE/$PROFILE/liblightcraft_bridge.a"
mkdir -p "$BUILT_PRODUCTS_DIR" "$SRCROOT/../LightCraft/.rust"
/bin/cp -f "$LIB" "$BUILT_PRODUCTS_DIR/liblightcraft_bridge.a"
/bin/cp -f "$LIB" "$SRCROOT/../LightCraft/.rust/liblightcraft_bridge.a"
