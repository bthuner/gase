#!/bin/sh
# Build the two native libraries of the Android app, for each ABI:
#
#   <out>/<abi>/libSDL2.so   SDL2 2.32 from source, with the NDK's CMake toolchain
#   <out>/<abi>/libmain.so   the Rust code (crates/mobile), linked against it
#
# Gradle runs this before packaging (task buildNative in app/build.gradle)
# and packs <out> into the APK as its jniLibs. It can also run on its own:
#
#   ANDROID_NDK_HOME=~/Android/Sdk/ndk/27.2.12479018 mobile/android/build-native.sh /tmp/jni
#
# Environment:
#   ANDROID_NDK_HOME  the NDK (required; Gradle passes the one it uses)
#   ABIS              which ABIs (default: "arm64-v8a armeabi-v7a x86_64")
#   CARGO_PROFILE     release (default) or dev; release is what you want on
#                     a phone: emulation in a debug build is far too slow
#
# Why no cargo-ndk? It only sets the variables below; spelling them out
# shows what cross-compiling Rust for Android takes: a target triple
# (rustup target add …), the NDK's clang as the linker, and where the
# libraries to link against are.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
out=${1:-$here/app/build/gase-jni}
ndk=${ANDROID_NDK_HOME:?set ANDROID_NDK_HOME to the Android NDK}
abis=${ABIS:-arm64-v8a armeabi-v7a x86_64}
profile=${CARGO_PROFILE:-release}
# The oldest Android the app runs on (Android 5.0); keep equal to minSdk in
# app/build.gradle.
api=21
build=${GASE_MOBILE_BUILD:-$repo/mobile/build}

case "$(uname -s)" in
    Linux) host=linux-x86_64 ;;
    Darwin) host=darwin-x86_64 ;; # also on Apple silicon
    *) echo "error: build on Linux or macOS (on Windows: WSL)" >&2; exit 1 ;;
esac
toolchain=$ndk/toolchains/llvm/prebuilt/$host/bin
[ -d "$toolchain" ] || { echo "error: no NDK toolchain at $toolchain" >&2; exit 1; }

sdl=$("$repo/mobile/fetch-sdl.sh")

for abi in $abis; do
    # Android's ABI names, Rust's target triples and the NDK's clang
    # wrappers (which pick the Android version by name) all differ.
    case $abi in
        arm64-v8a) triple=aarch64-linux-android clang=aarch64-linux-android ;;
        armeabi-v7a) triple=armv7-linux-androideabi clang=armv7a-linux-androideabi ;;
        x86_64) triple=x86_64-linux-android clang=x86_64-linux-android ;;
        x86) triple=i686-linux-android clang=i686-linux-android ;;
        *) echo "error: unknown ABI $abi" >&2; exit 1 ;;
    esac
    echo "== $abi ($triple)" >&2
    mkdir -p "$out/$abi"

    # 1. SDL2, as a shared library. ANDROID_STL=c++_static puts the bit of
    #    C++ SDL uses (its HID driver) inside libSDL2.so, so there is no
    #    libc++_shared.so to ship. Flexible page sizes: 16 KB-page devices
    #    (Android 15+) refuse libraries aligned for 4 KB pages.
    sdl_build=$build/sdl-android-$abi
    cmake -S "$sdl" -B "$sdl_build" \
        -DCMAKE_TOOLCHAIN_FILE="$ndk/build/cmake/android.toolchain.cmake" \
        -DANDROID_ABI="$abi" -DANDROID_PLATFORM="android-$api" \
        -DANDROID_STL=c++_static -DANDROID_SUPPORT_FLEXIBLE_PAGE_SIZES=ON \
        -DCMAKE_BUILD_TYPE=Release \
        -DSDL_SHARED=ON -DSDL_STATIC=OFF -DSDL_TEST=OFF >&2
    cmake --build "$sdl_build" --parallel >&2
    cp "$sdl_build/libSDL2.so" "$out/$abi/libSDL2.so"

    # 2. The Rust library, as a C dynamic library named the way SDL's Java
    #    side looks for it (SDLActivity.getLibraries(): "SDL2", "main").
    #    Cargo reads CARGO_TARGET_<TRIPLE>_LINKER and _RUSTFLAGS for the
    #    target only, so the host build scripts are unaffected.
    var=$(echo "$triple" | tr 'a-z-' 'A-Z_')
    env "CARGO_TARGET_${var}_LINKER=$toolchain/$clang$api-clang" \
        "CARGO_TARGET_${var}_RUSTFLAGS=-L native=$sdl_build -C link-arg=-Wl,-z,max-page-size=16384" \
        cargo rustc --manifest-path "$repo/Cargo.toml" -p gase-mobile \
        --profile "$profile" --target "$triple" --crate-type cdylib >&2
    target_dir=${CARGO_TARGET_DIR:-$repo/target}
    dir=$profile
    [ "$profile" = dev ] && dir=debug
    cp "$target_dir/$triple/$dir/libgase_mobile.so" "$out/$abi/libmain.so"
done
echo "Native libraries in $out" >&2
