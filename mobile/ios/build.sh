#!/bin/sh
# Build the iOS app without signing, for the simulator and for devices:
#
#   mobile/build/ios/app/Release-iphonesimulator/gase.app
#   mobile/build/ios/app/Release-iphoneos/gase.app   (unsigned: sign it to install)
#
# Needs a Mac with Xcode, rustup targets aarch64-apple-ios and
# aarch64-apple-ios-sim, and XcodeGen. This is what CI runs
# (.github/workflows/mobile.yml); for day-to-day work run build-sdl.sh once,
# `xcodegen generate`, and open Gase.xcodeproj in Xcode.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
build=${GASE_MOBILE_BUILD:-$here/../build}

"$here/build-sdl.sh"
cd "$here"
xcodegen generate
for platform in ${PLATFORMS:-iphonesimulator iphoneos}; do
    echo "== gase.app for $platform" >&2
    xcodebuild -project Gase.xcodeproj -target Gase -configuration Release \
        -sdk "$platform" ARCHS=arm64 \
        SYMROOT="$build/ios/app" OBJROOT="$build/ios/app/obj" \
        CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY="" \
        build
done
ls -d "$build"/ios/app/Release-*/gase.app
