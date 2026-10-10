#!/bin/sh
# Run the windowed frontend for a few seconds with SDL's dummy video and
# audio drivers and fail unless it is still running when time is up.
#
# Unit tests never open a window, so this is what catches crashes in the
# SDL shell itself (texture uploads, event handling, pacing). Use an
# optimised build: memory bugs in unsafe FFI code often only show up once
# the optimiser reuses stack slots.
#
# Usage: scripts/smoke-sdl.sh path/to/gase [rom...]
set -u
bin=${1:?usage: smoke-sdl.sh path/to/gase [rom...]}
shift
config=$(mktemp -d)
status=0
for rom in "" "$@"; do
    # shellcheck disable=SC2086 # an empty $rom means "no ROM": the home screen
    SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy GASE_CONFIG_DIR=$config \
        timeout 5 "$bin" $rom >/dev/null 2>&1
    rc=$?
    if [ "$rc" -eq 124 ]; then
        echo "ok: ${rom:-home screen} ran for 5 s"
    else
        echo "FAILED: ${rom:-home screen} exited with status $rc" >&2
        status=1
    fi
done
rm -rf "$config"
exit $status
