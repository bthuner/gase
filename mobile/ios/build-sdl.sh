#!/bin/sh
# Build SDL2 as a static library for iOS devices and the simulator, with
# SDL's own Xcode project (the way SDL supports iOS):
#
#   mobile/build/ios/iphoneos/libSDL2.a
#   mobile/build/ios/iphonesimulator/libSDL2.a
#   mobile/build/SDL2 -> the SDL source tree (headers for the app)
#
# Run once before building the app in Xcode (mobile/ios/build.sh does).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
build=${GASE_MOBILE_BUILD:-$here/../build}
sdl=$("$here/../fetch-sdl.sh")
ln -sfn "$sdl" "$build/SDL2"

for platform in ${PLATFORMS:-iphoneos iphonesimulator}; do
    out=$build/ios/$platform
    if [ -f "$out/libSDL2.a" ] && [ "$out/libSDL2.a" -nt "$sdl/.verified" ]; then
        echo "SDL2 for $platform is built" >&2
        continue
    fi
    echo "== SDL2 for $platform" >&2
    xcodebuild -project "$sdl/Xcode/SDL/SDL.xcodeproj" \
        -target "Static Library-iOS" -configuration Release -sdk "$platform" \
        ARCHS=arm64 ONLY_ACTIVE_ARCH=NO IPHONEOS_DEPLOYMENT_TARGET=14.0 \
        SYMROOT="$build/ios/sdl-xcode" OBJROOT="$build/ios/sdl-xcode/obj" \
        build >&2
    mkdir -p "$out"
    cp "$build/ios/sdl-xcode/Release-$platform/libSDL2.a" "$out/libSDL2.a"
done
