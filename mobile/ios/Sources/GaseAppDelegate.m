// The iOS side of gase: SDL's app delegate plus getting ROMs into the app.
//
// Everything else -- the window, Metal/OpenGL ES, touches, game
// controllers (MFi, Xbox, PlayStation through Apple's GameController
// framework), sound, rotation, the app lifecycle -- is SDL's, and the Rust
// code (crates/mobile, linked into this executable) runs SDL's main loop.
//
// Where ROMs come from
// --------------------
// An iOS app lives in a sandbox: its own container folder, nothing else. A
// file from elsewhere arrives in one of two ways, and both end up here:
//
//  * the app's "Open ROM..." button: Rust calls gase_ios_pick_rom(), which
//    shows the system's document picker (UIDocumentPickerViewController, the
//    Files app's browser). It hands back a temporary copy of the chosen file;
//  * "Open in gase" / "Share" from another app, or tapping a .md file in
//    Files: Info.plist declares the file types (CFBundleDocumentTypes), and
//    iOS copies the file into Documents/Inbox and calls
//    application:openURL:options:.
//
// Either way the file is moved into Documents/roms/ -- which the Files app
// shows as "On My iPhone > gase > roms" (UIFileSharingEnabled), so ROMs can
// also be copied there directly -- and its path is handed to Rust as an SDL
// drop-file event, which the shell turns into "open this ROM".
//
// Subclassing SDL's app delegate
// ------------------------------
// UIKit creates the app delegate by class name; SDL asks its own class
// +getAppDelegateClassName for that name and documents overriding it in a
// category to substitute a subclass. That is the one "trick" here.

#import <UIKit/UIKit.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>

#include "SDL.h"
#import "SDL_uikitappdelegate.h" // from SDL's src/video/uikit (see project.yml)

@interface GaseAppDelegate : SDLUIKitDelegate <UIDocumentPickerDelegate>
@end

#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wobjc-protocol-method-implementation"
@implementation SDLUIKitDelegate (Gase)
// SDL_UIKitRunApp passes this name to UIApplicationMain.
+ (NSString *)getAppDelegateClassName
{
    return @"GaseAppDelegate";
}
@end
#pragma clang diagnostic pop

// Documents/roms/, created on first use.
static NSURL *RomsDirectory(void)
{
    NSURL *documents = [[NSFileManager defaultManager] URLsForDirectory:NSDocumentDirectory
                                                              inDomains:NSUserDomainMask].firstObject;
    NSURL *roms = [documents URLByAppendingPathComponent:@"roms" isDirectory:YES];
    [[NSFileManager defaultManager] createDirectoryAtURL:roms
                             withIntermediateDirectories:YES
                                              attributes:nil
                                                   error:NULL];
    return roms;
}

// Move or copy a ROM into Documents/roms/; its new path, or nil.
static NSString *ImportRom(NSURL *url)
{
    NSFileManager *files = [NSFileManager defaultManager];
    NSURL *target = [RomsDirectory() URLByAppendingPathComponent:url.lastPathComponent];
    if ([url.URLByStandardizingPath.path isEqualToString:target.URLByStandardizingPath.path]) {
        return target.path; // already there (opened from roms/ in Files)
    }
    // Files from other apps' containers are only readable while "security
    // scoped" access is held.
    BOOL scoped = [url startAccessingSecurityScopedResource];
    NSError *error = nil;
    [files removeItemAtURL:target error:NULL]; // a newer copy replaces an older one
    // Copies made for us (the picker's, Documents/Inbox) are moved, so they
    // do not pile up; anything else is copied.
    NSString *path = url.path;
    BOOL ours = [path containsString:@"/tmp/"] || [path containsString:@"/Documents/Inbox/"];
    BOOL ok = ours ? [files moveItemAtURL:url toURL:target error:&error]
                   : [files copyItemAtURL:url toURL:target error:&error];
    if (scoped) {
        [url stopAccessingSecurityScopedResource];
    }
    if (!ok) {
        SDL_Log("gase: cannot import %s: %s", path.UTF8String,
                error.localizedDescription.UTF8String);
        return nil;
    }
    return target.path;
}

// Hand a ROM to Rust as an SDL_DROPFILE event (SDL frees `file` once the
// Rust side has read it). Until SDL is running -- an "Open in" can wake the
// app before SDL_main has started -- try again a little later.
static void DeliverRom(NSString *path)
{
    if (!SDL_WasInit(SDL_INIT_VIDEO)) {
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.25 * NSEC_PER_SEC)),
                       dispatch_get_main_queue(), ^{
                           DeliverRom(path);
                       });
        return;
    }
    SDL_Event event;
    SDL_zero(event);
    event.type = SDL_DROPFILE;
    event.drop.file = SDL_strdup(path.UTF8String);
    if (SDL_PushEvent(&event) != 1) {
        SDL_free(event.drop.file);
    }
}

@implementation GaseAppDelegate

// "Open in gase", tapping a ROM in Files, a cold start from either.
- (BOOL)application:(UIApplication *)app
            openURL:(NSURL *)url
            options:(NSDictionary<UIApplicationOpenURLOptionsKey, id> *)options
{
    NSString *path = ImportRom(url);
    if (path != nil) {
        DeliverRom(path);
    }
    return path != nil;
}

// SDL reports going to the background as SDL_APP_WILLENTERBACKGROUND and
// the Rust main loop writes the game's save when it next looks at its
// events. iOS may suspend an app soon after this method returns, so ask for
// a moment of background time to let the loop get there.
- (void)applicationDidEnterBackground:(UIApplication *)application
{
    __block UIBackgroundTaskIdentifier task =
        [application beginBackgroundTaskWithName:@"gase-save"
                               expirationHandler:^{
                                   [application endBackgroundTask:task];
                                   task = UIBackgroundTaskInvalid;
                               }];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(2 * NSEC_PER_SEC)),
                   dispatch_get_main_queue(), ^{
                       if (task != UIBackgroundTaskInvalid) {
                           [application endBackgroundTask:task];
                           task = UIBackgroundTaskInvalid;
                       }
                   });
}

// The document picker's answer.
- (void)documentPicker:(UIDocumentPickerViewController *)controller
    didPickDocumentsAtURLs:(NSArray<NSURL *> *)urls
{
    NSURL *url = urls.firstObject;
    NSString *path = url != nil ? ImportRom(url) : nil;
    if (path != nil) {
        DeliverRom(path);
    }
}

- (void)documentPickerWasCancelled:(UIDocumentPickerViewController *)controller
{
    // Nothing to do: the app stays where it was.
}

@end

// Called from Rust (crates/mobile/src/native.rs) on the main thread, where
// SDL_main runs on iOS: show the system's document picker over SDL's view.
void gase_ios_pick_rom(void)
{
    GaseAppDelegate *delegate = (GaseAppDelegate *)[SDLUIKitDelegate sharedAppDelegate];
    UIViewController *top = delegate.window.rootViewController;
    while (top.presentedViewController != nil) {
        top = top.presentedViewController;
    }
    if (top == nil) {
        SDL_Log("gase: no view to show the document picker on");
        return;
    }
    // Any file ("public.data"): iOS knows no type for most ROM extensions,
    // the Rust side checks what it gets. asCopy: we get our own copy, no
    // security-scoped access to keep.
    UIDocumentPickerViewController *picker =
        [[UIDocumentPickerViewController alloc] initForOpeningContentTypes:@[ UTTypeData ]
                                                                    asCopy:YES];
    picker.delegate = delegate;
    picker.allowsMultipleSelection = NO;
    [top presentViewController:picker animated:YES completion:nil];
}
