#!/bin/sh
# Xcode pre-build phase: builds the Rust static library for the SDK/arch being built and copies it
# to $BUILT_PRODUCTS_DIR/liblightcraft_ios.a. Release configs use --release.
set -eu
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
ROOT="$SRCROOT/../../.."
case "$PLATFORM_NAME" in
  iphoneos) TRIPLE=aarch64-apple-ios ;;
  iphonesimulator) case "$NATIVE_ARCH_ACTUAL" in arm64) TRIPLE=aarch64-apple-ios-sim ;; *) TRIPLE=x86_64-apple-ios ;; esac ;;
  *) echo "unsupported platform $PLATFORM_NAME" >&2; exit 1 ;;
esac
PROFILE=debug; FLAG=
if [ "$CONFIGURATION" = Release ]; then PROFILE=release; FLAG=--release; fi
cd "$ROOT"
cargo build -p lightcraft-ios --target "$TRIPLE" $FLAG
mkdir -p "$BUILT_PRODUCTS_DIR"
/bin/cp -f "target/$TRIPLE/$PROFILE/liblightcraft_ios.a" "$BUILT_PRODUCTS_DIR/liblightcraft_ios.a"
