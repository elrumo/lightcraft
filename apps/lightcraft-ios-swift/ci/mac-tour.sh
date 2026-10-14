#!/bin/sh
# Run the SwiftUI app on this Mac with an automation script (docs/ios-swift.md → Driving the app)
# and copy its replies and screenshots out: what CI shows of the running Mac app.
#
#   ci/mac-tour.sh <LightCraft.app> <out-dir> [script.jsonl]
#
# Starts the app's binary with LIGHTCRAFT_SCRIPT (an absolute path, so the script, its replies and
# its screenshots all stay in <out-dir>) on the in-memory demo library (LIGHTCRAFT_DEMO: never your
# own photos), waits for the replies and stops the app. It is driven only through its automation
# channel: no synthetic keys or clicks (CLAUDE.md). macOS with Xcode only.
set -eu
app=$1
out=$(mkdir -p "$2" && cd "$2" && pwd)
script=${3:-$(cd "$(dirname "$0")" && pwd)/tour.jsonl}
cp -f "$script" "$out/tour.jsonl"
rm -f "$out/tour.out" "$out"/*.png
want=$(grep -c . "$script")
LIGHTCRAFT_DEMO=1 LIGHTCRAFT_SCRIPT="$out/tour.jsonl" "$app/Contents/MacOS/LightCraft" > "$out/app.log" 2>&1 &
pid=$!
i=0
while [ "$(grep -c . "$out/tour.out" 2>/dev/null || echo 0)" -lt "$want" ]; do
    if ! kill -0 "$pid" 2>/dev/null; then
        echo "the app quit before the script finished" >&2
        break
    fi
    i=$((i + 1))
    if [ "$i" -gt 180 ]; then
        echo "the script did not finish in 3 minutes" >&2
        break
    fi
    sleep 1
done
kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
cp -f "$HOME/Library/Caches/ai.storyteller.lightcraft.swift/tmp/lightcraft-panics.log" "$out/" 2>/dev/null || true
cat "$out/tour.out" 2>/dev/null || true
# every request answered without an error
[ -f "$out/tour.out" ] && ! grep -q '"ok":false' "$out/tour.out"
