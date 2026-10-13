package com.storytold.pdfcraft.kit

import android.graphics.Bitmap
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.pdfcraft_ffi.FfiTile
import java.io.File
import java.io.FileOutputStream

@RunWith(AndroidJUnit4::class)
class PdfScreenshotsInstrumentedTest {

    data class Case(
        val name: String,
        val filename: String,
        val password: String? = null
    )

    private val cases = listOf(
        Case("truetype_subset", "issue14847.pdf"),
        Case("cff_type1c", "issue7492.pdf"),
        Case("jpeg_dct", "cmykjpeg.pdf"),
        Case("scanned_ccitt", "ccitt_EndOfBlock_false.pdf"),
        Case("cjk_embedded", "issue19971.pdf"),
        Case("cjk_non_embedded", "issue8372.pdf"),
        Case("transparency", "transparency_group.pdf"),
        Case("encrypted", "issue15893_reduced.pdf", "test"),
        Case("complex_vector", "colors.pdf"),
        Case("outlines_forms", "nested_outline.pdf")
    )

    @Test
    fun testRenderFitAnd4xZoomAcrossTileBoundary() {
        val appContext = InstrumentationRegistry.getInstrumentation().targetContext
        val outDir = File(appContext.getExternalFilesDir(null), "pdfcraft_screenshots")
        outDir.mkdirs()

        val corpusDir = File(appContext.getExternalFilesDir(null), "corpus")
        for (c in cases) {
            android.util.Log.i("PdfScreenshots", "Processing ${c.name} (${c.filename})...")
            val file = File(corpusDir, c.filename)
            assertTrue("PDF file must exist: ${file.absolutePath}", file.exists())

            val bytes = file.readBytes()
            val doc = PdfDocument.openBytes(name = c.filename, bytes = bytes, password = c.password)
            assertTrue("Document page count > 0", doc.pageCount > 0)

            val geom = doc.geometry(0)
            assertTrue("Page dimensions positive", geom.width > 0f && geom.height > 0f)

            // 1. Fit-to-screen full page render (scale = 1.0f)
            val bmpFit = doc.renderPage(page = 0, scale = 1.0f)
            assertNotNull("Fit render must succeed for ${c.name}", bmpFit)
            assertEquals("Config must be ARGB_8888", Bitmap.Config.ARGB_8888, bmpFit!!.config)

            val fitFile = File(outDir, "${c.name}_fit.png")
            FileOutputStream(fitFile).use { out ->
                bmpFit.compress(Bitmap.CompressFormat.PNG, 100, out)
            }
            assertTrue("Fit screenshot saved", fitFile.exists() && fitFile.length() > 0)

            // 2. 4x zoom across a tile boundary
            // We request two horizontally adjacent tiles at 4x zoom:
            // Tile A: x=0, y=0, width=256, height=256
            // Tile B: x=256, y=0, width=256, height=256
            val tileA = FfiTile(x = 0u, y = 0u, width = 256u, height = 256u)
            val tileB = FfiTile(x = 256u, y = 0u, width = 256u, height = 256u)

            val bmpTileA = doc.renderTile(page = 0, tile = tileA, scale = 4.0f)
            assertNotNull("Tile A at 4x must succeed for ${c.name}", bmpTileA)

            // If page is wide enough for tile B at 4x (page width * 4 > 256):
            val bmpTileB = if ((geom.width * 4f) > 256f) {
                doc.renderTile(page = 0, tile = tileB, scale = 4.0f)
            } else {
                null
            }

            // Stitch them side-by-side to verify seam at x=256
            val seamW = if (bmpTileB != null) 512 else 256
            val bmpSeam = Bitmap.createBitmap(seamW, 256, Bitmap.Config.ARGB_8888)
            val canvas = android.graphics.Canvas(bmpSeam)
            canvas.drawBitmap(bmpTileA!!, 0f, 0f, null)
            if (bmpTileB != null) {
                canvas.drawBitmap(bmpTileB, 256f, 0f, null)
            }

            val seamFile = File(outDir, "${c.name}_4x_seam.png")
            FileOutputStream(seamFile).use { out ->
                bmpSeam.compress(Bitmap.CompressFormat.PNG, 100, out)
            }
            assertTrue("Seam screenshot saved", seamFile.exists() && seamFile.length() > 0)

            doc.close()
        }
    }
}
