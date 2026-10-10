#!/bin/sh
# Download the SDL2 source release both phone builds compile, check it
# against a pinned SHA-256, unpack it, and print where it is.
#
#   SDL=$(mobile/fetch-sdl.sh)
#
# Nothing of SDL is kept in the repository: the version and checksum below
# are the whole dependency. To update SDL, change both (the checksum of the
# new tarball from https://github.com/libsdl-org/SDL/releases) and check
# that mobile/android/app/src/main/java/.../GaseActivity.java still fits
# SDL's android-project (its SDLActivity is copied from the same release).
#
# The download lands in $GASE_MOBILE_BUILD (default: mobile/build/, which
# git ignores); a verified copy is reused.
set -eu

SDL_VERSION=2.32.10
SDL_SHA256=5f5993c530f084535c65a6879e9b26ad441169b3e25d789d83287040a9ca5165

here=$(cd "$(dirname "$0")" && pwd)
build=${GASE_MOBILE_BUILD:-$here/build}
dir=$build/SDL2-$SDL_VERSION

if [ ! -f "$dir/.verified" ]; then
    mkdir -p "$build"
    tarball=$build/SDL2-$SDL_VERSION.tar.gz
    url=https://github.com/libsdl-org/SDL/releases/download/release-$SDL_VERSION/SDL2-$SDL_VERSION.tar.gz
    echo "Downloading $url" >&2
    curl -fsSL --retry 3 -o "$tarball" "$url"
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$tarball" | cut -d' ' -f1)
    else
        actual=$(shasum -a 256 "$tarball" | cut -d' ' -f1) # macOS
    fi
    if [ "$actual" != "$SDL_SHA256" ]; then
        echo "error: $tarball has SHA-256 $actual, expected $SDL_SHA256" >&2
        rm -f "$tarball"
        exit 1
    fi
    rm -rf "$dir"
    tar -xzf "$tarball" -C "$build"
    touch "$dir/.verified"
fi
echo "$dir"
