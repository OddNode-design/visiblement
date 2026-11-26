#!/usr/bin/env bash
# Opens a generated test video in the built app on a CI Mac, then saves what the screen shows and
# mpv's log, so the result can be checked without a Mac. Fails if the app does not stay up.
set -uo pipefail
cd "$(dirname "$0")/../.."
OUT=dist/smoke
mkdir -p "$OUT"
APP=dist/Visiblement.app

ffmpeg -v error -y -f lavfi -i "testsrc2=size=1280x720:rate=25:duration=20" \
  -f lavfi -i "sine=frequency=440:duration=20" -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest "$OUT/test.mp4"

# 1. Straight from the binary, the file as an argument.
VISIBLEMENT_LOG="$PWD/$OUT/mpv-arg.log" "$APP/Contents/MacOS/visiblement" "$PWD/$OUT/test.mp4" \
  >"$OUT/stdout-arg.txt" 2>&1 &
PID=$!
sleep 10
screencapture -x "$OUT/1-argument.png" || true
alive=0; kill -0 $PID 2>/dev/null && alive=1
kill $PID 2>/dev/null; sleep 1

# 2. The way Finder does it: LaunchServices, the file comes as an Apple Event.
open -a "$PWD/$APP" "$PWD/$OUT/test.mp4"
sleep 10
screencapture -x "$OUT/2-open.png" || true
pgrep -x visiblement >/dev/null && open_alive=1 || open_alive=0
pkill -x visiblement; sleep 1

echo "running after 10 s: argument=$alive open=$open_alive" | tee "$OUT/result.txt"
[ $alive = 1 ] && [ $open_alive = 1 ]
