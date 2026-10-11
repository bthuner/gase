#!/bin/sh
# Build the Rust code (crates/mobile) as a static library for the platform
# Xcode is building for. Xcode runs this before compiling (see project.yml);
# outside Xcode, set PLATFORM_NAME=iphoneos or iphonesimulator.
#
#   mobile/build/ios/<platform>/libgase_mobile.a
#
# GASE_MOBILE_MANIFEST and GASE_MOBILE_PACKAGE choose another crate than
# this repository's gase-mobile: a game that embeds gase (a statically
# recompiled one) and exports its own SDL_main. The library keeps the name
# the Xcode project links (libgase_mobile.a).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
build=${GASE_MOBILE_BUILD:-$repo/mobile/build}
platform=${PLATFORM_NAME:-iphonesimulator}
archs=${ARCHS:-arm64}
# Xcode's environment lacks the user's PATH.
PATH="$HOME/.cargo/bin:$PATH"
# Xcode points SDKROOT at the iOS SDK; Cargo also builds programs for the
# Mac itself (build scripts), which must not see it. rustc finds the iOS
# SDK on its own (xcrun).
unset SDKROOT
# Debug app builds still get optimised Rust: an emulator in a debug build
# is far too slow to play.
profile=release
manifest=${GASE_MOBILE_MANIFEST:-$repo/Cargo.toml}
package=${GASE_MOBILE_PACKAGE:-gase-mobile}
target_dir=${CARGO_TARGET_DIR:-$(cargo metadata --format-version 1 --no-deps \
    --manifest-path "$manifest" | sed 's/.*"target_directory":"\([^"]*\)".*/\1/')}
lib=lib$(echo "$package" | tr - _).a

libs=""
for arch in $archs; do
    case "$platform/$arch" in
        iphoneos/arm64) triple=aarch64-apple-ios ;;
        iphonesimulator/arm64) triple=aarch64-apple-ios-sim ;;
        iphonesimulator/x86_64) triple=x86_64-apple-ios ;;
        *) echo "error: no Rust target for $platform/$arch" >&2; exit 1 ;;
    esac
    cargo rustc --manifest-path "$manifest" -p "$package" \
        --profile "$profile" --target "$triple" --lib --crate-type staticlib
    libs="$libs $target_dir/$triple/$profile/$lib"
done
mkdir -p "$build/ios/$platform"
# One library for all architectures (lipo merges them; with one, it copies).
# shellcheck disable=SC2086
lipo -create $libs -output "$build/ios/$platform/libgase_mobile.a"
