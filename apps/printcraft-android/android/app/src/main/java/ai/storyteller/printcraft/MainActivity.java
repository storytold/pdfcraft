package ai.storyteller.printcraft;

import com.google.androidgamesdk.GameActivity;

import android.content.Intent;
import android.os.Bundle;

public class MainActivity extends GameActivity {

    static {
        // Loads the Rust cdylib (android.app.lib_name=main in the manifest).
        System.loadLibrary("main");
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == SafBridge.REQUEST_CODE || requestCode == SafBridge.REQUEST_SAVE) {
            SafBridge.onResult(this, requestCode, resultCode == RESULT_OK ? data : null);
        }
    }

    /**
     * Rust calls these instance methods (calling a method on an object we already hold needs no
     * class lookup, which a natively attached thread cannot do for app classes).
     */
    public void safPickDocuments(String mimes, boolean multiple) {
        SafBridge.pickDocuments(this, mimes, multiple);
    }

    /** Whether the open picker and the copy into the app directory are finished. */
    public boolean safIsDone() {
        return SafBridge.isDone();
    }

    /** Open the "where should this be saved?" picker. */
    public void safPickSave(String title, String fileName, String mime) {
        SafBridge.pickSave(this, title, fileName, mime);
    }

    public boolean safSaveDone() {
        return SafBridge.isSaveDone();
    }

    public boolean safHasSaveUri() {
        return SafBridge.hasSaveUri();
    }

    /** Copy a finished file to the destination the user chose. */
    public boolean safPublish(String path) {
        return SafBridge.publish(this, path);
    }
}
