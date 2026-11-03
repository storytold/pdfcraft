package io.github.dikshant363.pdfcraft

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
class AnnotationsAndMediaTest {

    private fun createPdf(file: File): File {
        val pdf = """
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
        FileOutputStream(file).use { it.write(pdf.toByteArray(Charsets.US_ASCII)) }
        return file
    }

    @Test
    fun addsNoteAnnotationAndSurvivesSaveReopen() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val file = File(context.cacheDir, "annot_test.pdf")
        val savedFile = File(context.cacheDir, "annot_saved.pdf")
        createPdf(file)

        val session = MobileSession()
        val info = session.open(file.absolutePath, null)

        val toolArgs = """
            {
                "doc": ${info.id},
                "page": 1,
                "type": "note",
                "at": [100.0, 500.0],
                "contents": "Test mobile annotation"
            }
        """.trimIndent()

        val result = session.execute("comment_add", toolArgs)
        assertNotNull(result)

        // Save to new destination via doc_save tool
        val saveArgs = """
            {
                "doc": ${info.id},
                "path": "${savedFile.absolutePath}"
            }
        """.trimIndent()
        session.execute("doc_save", saveArgs)
        session.close(info.id, false)
        assertTrue(savedFile.exists())

        // Reopen and verify annotation
        val reopened = session.open(savedFile.absolutePath, null)
        val comments = session.comments(reopened.id)
        assertNotNull(comments)
        assertEquals(1, comments.size)
        assertEquals("Test mobile annotation", comments[0].contents)

        session.close(reopened.id, false)
    }
}
