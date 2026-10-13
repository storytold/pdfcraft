package com.storytold.pdfcraft.kit

import android.graphics.RectF
import org.junit.Assert.*
import org.junit.Test
import uniffi.pdfcraft_ffi.FfiPageGeometry

class PdfTileRendererTest {
    private fun rect(l: Float, t: Float, r: Float, b: Float): RectF {
        val res = RectF()
        res.left = l
        res.top = t
        res.right = r
        res.bottom = b
        return res
    }

    @Test
    fun testComputeTileNormalScale() {
        val geom = FfiPageGeometry(width = 612f, height = 792f, rotation = 0u, label = "1")
        val r = rect(0f, 0f, 256f, 256f)
        val tile = PdfTileRenderer.computeTile(geom, r, 1.0f)

        assertNotNull(tile)
        assertEquals(0u, tile!!.x)
        assertEquals(0u, tile.y)
        assertEquals(256u, tile.width)
        assertEquals(256u, tile.height)
    }

    @Test
    fun testComputeTileZoomedIn() {
        val geom = FfiPageGeometry(width = 612f, height = 792f, rotation = 0u, label = "1")
        val r = rect(100f, 100f, 356f, 356f) // 256x256 points
        val tile = PdfTileRenderer.computeTile(geom, r, 2.0f)

        assertNotNull(tile)
        assertEquals(200u, tile!!.x)
        assertEquals(200u, tile.y)
        assertEquals(512u, tile.width)
        assertEquals(512u, tile.height)
    }

    @Test
    fun testComputeTileEdgeClamping() {
        val geom = FfiPageGeometry(width = 612f, height = 792f, rotation = 0u, label = "1")
        val r = rect(500f, 0f, 700f, 100f) // exceeds right edge (612)
        val tile = PdfTileRenderer.computeTile(geom, r, 1.0f)

        assertNotNull(tile)
        assertEquals(500u, tile!!.x)
        assertEquals(112u, tile.width) // 612 - 500 = 112
    }

    @Test
    fun testComputeTileOutOfBounds() {
        val geom = FfiPageGeometry(width = 612f, height = 792f, rotation = 0u, label = "1")
        val r = rect(700f, 900f, 800f, 1000f)
        val tile = PdfTileRenderer.computeTile(geom, r, 1.0f)

        assertNull(tile)
    }
}
