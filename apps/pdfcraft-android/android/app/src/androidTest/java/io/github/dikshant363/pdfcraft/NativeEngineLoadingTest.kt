package io.github.dikshant363.pdfcraft

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.dikshant363.pdfcraft.core.MobileSession
import io.github.dikshant363.pdfcraft.core.engineVersion
import io.github.dikshant363.pdfcraft.core.initLogging
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Validates that the native Rust PDF engine library links, loads, and initializes
 * correctly on real Android runtime and emulators across supported ABIs.
 */
@RunWith(AndroidJUnit4::class)
class NativeEngineLoadingTest {

    @Test
    fun nativeLibraryLoadsAndReportsVersion() {
        initLogging()
        val version = engineVersion()
        assertNotNull("Engine version string must not be null", version)
        assertTrue("Engine version should start with 0.", version.startsWith("0."))
    }

    @Test
    fun mobileSessionInitializesAndExecutesOnDevice() {
        val appContext = InstrumentationRegistry.getInstrumentation().targetContext
        assertNotNull(appContext)
        val session = MobileSession()
        assertNotNull(session)
    }
}
