#!/usr/bin/env bash
set -euo pipefail

mode="${1:?Smoke target required}"
mod_id="${2:?Mod ID required}"
network="${3:?Gradle cache mode required}"
log="/work/$mode.log"
gradle_flags=(--no-daemon --console=plain --max-workers=2 "-PbezalelModId=$mod_id")
if [[ "$network" == offline ]]; then
    gradle_flags+=(--offline)
fi

game_pid=
xvfb_pid=
cleanup() {
    if [[ -n "$game_pid" ]]; then
        if kill -- "-$game_pid" 2>/dev/null; then
            sleep 1
            kill -KILL -- "-$game_pid" 2>/dev/null || true
        fi
        wait "$game_pid" 2>/dev/null || true
    fi
    if [[ -n "$xvfb_pid" ]]; then
        kill "$xvfb_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT

case "$mode" in
    client)
        export DISPLAY=:91
        Xvfb "$DISPLAY" -screen 0 1280x720x24 -nolisten tcp -ac >/work/xvfb.log 2>&1 &
        xvfb_pid=$!
        # The X server is ready when a display query succeeds, not when it starts.
        for _ in {1..20}; do
            if xdotool getdisplaygeometry >/dev/null 2>&1; then
                break
            fi
            sleep 0.25
        done
        xdotool getdisplaygeometry >/dev/null
        task=runClient
        ;;
    server)
        mkdir -p run-server
        printf 'eula=true\n' >run-server/eula.txt
        printf 'online-mode=false\nserver-ip=127.0.0.1\nserver-port=25565\nlevel-type=minecraft:flat\nview-distance=3\nsimulation-distance=3\n' >run-server/server.properties
        task=runServer
        ;;
    *) exit 2 ;;
esac

setsid gradle "${gradle_flags[@]}" "$task" </dev/null >"$log" 2>&1 &
game_pid=$!
ready=0
for ((elapsed=0; elapsed<240; elapsed++)); do
    if ! kill -0 "$game_pid" 2>/dev/null || grep -Eiq \
        'crash report has been saved|Exception in thread "main"|Failed to load mods|Failed to start the minecraft server|LoadingFailedException|Failed to initialize mod containers|Duplicate mods found|has mods that were not found' "$log"; then
        break
    fi
    if [[ "$mode" == server ]]; then
        if grep -Eq 'Done \([0-9.]+s\)!' "$log" && ( : </dev/tcp/127.0.0.1/25565 ) 2>/dev/null; then
            ((ready+=1))
        else
            ready=0
        fi
    elif grep -Eq 'textures/atlas/gui[.]png' "$log" \
        && xdotool search --onlyvisible --name 'Minecraft.*1[.]21[.]1' >/dev/null 2>&1; then
        ((ready+=1))
    else
        ready=0
    fi
    if [[ "$ready" -ge 8 ]]; then
        exit 0
    fi
    sleep 1
done

printf '%s smoke test did not reach a stable startup.\n' "$mode" >&2
tail -n 75 "$log" >&2
exit 1
