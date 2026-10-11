package io.github.bthuner.gase;

import android.app.PendingIntent;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.content.pm.PackageInstaller;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.util.Log;
import android.widget.Toast;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

import org.libsdl.app.SDLActivity;

/**
 * The gase Android app: SDL's activity plus the one thing SDL cannot do,
 * getting ROMs from the rest of the phone.
 *
 * <p>Everything else is SDL's {@link SDLActivity} (from the pinned SDL
 * release, see mobile/fetch-sdl.sh): it loads libSDL2.so and libmain.so,
 * starts a thread that calls the Rust {@code SDL_main}, and forwards the
 * surface, touches, keys, gamepads, sound and the activity lifecycle
 * (onPause/onResume become SDL_APP_WILLENTERBACKGROUND / ..FOREGROUND) to
 * native code through JNI.
 *
 * <h2>Where ROMs come from</h2>
 *
 * An Android app may not read the user's files directly. It asks the
 * system: the Storage Access Framework's document picker
 * ({@code ACTION_OPEN_DOCUMENT}) lets the user choose one file and grants
 * this app access to that file only, as a {@code content://} URI, which is
 * not a path: it can only be read through a {@code ContentResolver}. Other
 * apps hand over files the same way ("Open with", "Share").
 *
 * <p>So every ROM is <b>copied</b> into the app's private folder,
 * {@code files/roms/} (the same folder SDL_GetPrefPath gives the Rust
 * side), and the copy's path goes to Rust as an SDL "drop file" event,
 * which the shell already turns into "open this ROM". The copy can be
 * opened again from the recent list without asking the user, and it is
 * read with plain file I/O. Copying happens on a background thread: the UI
 * thread must never wait for storage.
 *
 * <p>The three ways in:
 * <ul>
 *   <li>the app's "Open ROM…" button: Rust calls
 *       {@code SDL_AndroidSendMessage(COMMAND_PICK_ROM)}, which arrives in
 *       {@link #onUnhandledMessage} on the UI thread, which opens the
 *       picker; the answer comes in {@link #onActivityResult};</li>
 *   <li>"Open with gase" while gase runs: {@link #onNewIntent};</li>
 *   <li>"Open with gase" starting gase: native code is not running yet, so
 *       an event would be lost; {@link #getArguments} imports the ROM on
 *       SDL's thread and passes its path as {@code argv[1]} instead.</li>
 * </ul>
 *
 * <h2>Installing an app (phone builds)</h2>
 *
 * gase-recomp's phone app can have a computer on the network turn a game
 * into an app of its own, and receives the APK. Rust writes it to
 * {@code files/builds/app.apk} and sends {@code COMMAND_INSTALL_APK};
 * {@link #installApk} hands it to the system's {@link PackageInstaller},
 * which asks the user to confirm (and, the first time, to allow this app
 * to install apps: the {@code REQUEST_INSTALL_PACKAGES} permission, which
 * only that app's manifest asks for, see build.gradle's
 * {@code gasePhoneBuilds}). The installer reports back with an intent to
 * this activity, {@link #onNewIntent}.
 */
public class GaseActivity extends SDLActivity {
    private static final String TAG = "gase";

    /** Must match COMMAND_PICK_ROM in crates/mobile/src/native.rs. */
    static final int COMMAND_PICK_ROM = COMMAND_USER + 1;
    /** Must match COMMAND_INSTALL_APK in crates/mobile/src/native.rs. */
    static final int COMMAND_INSTALL_APK = COMMAND_USER + 2;

    /** How the package installer reports back to this activity. */
    private static final String ACTION_INSTALL_STATUS =
            "io.github.bthuner.gase.INSTALL_STATUS";

    private static final int REQUEST_PICK_ROM = 1;

    /** Larger files are not Mega Drive ROMs (or ZIP archives of one). */
    private static final long MAX_ROM_BYTES = 64L * 1024 * 1024;

    /** A ROM this activity was started with, imported by getArguments(). */
    private Uri launchRom;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        launchRom = romUri(getIntent());
        if (launchRom != null) {
            // SDLActivity.onCreate would pass getIntent().getData().getPath()
            // to native code as a dropped file. For a content:// URI that is
            // not a file path, and SDL_main has not started, so the event
            // would be lost anyway. We import the ROM ourselves.
            setIntent(new Intent(Intent.ACTION_MAIN));
        }
        super.onCreate(savedInstanceState);
    }

    /**
     * SDL asks for SDL_main's arguments on its own thread, just before
     * calling it: a good moment to copy a ROM (slow storage must not block
     * the UI thread).
     */
    @Override
    protected String[] getArguments() {
        Uri uri = launchRom;
        launchRom = null;
        String path = uri != null ? importRom(uri) : null;
        return path != null ? new String[] {path} : new String[0];
    }

    /** "Open with gase" while gase is already running (launchMode singleTask). */
    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        if (ACTION_INSTALL_STATUS.equals(intent.getAction())) {
            installStatus(intent);
            return;
        }
        Uri uri = romUri(intent);
        if (uri != null) {
            importInBackground(uri);
        }
    }

    /** Messages from native code that SDL does not know: ours. */
    @Override
    protected boolean onUnhandledMessage(int command, Object param) {
        if (command == COMMAND_PICK_ROM) {
            pickRom();
            return true;
        }
        if (command == COMMAND_INSTALL_APK) {
            installApk();
            return true;
        }
        return super.onUnhandledMessage(command, param);
    }

    /** Show the system's document picker. */
    private void pickRom() {
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        // Android knows no MIME type for ROMs: offer every file, the Rust
        // side checks what it gets.
        intent.setType("*/*");
        try {
            startActivityForResult(intent, REQUEST_PICK_ROM);
        } catch (ActivityNotFoundException e) {
            // Some Android TV boxes have no document picker; try the older,
            // simpler "get content".
            Intent fallback = new Intent(Intent.ACTION_GET_CONTENT);
            fallback.addCategory(Intent.CATEGORY_OPENABLE);
            fallback.setType("*/*");
            try {
                startActivityForResult(fallback, REQUEST_PICK_ROM);
            } catch (ActivityNotFoundException e2) {
                showError("This device has no file picker");
            }
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == REQUEST_PICK_ROM && resultCode == RESULT_OK
                && data != null && data.getData() != null) {
            importInBackground(data.getData());
        }
        // Cancelled: nothing to do, the app stays where it was.
    }

    /** The file an intent carries: VIEW has it as data, SEND as an extra. */
    @SuppressWarnings("deprecation") // getParcelableExtra(String), needed before Android 13
    private static Uri romUri(Intent intent) {
        if (intent == null) {
            return null;
        }
        if (Intent.ACTION_VIEW.equals(intent.getAction())) {
            return intent.getData();
        }
        if (Intent.ACTION_SEND.equals(intent.getAction())) {
            if (Build.VERSION.SDK_INT >= 33) {
                return intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri.class);
            }
            Object stream = intent.getParcelableExtra(Intent.EXTRA_STREAM);
            return stream instanceof Uri ? (Uri) stream : null;
        }
        return null;
    }

    /** Copy on a background thread, then tell native code. */
    private void importInBackground(final Uri uri) {
        new Thread(new Runnable() {
            @Override
            public void run() {
                String path = importRom(uri);
                if (path != null) {
                    // Pushes an SDL_DROPFILE event (thread-safe); the Rust
                    // shell opens the ROM on its next frame.
                    SDLActivity.onNativeDropFile(path);
                }
            }
        }, "gase-import").start();
    }

    /**
     * Copy a ROM into files/roms/ and return the copy's path, or null (after
     * telling the user) if it cannot be read.
     */
    private String importRom(Uri uri) {
        String name = safeName(displayName(uri));
        File dir = new File(getFilesDir(), "roms");
        if (!dir.isDirectory() && !dir.mkdirs()) {
            showError("Cannot create " + dir);
            return null;
        }
        File target = new File(dir, name);
        // Write next to it and rename: never a half-copied ROM.
        File partial = new File(dir, name + ".partial");
        try (InputStream in = getContentResolver().openInputStream(uri);
             OutputStream out = new FileOutputStream(partial)) {
            if (in == null) {
                throw new IOException("no data");
            }
            byte[] buffer = new byte[64 * 1024];
            long total = 0;
            int n;
            while ((n = in.read(buffer)) > 0) {
                total += n;
                if (total > MAX_ROM_BYTES) {
                    throw new IOException("too large for a Mega Drive ROM");
                }
                out.write(buffer, 0, n);
            }
        } catch (IOException | SecurityException | IllegalArgumentException e) {
            Log.e(TAG, "Cannot import " + uri, e);
            //noinspection ResultOfMethodCallIgnored
            partial.delete();
            showError("Cannot open " + name + ": " + e.getMessage());
            return null;
        }
        if (!partial.renameTo(target)) {
            //noinspection ResultOfMethodCallIgnored
            partial.delete();
            showError("Cannot store " + name);
            return null;
        }
        Log.i(TAG, "Imported " + uri + " as " + target);
        return target.getPath();
    }

    /** The file's name as the user saw it in the picker. */
    private String displayName(Uri uri) {
        if ("content".equals(uri.getScheme())) {
            try (Cursor cursor = getContentResolver().query(
                    uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
                if (cursor != null && cursor.moveToFirst() && !cursor.isNull(0)) {
                    return cursor.getString(0);
                }
            } catch (RuntimeException e) {
                Log.w(TAG, "No name for " + uri, e);
            }
        }
        String last = uri.getLastPathSegment();
        return last != null ? last : "game.bin";
    }

    /** A name that is a plain file name inside roms/, whatever we were given. */
    static String safeName(String name) {
        String clean = name.replace('/', '_').replace('\\', '_').replace('\0', '_').trim();
        while (clean.startsWith(".")) {
            clean = clean.substring(1);
        }
        if (clean.length() > 120) {
            clean = clean.substring(clean.length() - 120);
        }
        return clean.isEmpty() ? "game.bin" : clean;
    }

    /**
     * Hand files/builds/app.apk to the package installer: open a session,
     * copy the APK into it, and commit it. Copying is on a background
     * thread; committing only asks, the installer then shows its window.
     */
    private void installApk() {
        final File apk = new File(new File(getFilesDir(), "builds"), "app.apk");
        new Thread(new Runnable() {
            @Override
            public void run() {
                PackageInstaller installer = getPackageManager().getPackageInstaller();
                PackageInstaller.SessionParams params = new PackageInstaller.SessionParams(
                        PackageInstaller.SessionParams.MODE_FULL_INSTALL);
                try {
                    int id = installer.createSession(params);
                    try (PackageInstaller.Session session = installer.openSession(id)) {
                        try (InputStream in = new FileInputStream(apk);
                             OutputStream out = session.openWrite("app.apk", 0, apk.length())) {
                            byte[] buffer = new byte[64 * 1024];
                            int n;
                            while ((n = in.read(buffer)) > 0) {
                                out.write(buffer, 0, n);
                            }
                            session.fsync(out);
                        }
                        Intent status = new Intent(ACTION_INSTALL_STATUS)
                                .setClass(GaseActivity.this, GaseActivity.class);
                        // The installer adds its answer to the intent: from
                        // Android 12 on, that needs a mutable PendingIntent.
                        int flags = PendingIntent.FLAG_UPDATE_CURRENT;
                        if (Build.VERSION.SDK_INT >= 31) {
                            flags |= PendingIntent.FLAG_MUTABLE;
                        }
                        PendingIntent pending = PendingIntent.getActivity(
                                GaseActivity.this, id, status, flags);
                        session.commit(pending.getIntentSender());
                    }
                } catch (IOException | RuntimeException e) {
                    Log.e(TAG, "Cannot install " + apk, e);
                    showError("Cannot install the app: " + e.getMessage());
                }
            }
        }, "gase-install").start();
    }

    /** The package installer's answer to {@link #installApk}. */
    @SuppressWarnings("deprecation") // getParcelableExtra(String), needed before Android 13
    private void installStatus(Intent intent) {
        int status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS,
                PackageInstaller.STATUS_FAILURE);
        if (status == PackageInstaller.STATUS_PENDING_USER_ACTION) {
            // The user must confirm: show the installer's own window.
            Object confirm = intent.getParcelableExtra(Intent.EXTRA_INTENT);
            if (confirm instanceof Intent) {
                try {
                    startActivity((Intent) confirm);
                } catch (ActivityNotFoundException e) {
                    showError("This device cannot install apps");
                }
            }
        } else if (status == PackageInstaller.STATUS_SUCCESS) {
            showError("App installed: find it with your other apps");
        } else {
            String message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE);
            if (status == PackageInstaller.STATUS_FAILURE_CONFLICT) {
                // Same app id, other signing key: built on another computer.
                message = "an app with this name, built elsewhere, is installed: "
                        + "uninstall it first";
            }
            showError("The app was not installed: " + message);
        }
    }

    private void showError(final String message) {
        Log.w(TAG, message);
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                Toast.makeText(GaseActivity.this, message, Toast.LENGTH_LONG).show();
            }
        });
    }
}
