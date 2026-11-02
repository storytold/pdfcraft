package io.github.dikshant363.pdfcraft

import android.content.Intent
import android.net.Uri
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.dikshant363.pdfcraft.core.MobileSession
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.io.FileOutputStream

@RunWith(AndroidJUnit4::class)
class DocumentLifecycleTest {

    private fun createMinimalPdf(file: File): File {
        val minimalPdf = """
            %PDF-1.4
            1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
            2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
            3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources <<>> >> endobj
            xref
            0 4
            0000000000 65535 f 
            0000000009 00000 n 
            0000000058 00000 n 
            0000000115 00000 n 
            trailer << /Size 4 /Root 1 0 R >>
            startxref
            194
            %%EOF
        """.trimIndent()
        FileOutputStream(file).use { it.write(minimalPdf.toByteArray(Charsets.US_ASCII)) }
        return file
    }

    @Test
    fun opensDocumentFromSandboxedStorage() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val pdfFile = File(context.cacheDir, "instrumentation_test.pdf")
        createMinimalPdf(pdfFile)
        assertTrue(pdfFile.exists())

        val session = MobileSession()
        val info = session.open(pdfFile.absolutePath, null)
        assertNotNull(info)
        assertEquals(1, info.pages.size)
        assertEquals(612f, info.pages[0].width, 0.1f)
        assertEquals(792f, info.pages[0].height, 0.1f)
        session.close(info.id, false)
    }

    @Test
    fun externalViewIntentLaunchesMainActivity() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val pdfFile = File(context.cacheDir, "intent_launch.pdf")
        createMinimalPdf(pdfFile)

        val intent = Intent(context, MainActivity::class.java).apply {
            action = Intent.ACTION_VIEW
            setDataAndType(Uri.fromFile(pdfFile), "application/pdf")
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }

        val scenario = ActivityScenario.launch<MainActivity>(intent)
        scenario.onActivity { activity ->
            assertNotNull(activity)
            assertNotNull(activity.intent)
            assertEquals(Intent.ACTION_VIEW, activity.intent.action)
        }
        scenario.close()
    }
}
