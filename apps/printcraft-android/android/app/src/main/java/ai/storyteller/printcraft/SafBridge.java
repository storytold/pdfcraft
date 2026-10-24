package ai.storyteller.printcraft;

import android.app.Activity;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.provider.OpenableColumns;
import android.util.Log;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.io.OutputStreamWriter;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * Storage Access Framework bridge for PrintCraft. The engine reads and writes real file paths
 * (a {@code content://} URI is useless to it), so
 *
 * <ul>
 *   <li><b>open</b>: a picked document is copied into the app's own files directory before the
 *       engine opens it (File ▸ Open, Combine files, Insert pages, Scan &amp; OCR, Attach);</li>
 *   <li><b>save</b>: the engine writes to a scratch path in the app's export directory and
 *       {@link #publish} copies the finished file to the location the user chose with
 *       {@code ACTION_CREATE_DOCUMENT}.</li>
 * </ul>
 *
 * <p>Called from Rust ({@code apps/printcraft-android/src/saf.rs}), from a background thread or
 * the UI thread. The pickers are posted to the UI thread here, and the Rust side polls the
 * {@code is*} methods; bulk data travels through the filesystem (a manifest file) rather than JNI
 * marshalling, so the JNI surface stays at a handful of calls.
 */
public final class SafBridge {
    public static final int REQUEST_CODE = 0x5AF;
    public static final int REQUEST_SAVE = 0x5B0;
    private static final String TAG = "SafBridge";
    private static final String SUBDIR = "import";
    private static final String MANIFEST = "import-manifest.txt";

    private static volatile String destDir;
    private static volatile String manifestPath;
    private static volatile boolean done = true;
    private static final List<String> picked = Collections.synchronizedList(new ArrayList<String>());

    private static volatile boolean saveDone = true;
    private static volatile String saveUri;

    private SafBridge() {}

    // ---------------------------------------------------------------- open

    /**
     * Open the document picker. Returns immediately; poll {@link #isDone}.
     *
     * @param mimes    the types the caller accepts, {@code |}-separated ({@code application/pdf}).
     *                 Without an explicit list some pickers (vivo's, for one) hide everything but
     *                 "documents", so the caller always names what it wants.
     * @param multiple allow selecting more than one document.
     */
    public static void pickDocuments(Activity activity, String mimes, boolean multiple) {
        File dir = new File(activity.getFilesDir(), SUBDIR);
        if (!dir.exists() && !dir.mkdirs()) {
            Log.w(TAG, "cannot create " + dir);
        }
        destDir = dir.getAbsolutePath();
        manifestPath = new File(activity.getFilesDir(), MANIFEST).getAbsolutePath();
        picked.clear();
        done = false;
        final String[] types = mimes == null || mimes.isEmpty() ? new String[] {"*/*"} : mimes.split("\\|");
        activity.runOnUiThread(() -> {
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            intent.setType(types.length == 1 ? types[0] : "*/*");
            intent.putExtra(Intent.EXTRA_MIME_TYPES, types);
            intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, multiple);
            try {
                activity.startActivityForResult(intent, REQUEST_CODE);
            } catch (Exception e) {
                Log.w(TAG, "picker failed: " + e);
                writeManifest();
                done = true;
            }
        });
    }

    /** Whether the picker and the copy into the app directory are finished. */
    public static boolean isDone() {
        return done;
    }

    /** The files copied in so far; clears the list. */
    public static String[] takePickedPaths() {
        synchronized (picked) {
            String[] out = picked.toArray(new String[0]);
            picked.clear();
            return out;
        }
    }

    // ---------------------------------------------------------------- save

    /**
     * Ask where a file should be saved ({@code ACTION_CREATE_DOCUMENT}). Returns immediately; poll
     * {@link #isSaveDone}, then {@link #hasSaveUri}, then {@link #publish} once the file exists.
     *
     * @param title    the dialog's title.
     * @param fileName the suggested name ({@code report.pdf}); the picker pre-fills it.
     * @param mime     the type of the file being written ({@code application/pdf}).
     */
    public static void pickSave(Activity activity, String title, String fileName, String mime) {
        saveUri = null;
        saveDone = false;
        final String type = mime == null || mime.isEmpty() ? "*/*" : mime;
        activity.runOnUiThread(() -> {
            Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            intent.setType(type);
            if (fileName != null && !fileName.isEmpty()) {
                intent.putExtra(Intent.EXTRA_TITLE, fileName);
            }
            try {
                activity.startActivityForResult(intent, REQUEST_SAVE);
            } catch (Exception e) {
                Log.w(TAG, "save picker failed: " + e);
                saveDone = true;
            }
        });
    }

    public static boolean isSaveDone() {
        return saveDone;
    }

    public static boolean hasSaveUri() {
        return saveUri != null;
    }

    /**
     * Copy the finished file at {@code srcPath} into the location the user chose. Returns true when
     * the bytes are in place (the URI is spent either way).
     */
    public static boolean publish(Activity activity, String srcPath) {
        String uri = saveUri;
        saveUri = null;
        if (uri == null) {
            return false;
        }
        try (InputStream in = new java.io.FileInputStream(srcPath);
             OutputStream out = activity.getContentResolver().openOutputStream(Uri.parse(uri))) {
            if (out == null) {
                Log.w(TAG, "no output stream for " + uri);
                return false;
            }
            byte[] buffer = new byte[1 << 16];
            int read;
            while ((read = in.read(buffer)) > 0) {
                out.write(buffer, 0, read);
            }
            Log.i(TAG, "published " + srcPath + " -> " + uri);
            return true;
        } catch (Exception e) {
            Log.w(TAG, "publish failed: " + e);
            return false;
        }
    }

    // ---------------------------------------------------------------- results

    /** Called from {@code MainActivity.onActivityResult} for both request codes. */
    public static void onResult(Activity activity, int requestCode, Intent data) {
        if (requestCode == REQUEST_SAVE) {
            saveUri = data == null || data.getData() == null ? null : data.getData().toString();
            saveDone = true;
            Log.i(TAG, "save destination: " + saveUri);
            return;
        }
        try {
            if (data == null || destDir == null) {
                return;
            }
            List<Uri> uris = new ArrayList<>();
            if (data.getClipData() != null) {
                for (int i = 0; i < data.getClipData().getItemCount(); i++) {
                    uris.add(data.getClipData().getItemAt(i).getUri());
                }
            } else if (data.getData() != null) {
                uris.add(data.getData());
            }
            for (Uri uri : uris) {
                String path = copyTo(activity, uri, destDir);
                if (path != null) {
                    picked.add(path);
                }
            }
        } catch (Exception e) {
            Log.w(TAG, "open failed: " + e);
        } finally {
            writeManifest();
            done = true;
        }
    }

    /** The Rust side reads this file; one picked path per line. */
    private static void writeManifest() {
        String path = manifestPath;
        if (path == null) {
            return;
        }
        try (Writer out = new OutputStreamWriter(new FileOutputStream(path), StandardCharsets.UTF_8)) {
            synchronized (picked) {
                for (String entry : picked) {
                    out.write(entry);
                    out.write('\n');
                }
            }
        } catch (Exception e) {
            Log.w(TAG, "manifest write failed: " + e);
        }
    }

    private static String copyTo(Activity activity, Uri uri, String dir) {
        try {
            String name = displayName(activity, uri);
            File out = new File(dir, name);
            for (int n = 1; out.exists(); n++) {
                out = new File(dir, n + "_" + name);
            }
            try (InputStream in = activity.getContentResolver().openInputStream(uri);
                 FileOutputStream sink = new FileOutputStream(out)) {
                if (in == null) {
                    return null;
                }
                byte[] buffer = new byte[1 << 16];
                int read;
                while ((read = in.read(buffer)) > 0) {
                    sink.write(buffer, 0, read);
                }
            }
            Log.i(TAG, "copied " + uri + " -> " + out.getAbsolutePath());
            return out.getAbsolutePath();
        } catch (Exception e) {
            Log.w(TAG, "copy failed for " + uri + ": " + e);
            return null;
        }
    }

    private static String displayName(Activity activity, Uri uri) {
        try (Cursor cursor = activity.getContentResolver().query(uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                String name = cursor.getString(0);
                if (name != null && !name.isEmpty()) {
                    return name;
                }
            }
        } catch (Exception e) {
            Log.w(TAG, "display name lookup failed: " + e);
        }
        String fallback = uri.getLastPathSegment();
        return fallback == null ? "imported.pdf" : fallback.replace('/', '_');
    }
}
