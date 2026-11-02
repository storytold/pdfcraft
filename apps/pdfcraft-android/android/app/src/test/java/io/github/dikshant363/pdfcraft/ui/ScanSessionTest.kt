package io.github.dikshant363.pdfcraft.ui

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ScanSessionTest {
    private val crop = floatArrayOf(0.1f, 0.1f, 0.9f, 0.12f, 0.88f, 0.95f, 0.08f, 0.9f)

    @Test
    fun aScanComesBackAsItWas() {
        val s = ScanSession(
            shot = "/cache/camera/scan-3.jpg", retake = 1, forImage = false, name = "Scan 2026-10-29",
            pages = listOf(ScanSession.Page("/cache/camera/scan-1.jpg", crop, crop), ScanSession.Page("/cache/camera/scan-2.jpg", ScanPage.WHOLE, null)),
        )
        val back = ScanSession.decode(s.encode())!!
        assertEquals("/cache/camera/scan-3.jpg", back.shot)
        assertEquals(1, back.retake)
        assertEquals(false, back.forImage)
        assertEquals("Scan 2026-10-29", back.name)
        assertEquals(listOf("/cache/camera/scan-1.jpg", "/cache/camera/scan-2.jpg"), back.pages.map { it.path })
        assertArrayEquals(crop, back.pages[0].corners, 0f)
        assertArrayEquals(crop, back.pages[0].found, 0f)
        assertArrayEquals(ScanPage.WHOLE, back.pages[1].corners, 0f)
        assertNull(back.pages[1].found)
    }

    @Test
    fun aPhotoForAddImageWithNoPagesComesBack() {
        val back = ScanSession.decode(ScanSession("/cache/camera/image-1.jpg", null, true, "", emptyList()).encode())!!
        assertEquals("/cache/camera/image-1.jpg", back.shot)
        assertNull(back.retake)
        assertTrue(back.forImage)
        assertTrue(back.pages.isEmpty())
    }

    @Test
    fun aNameCannotBreakTheLines() {
        val back = ScanSession.decode(ScanSession(null, null, false, "two\nlines\tand a tab", emptyList()).encode())!!
        assertEquals("two lines and a tab", back.name)
        assertNull(back.shot)
    }

    @Test
    fun anythingElseIsNoScanAndABadPageIsLeftOut() {
        assertNull(ScanSession.decode(null))
        assertNull(ScanSession.decode(""))
        assertNull(ScanSession.decode("something else\nshot\t/x.jpg"))
        val back = ScanSession.decode("pdfcraft-scan 1\npage\t/a.jpg\t1,2,3\t\npage\t/b.jpg\t0,0,1,0,1,1,0,1\tNaN,0,1,0,1,1,0,1\npage\t\t0,0,1,0,1,1,0,1\t")!!
        assertEquals(listOf("/b.jpg"), back.pages.map { it.path })
        assertNull(back.pages[0].found)
    }
}
