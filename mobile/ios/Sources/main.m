// The executable's entry point on iOS.
//
// An iOS app is one executable that hands control to UIKit at once:
// UIApplicationMain creates the app delegate, runs the launch, and then the
// main run loop, forever. SDL_UIKitRunApp does exactly that with SDL's app
// delegate (GaseAppDelegate.m substitutes ours), and calls SDL_main -- the
// Rust function exported by crates/mobile -- once the app has launched.
//
// SDL2 ships the same three lines as libSDL2main; writing them here keeps
// the whole path from `main` to Rust visible, and SDL_MAIN_HANDLED tells
// SDL_main.h not to rename this `main`.
#define SDL_MAIN_HANDLED
#include "SDL_main.h"

int main(int argc, char *argv[])
{
    return SDL_UIKitRunApp(argc, argv, SDL_main);
}
