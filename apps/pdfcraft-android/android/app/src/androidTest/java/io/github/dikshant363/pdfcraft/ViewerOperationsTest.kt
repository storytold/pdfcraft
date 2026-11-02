package io.github.dikshant363.pdfcraft

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.dikshant363.pdfcraft.core.MobileSession
import io.github.dikshant363.pdfcraft.core.TileRect
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.io.FileOutputStream

@RunWith(AndroidJUnit4::class)
class ViewerOperationsTest {

    private fun createPdfWithContent(file: File): File {
        val pdf = """
            %PDF-1.4
            1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
            2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
            3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
            4 0 obj << /Length 44 >> stream
            BT /F1 24 Tf 100 700 Td (Hello Android) Tj ET
            endstream endobj
            5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
            xref
            0 6
            0000000000 65535 f 
            0000000009 00000 n 
            0000000058 00000 n 
            0000000115 00000 n 
            0000000244 00000 n 
            0000000338 00000 n 
            trailer << /Size 6 /Root 1 0 R >>
            startxref
            418
            %%EOF
        """.trimIndent()
        FileOutputStream(file).use { it.write(pdf.toByteArray(Charsets.US_ASCII)) }
        return file
    }

    @Test
    fun rendersPageAndTilesAtMultipleScales() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val file = File(context.cacheDir, "render_test.pdf")
        createPdfWithContent(file)

        val session = MobileSession()
        val info = session.open(file.absolutePath, null)

        // Render full page raster
        val full = session.renderPage(info.id, 0u, 1.0f, null)
        assertNotNull(full)
        assertTrue(full.width > 0u)
        assertTrue(full.height > 0u)
        assertTrue(full.rgba.isNotEmpty())

        // Render viewport tile (pinch zoom)
        val tile = session.renderPage(info.id, 0u, 2.0f, TileRect(0u, 0u, 256u, 256u))
        assertNotNull(tile)
        assertEquals(256u, tile.width)
        assertEquals(256u, tile.height)
        assertEquals(256 * 256 * 4, tile.rgba.size)

        session.close(info.id, false)
    }

    @Test
    fun textSearchFindsMatchesAcrossDocument() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val file = File(context.cacheDir, "search_test.pdf")
        createPdfWithContent(file)

        val session = MobileSession()
        val info = session.open(file.absolutePath, null)

        val hits = session.findInPage(info.id, 0u, "Hello", false, false)
        assertNotNull(hits)
        assertTrue("Search should locate 'Hello' on page 1", hits.isNotEmpty())
        assertEquals(0u, hits[0].range.page)

        session.close(info.id, false)
    }
}
