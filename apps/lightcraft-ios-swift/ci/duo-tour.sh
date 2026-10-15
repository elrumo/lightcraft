#!/bin/sh
# Run the SwiftUI app on an iPhone Duo simulator with an automation script and capture the live display each time
# the script takes a screenshot of its own: unlike `ui.screenshot` (the window only), the capture shows what the
# system adds on a Duo, the side column with the status bar, the back button and the toolbar items.
#
#   ci/duo-tour.sh <udid> <LightCraft.app> <out-dir> [script.jsonl]      (default script: ci/duo.jsonl)
#
# For each `ui.screenshot {path: "x.png"}` in the script, <out-dir> gets x.png (the app's own) and x-display.png (the
# display, whichever of the two is on: the outer display 466 x 678 pt, or the inner one 669 x 951 pt). The script follows
# each screenshot with `{"method": "ui.wait", "params": {"path": "x.done"}}`: this creates x.done once the display is
# captured, so the screen stays as it is until then, however slow the machine is.
# The reply to every request, `ui.inspect`'s `layout` among them, is in <out-dir>/tour.out.
#
# The fold, opening and closing are Device Hub's: this runs on whatever pose the simulator is in. The script can ask for
# the outer display on its side (`ui.set {orientation}`) and make up a hinge (`ui.set {fold}`), so the layouts for a
# book or a tabletop can be seen without one. macOS with Xcode 27.1 only.
set -eu
udid=$1
app=$2
out=$3
script=${4:-$(dirname "$0")/duo.jsonl}
bundle=ai.storyteller.lightcraft.swift
mkdir -p "$out"

xcrun simctl boot "$udid" 2> /dev/null || true
xcrun simctl bootstatus "$udid" -b
xcrun simctl install "$udid" "$app"
data=$(xcrun simctl get_app_container "$udid" "$bundle" data)
mkdir -p "$data/tmp"
cp -f "$script" "$data/tmp/tour.jsonl"
rm -f "$data/tmp/tour.out" "$data/tmp/"*.png "$data/tmp/"*.done
SIMCTL_CHILD_LIGHTCRAFT_DEMO=1 SIMCTL_CHILD_LIGHTCRAFT_SCRIPT=tour.jsonl xcrun simctl launch --terminate-running-process "$udid" "$bundle"

for name in $(grep -o '"path": *"[^"]*\.png"' "$script" | sed 's/.*"\([^"]*\)"$/\1/'); do
    i=0
    while [ ! -s "$data/tmp/$name" ]; do
        i=$((i + 1))
        if [ "$i" -gt "${TOUR_SECONDS:-180}" ]; then
            echo "the script never took $name" >&2
            break
        fi
        sleep 1
    done
    [ -s "$data/tmp/$name" ] || continue
    sleep 1
    # which display is on: the app's window is 466 x 678 pt at most on the outer one (3x: 1398 x 2034 px)
    w=$(sips -g pixelWidth "$data/tmp/$name" | awk '/pixelWidth/ {print $2}')
    h=$(sips -g pixelHeight "$data/tmp/$name" | awk '/pixelHeight/ {print $2}')
    short=$((w < h ? w : h))
    long=$((w < h ? h : w))
    display=primary-1
    if [ "$short" -le 1410 ] && [ "$long" -le 2046 ]; then
        display=primary
    fi
    cp -f "$data/tmp/$name" "$out/$name"
    xcrun simctl io "$udid" screenshot --display="$display" --mask=ignored "$out/${name%.png}-display.png" > /dev/null 2>&1 || true
    touch "$data/tmp/${name%.png}.done"
    echo "$name: window ${w}x${h} px, display $display"
done

want=$(grep -c . "$script")
i=0
while [ "$(grep -c . "$data/tmp/tour.out" 2> /dev/null || echo 0)" -lt "$want" ]; do
    i=$((i + 1))
    if [ "$i" -gt "${TOUR_SECONDS:-180}" ]; then
        echo "the script did not finish in ${TOUR_SECONDS:-180} seconds" >&2
        break
    fi
    sleep 1
done
cp -f "$data/tmp/tour.out" "$out/" 2> /dev/null || true
# every request answered without an error
[ -f "$out/tour.out" ] && ! grep -q '"ok":false' "$out/tour.out"
