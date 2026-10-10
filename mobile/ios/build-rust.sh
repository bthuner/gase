#!/bin/sh
# Build the Rust code (crates/mobile) as a static library for the platform
# Xcode is building for. Xcode runs this before compiling (see project.yml);
# outside Xcode, set PLATFORM_NAME=iphoneos or iphonesimulator.
#
#   mobile/build/ios/<platform>/libgase_mobile.a
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

libs=""
for arch in $archs; do
    case "$platform/$arch" in
        iphoneos/arm64) triple=aarch64-apple-ios ;;
        iphonesimulator/arm64) triple=aarch64-apple-ios-sim ;;
        iphonesimulator/x86_64) triple=x86_64-apple-ios ;;
        *) echo "error: no Rust target for $platform/$arch" >&2; exit 1 ;;
    esac
    cargo rustc --manifest-path "$repo/Cargo.toml" -p gase-mobile \
        --profile "$profile" --target "$triple" --crate-type staticlib
    libs="$libs ${CARGO_TARGET_DIR:-$repo/target}/$triple/$profile/libgase_mobile.a"
done
mkdir -p "$build/ios/$platform"
# One library for all architectures (lipo merges them; with one, it copies).
# shellcheck disable=SC2086
lipo -create $libs -output "$build/ios/$platform/libgase_mobile.a"
