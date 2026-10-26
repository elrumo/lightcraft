#!/bin/sh
# Installs the LightCraft sync server on a Mac and starts it (at every login). See docs/self-hosting.md.
#
#   curl -fsSL https://raw.githubusercontent.com/elrumo/lightcraft/main/apps/lightcraft-server/install.sh | sh
#
# A specific version: ... | sh -s -- 0.14.0        Remove it again: lightcraft-server uninstall (or see the guide)
#
# It downloads the zip for this Mac from the GitHub release, checks its checksum, and runs `lightcraft-server install`
# (the program and the web app go to ~/Library/Application Support/LightCraft Server, which a downloaded-by-curl
# file needs no Gatekeeper approval for).
set -eu

repo=elrumo/lightcraft
[ "$(uname -s)" = Darwin ] || { echo "This installer is for macOS. For Linux, Windows and Docker see https://github.com/$repo/blob/main/docs/self-hosting.md" >&2; exit 1; }
case "$(uname -m)" in
  arm64) system=macos-arm64 ;;
  x86_64) system=macos-x86_64 ;;
  *) echo "unsupported processor: $(uname -m)" >&2; exit 1 ;;
esac

want=${1:-}
api=$(curl -fsSL "https://api.github.com/repos/$repo/releases?per_page=30") || { echo "can't reach GitHub" >&2; exit 1; }
# the newest published server release (tag server-v<version>), or the one asked for
tag=$(printf '%s' "$api" | grep -o '"tag_name": *"server-v[^"]*"' | sed 's/.*"server-v\([^"]*\)"/\1/' | { if [ -n "$want" ]; then grep -Fx "$want"; else head -1; fi; } || true)
[ -n "$tag" ] || { echo "no server release found${want:+ for $want}: https://github.com/$repo/releases" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
zip="lightcraft-server-$tag-$system.zip"
base="https://github.com/$repo/releases/download/server-v$tag"
echo "Downloading LightCraft Server $tag for $system..."
curl -fsSL -o "$tmp/$zip" "$base/$zip"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS-server.txt"
(cd "$tmp" && grep " $zip\$" SHA256SUMS | shasum -a 256 -c -) >/dev/null || { echo "the download does not match its checksum" >&2; exit 1; }

unzip -q "$tmp/$zip" -d "$tmp"
"$tmp/lightcraft-server-$tag-$system/lightcraft-server" install
