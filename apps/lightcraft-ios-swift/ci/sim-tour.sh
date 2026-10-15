#!/bin/sh
# Run the SwiftUI app on an iOS simulator with an automation script (docs/ios-swift.md → Driving the
# app) and copy its replies and screenshots out: what CI shows of the running app.
#
#   ci/sim-tour.sh <LightCraft.app> <out-dir> [script.jsonl]
#
# Boots a simulator, installs the app, launches it with LIGHTCRAFT_SCRIPT (the script is copied into the
# app's tmp folder), waits for the replies and copies tmp/*.png and the .out file to <out-dir>. macOS with
# Xcode only.
#
# Which simulator: SIM_UDID, else the first one whose name starts with SIM_NAME, else the first iPhone of the
# newest runtime that is not an iPhone Duo (a Duo is checked by ci/duo-tour.sh, on a simulator of its own).
# TOUR_SECONDS (default 180) is how long to wait for the script to finish.
set -eu
app=$1
out=$2
script=${3:-$(dirname "$0")/tour.jsonl}
bundle=ai.storyteller.lightcraft.swift
mkdir -p "$out"

if [ -n "${SIM_UDID:-}" ]; then
    udid=$SIM_UDID
else
    udid=$(SIM_NAME="${SIM_NAME:-}" xcrun simctl list devices available -j | python3 -c '
import json, os, sys
d = json.load(sys.stdin)["devices"]
want = os.environ.get("SIM_NAME", "")
ios = sorted((k for k in d if "iOS" in k), reverse=True)
def ok(name):
    return name.startswith(want) if want else (name.startswith("iPhone") and "Duo" not in name)
print(next(x["udid"] for k in ios for x in d[k] if ok(x["name"])))
')
fi
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
    if [ "$i" -gt "${TOUR_SECONDS:-180}" ]; then
        echo "the script did not finish in ${TOUR_SECONDS:-180} seconds" >&2
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
