#!/usr/bin/env bash
set -euo pipefail

mode="${1:?Build step required}"
mod_id="${2:?Mod ID required}"
[[ "$mod_id" =~ ^[a-z][a-z0-9_]{1,63}$ ]]

mkdir -p "$HOME" /work/project
cp -R /opt/bezalel/cache/gradle "$GRADLE_USER_HOME"
cp -R /opt/bezalel/cache/project /work/project/.gradle
cp -R /opt/bezalel/template/. /work/project/
cd /work/project
if [[ -f /opt/bezalel/cache/mc-version.json ]]; then
    mkdir -p build/downloadMCMeta
    cp /opt/bezalel/cache/mc-version.json build/downloadMCMeta/version.json
fi
tar --extract --file=- --no-same-owner --no-same-permissions

case "$mode" in
    compile)
        if ! gradle --offline --no-daemon --console=plain --max-workers=2 \
            "-PbezalelModId=$mod_id" build >/work/compile.log 2>&1; then
            tail -n 75 /work/compile.log >&2
            exit 1
        fi
        cat build/libs/mod.jar
        ;;
    client | server)
        bezalel-smoke "$mode" "$mod_id" offline
        printf 'passed\n'
        ;;
    *)
        printf 'Unknown build step.\n' >&2
        exit 2
        ;;
esac
