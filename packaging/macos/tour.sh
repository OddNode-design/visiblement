#!/usr/bin/env bash
# A filmed tour of the built app on a CI Mac (nobody on the team has one): plays a video, opens the
# context menu, the settings, Convertir and Infos, resizes the window, goes fullscreen and back,
# with screenshots at each stop and a recording of the whole screen. Run after build-dmg.sh.
set -uo pipefail
cd "$(dirname "$0")/../.."
OUT=dist/tour
mkdir -p "$OUT"
APP=dist/Visiblement.app

# cliclick moves the mouse and types; macOS lets it only with the Accessibility permission (GitHub's
# Macs run without System Integrity Protection, so it can be written into the TCC database).
brew list cliclick >/dev/null 2>&1 || brew install cliclick >/dev/null
CLICK=$(command -v cliclick)
db="/Library/Application Support/com.apple.TCC/TCC.db"; now=$(date +%s)
for service in kTCCServiceAccessibility kTCCServicePostEvent; do
  sudo sqlite3 "$db" "INSERT OR REPLACE INTO access VALUES('$service','$CLICK',1,2,4,1,NULL,NULL,0,'UNUSED',NULL,0,$now,NULL,NULL,'UNUSED',0);" || true
done

[ -f dist/smoke/test.mp4 ] || ffmpeg -v error -y -f lavfi -i "testsrc2=size=1280x720:rate=25:duration=60" \
  -f lavfi -i "sine=frequency=440:duration=60" -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest dist/smoke/test.mp4

VISIBLEMENT_LOG="$PWD/$OUT/mpv.log" "$APP/Contents/MacOS/visiblement" "$PWD/dist/smoke/test.mp4" 2>"$OUT/app.txt" &
APP_PID=$!
sleep 6
osascript -e 'tell application "System Events" to set frontmost of (first process whose unix id is '"$APP_PID"') to true' 2>/dev/null || true

# The window, from the app's own log (physical pixels; scale 1 on these Macs): x, y, width, height.
read -r X Y W H < <(grep -o 'ui ([0-9-]*, [0-9-]*, [0-9]*, [0-9]*)' "$OUT/app.txt" | tail -1 | tr -d 'ui(),' )
echo "window: $X $Y $W $H"
CX=$((X + W / 2)); CY=$((Y + H / 2))

screencapture -x -v -V 75 "$OUT/tour.mov" &
REC=$!
sleep 1
n=0
shot() { n=$((n + 1)); sleep "${2:-1.2}"; screencapture -x "$OUT/$(printf %02d $n)-$1.png"; }

"$CLICK" m:$CX,$CY;                         shot playing 1.5
"$CLICK" rc:$((CX - 150)),$((CY - 120));    shot context-menu
"$CLICK" kp:esc;                            shot closed 0.8
# Player bar: the gear is the 4th of the 5 buttons of the right group (36 px each, 16 px margin).
"$CLICK" m:$((X + W - 90)),$((Y + H - 33)); sleep 0.5
"$CLICK" c:$((X + W - 90)),$((Y + H - 33)); shot settings
"$CLICK" kp:esc
# Title bar menus, centred: Convertir and Infos sit right of the centre.
"$CLICK" c:$((CX + 47)),$((Y + 19));        shot convert 1.6
"$CLICK" kp:esc
"$CLICK" c:$((CX + 111)),$((Y + 19));       shot info 1.6
"$CLICK" kp:esc
"$CLICK" c:$((CX - 106)),$((Y + 19));       shot media-menu
"$CLICK" kp:esc
# Resize from the bottom-right corner.
"$CLICK" dd:$((X + W - 2)),$((Y + H - 2)) dm:$((X + W - 150)),$((Y + H - 100)) du:$((X + W - 200)),$((Y + H - 140))
shot resized 1.5
# Fullscreen and back (F), pause with space, frame steps with ; and ,
"$CLICK" t:f;                               shot fullscreen 2
"$CLICK" t:f;                               shot windowed 2
"$CLICK" kp:space;                          shot paused
"$CLICK" t:';' t:';' t:',';                 shot frame-steps

sleep 2
kill $APP_PID 2>/dev/null
wait $REC 2>/dev/null
ls "$OUT"
