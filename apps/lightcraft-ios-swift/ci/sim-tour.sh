#!/bin/sh
# Run the SwiftUI app on an iOS simulator with an automation script (docs/ios-swift.md → Driving the
# app) and copy its replies and screenshots out: what CI shows of the running app.
#
#   ci/sim-tour.sh <LightCraft.app> <out-dir> [script.jsonl]
#
# Boots the first available iPhone simulator, installs the app, launches it with
# LIGHTCRAFT_SCRIPT (the script is copied into the app's tmp folder), waits for the replies and
# copies tmp/*.png and the .out file to <out-dir>. macOS with Xcode only.
set -eu
app=$1
out=$2
script=${3:-$(dirname "$0")/tour.jsonl}
bundle=ai.storyteller.lightcraft.swift
mkdir -p "$out"

udid=$(xcrun simctl list devices available -j | python3 -c '
import json, sys
d = json.load(sys.stdin)["devices"]
ios = sorted((k for k in d if "iOS" in k), reverse=True)
print(next(x["udid"] for k in ios for x in d[k] if x["name"].startswith("iPhone")))
')
echo "simulator: $udid"
xcrun simctl boot "$udid" 2>/dev/null || true
xcrun simctl bootstatus "$udid" -b
xcrun simctl install "$udid" "$app"
# the data container exists once the app is installed: put the script in its tmp folder
data=$(xcrun simctl get_app_container "$udid" "$bundle" data)
mkdir -p "$data/tmp"
cp -f "$script" "$data/tmp/tour.jsonl"
rm -f "$data/tmp/tour.out"
SIMCTL_CHILD_LIGHTCRAFT_SCRIPT=tour.jsonl xcrun simctl launch "$udid" "$bundle"
want=$(grep -c . "$script")
i=0
while [ "$(grep -c . "$data/tmp/tour.out" 2>/dev/null || echo 0)" -lt "$want" ]; do
    i=$((i + 1))
    if [ "$i" -gt 180 ]; then
        echo "the script did not finish in 3 minutes" >&2
        break
    fi
    sleep 1
done
cp -f "$data/tmp/tour.out" "$out/" 2>/dev/null || true
cp -f "$data/tmp/"*.png "$out/" 2>/dev/null || true
cp -f "$data/tmp/lightcraft-panics.log" "$out/" 2>/dev/null || true
xcrun simctl spawn "$udid" log show --last 5m --predicate 'subsystem == "ai.storyteller.lightcraft"' > "$out/engine.log" 2>/dev/null || true
cat "$out/tour.out" 2>/dev/null || true
# every request answered without an error
[ -f "$out/tour.out" ] && ! grep -q '"ok":false' "$out/tour.out"
