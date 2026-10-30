package com.storytold.pdfcraft.kit

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.pdfcraft_ffi.FfiTile
import java.io.File

@RunWith(AndroidJUnit4::class)
class PdfSoakInstrumentedTest {

    @Test
    fun testFifteenMinuteSoakLoop() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val corpusDir = File(context.getExternalFilesDir(null), "corpus")
        assertTrue("Corpus directory must exist: ${corpusDir.absolutePath}", corpusDir.exists())

        val testFiles = listOf(
            "issue14847.pdf",
            "nested_outline.pdf",
            "cmykjpeg.pdf",
            "colors.pdf",
            "transparency_group.pdf"
        )

        val args = InstrumentationRegistry.getArguments()
        val durationMs = args.getString("soakDurationMs")?.toLongOrNull() ?: 900_000L // 15 minutes default (900 seconds)
        val startTime = System.currentTimeMillis()
        var cycles = 0
        var totalTilesRendered = 0

        println("SOAK_TEST_START: duration=${durationMs}ms target=emulator-5554")

        while (System.currentTimeMillis() - startTime < durationMs) {
            for (filename in testFiles) {
                val file = File(corpusDir, filename)
                if (!file.exists()) continue

                val doc = PdfDocument.openFile(path = file.absolutePath)
                assertNotNull("Document must open: $filename", doc)

                val pageCount = doc.pageCount
                // Scroll through all pages of the document
                for (p in 0 until pageCount) {
                    val geom = doc.geometry(p)
                    val w = (geom.width * 1.0f).toInt().coerceAtLeast(1)
                    val h = (geom.height * 1.0f).toInt().coerceAtLeast(1)
                    val halfW = (w / 2).coerceAtLeast(1)
                    val halfH = (h / 2).coerceAtLeast(1)

                    val tiles = listOf(
                        FfiTile(x = 0u, y = 0u, width = halfW.toUInt(), height = halfH.toUInt()),
                        FfiTile(x = halfW.toUInt(), y = 0u, width = (w - halfW).toUInt(), height = halfH.toUInt()),
                        FfiTile(x = 0u, y = halfH.toUInt(), width = halfW.toUInt(), height = (h - halfH).toUInt()),
                        FfiTile(x = halfW.toUInt(), y = halfH.toUInt(), width = (w - halfW).toUInt(), height = (h - halfH).toUInt())
                    )

                    for (tile in tiles) {
                        val bmp = doc.renderTile(page = p, tile = tile, scale = 1.0f)
                        assertNotNull("Render tile must succeed", bmp)
                        totalTilesRendered++
                    }
                }

                doc.close()
            }

            cycles++
            Thread.sleep(50) // Modest throttle for realistic user cadence and GC scheduling
        }

        val elapsedSec = (System.currentTimeMillis() - startTime) / 1000
        println("SOAK_TEST_COMPLETE: elapsed=${elapsedSec}s cycles=$cycles tiles=$totalTilesRendered")
        assertTrue("Completed at least 10 full document cycles", cycles >= 10)
    }
}
