package ai.storyteller.pdfcraft;

import android.app.NativeActivity;

/**
 * Thin {@link NativeActivity} subclass.
 *
 * <p>NativeActivity loads {@code libpdfcraft.so} (see the {@code android.app.lib_name} meta-data
 * in AndroidManifest.xml) and {@code android-activity}'s glue invokes the Rust {@code android_main}.
 * This subclass exists as the future home for Android-specific integration (intent/file-provider
 * handling, system bar styling, etc.) without touching the Rust side.</p>
 */
public class MainActivity extends NativeActivity {
}
