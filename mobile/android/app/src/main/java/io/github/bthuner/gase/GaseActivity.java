package io.github.bthuner.gase;

import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.util.Log;
import android.widget.Toast;

import java.io.File;
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
 */
public class GaseActivity extends SDLActivity {
    private static final String TAG = "gase";

    /** Must match COMMAND_PICK_ROM in crates/mobile/src/native.rs. */
    static final int COMMAND_PICK_ROM = COMMAND_USER + 1;

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
