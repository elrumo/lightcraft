#!/bin/sh
# Xcode pre-build phase: builds the Rust static library (the engine and its C ABI) for the SDK and
# architecture being built and copies it where the app links it from: $BUILT_PRODUCTS_DIR and the
# Swift package's .rust/<ios|macos>/ (its linker search path for that platform: one folder each,
# so a Mac build never links an iOS library). Release configurations use --release.
set -eu
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
ROOT="$SRCROOT/../../.."
case "$PLATFORM_NAME" in
  iphoneos) TRIPLE=aarch64-apple-ios; SLOT=ios ;;
  iphonesimulator) SLOT=ios; case "$NATIVE_ARCH_ACTUAL" in arm64) TRIPLE=aarch64-apple-ios-sim ;; *) TRIPLE=x86_64-apple-ios ;; esac ;;
  # (arm64 only: the project sets ARCHS[sdk=macosx*]; x86_64 would be a second build and a lipo)
  macosx) TRIPLE=aarch64-apple-darwin; SLOT=macos ;;
  *) echo "unsupported platform $PLATFORM_NAME" >&2; exit 1 ;;
esac
export IPHONEOS_DEPLOYMENT_TARGET=26.0 MACOSX_DEPLOYMENT_TARGET=26.0
PROFILE=debug; FLAG=
if [ "$CONFIGURATION" = Release ]; then PROFILE=release; FLAG=--release; fi
cd "$ROOT"
cargo build -p lightcraft-ios-swift --target "$TRIPLE" $FLAG
LIB="${CARGO_TARGET_DIR:-target}/$TRIPLE/$PROFILE/liblightcraft_bridge.a"
mkdir -p "$BUILT_PRODUCTS_DIR" "$SRCROOT/../LightCraft/.rust/$SLOT"
/bin/cp -f "$LIB" "$BUILT_PRODUCTS_DIR/liblightcraft_bridge.a"
/bin/cp -f "$LIB" "$SRCROOT/../LightCraft/.rust/$SLOT/liblightcraft_bridge.a"
