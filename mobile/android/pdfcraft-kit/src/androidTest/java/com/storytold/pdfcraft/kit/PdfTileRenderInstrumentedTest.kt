package com.storytold.pdfcraft.kit

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.pdfcraft_ffi.FfiTile

@RunWith(AndroidJUnit4::class)
class PdfTileRenderInstrumentedTest {

    private fun samplePdf(): ByteArray {
        val stream = "1 0 0 rg 50 50 100 100 re f\n0 0 1 rg 80 80 40 40 re f"
        val pdf = "%PDF-1.4\n" +
            "1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n" +
            "2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n" +
            "3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >> endobj\n" +
            "4 0 obj << /Length ${stream.length} >> stream\n$stream\nendstream endobj\n" +
            "trailer << /Root 1 0 R >>\n%%EOF"
        return pdf.toByteArray(Charsets.US_ASCII)
    }

    @Test
    fun testOpenFixtureRenderTilesAndAssertNonBlankStatistics() {
        val bytes = samplePdf()
        val doc = PdfDocument.openBytes(name = "test_instrumented.pdf", bytes = bytes)
        assertEquals(1, doc.pageCount)

        val geom = doc.geometry(0)
        assertEquals(200f, geom.width)
        assertEquals(200f, geom.height)

        // Render 4 tiles covering the 200x200 page at scale 1.0 (100x100 pixels each)
        val tiles = listOf(
            FfiTile(x = 0u, y = 0u, width = 100u, height = 100u),
            FfiTile(x = 100u, y = 0u, width = 100u, height = 100u),
            FfiTile(x = 0u, y = 100u, width = 100u, height = 100u),
            FfiTile(x = 100u, y = 100u, width = 100u, height = 100u)
        )

        var totalNonWhitePixels = 0
        var totalPixels = 0

        for ((idx, tile) in tiles.withIndex()) {
            val bmp = doc.renderTile(page = 0, tile = tile, scale = 1.0f)
            assertNotNull("Tile $idx must render successfully", bmp)
            assertEquals(100, bmp!!.width)
            assertEquals(100, bmp.height)

            var tileNonWhite = 0
            for (y in 0 until bmp.height) {
                for (x in 0 until bmp.width) {
                    val pixel = bmp.getPixel(x, y)
                    // Non-blank check: colored pixel
                    if (pixel != -1 && pixel != 0) {
                        tileNonWhite++
                    }
                }
            }
            totalPixels += bmp.width * bmp.height
            totalNonWhitePixels += tileNonWhite
        }

        doc.close()

        // Assert non-blank pixel statistics
        assertTrue(
            "Rendered tiles must contain colored/non-blank pixels (measured: $totalNonWhitePixels / $totalPixels)",
            totalNonWhitePixels > 500
        )
    }
}
