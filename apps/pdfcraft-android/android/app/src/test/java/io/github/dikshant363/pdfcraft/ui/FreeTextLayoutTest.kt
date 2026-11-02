package io.github.dikshant363.pdfcraft.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FreeTextLayoutTest {
    @Test
    fun widthFollowsTheEngineTable() {
        // "Hi" = H 680 + i 260 units
        assertEquals(9.4f, FreeTextLayout.width("Hi", 10f), 0.001f)
    }

    @Test
    fun longTextWrapsAtSpacesWithinTheRoom() {
        val text = "Wrapping test for a long note that runs past the edge of the page"
        val (lines, w, h) = FreeTextLayout.layout(text, 16f, 200f)
        assertTrue(lines.size > 1)
        assertEquals(text, lines.joinToString(" "))
        assertTrue(w <= 200f)
        // each line fits the width the engine will wrap at, so it draws the same lines
        lines.forEach { assertTrue(FreeTextLayout.width(it, 16f) <= w - 2 * FreeTextLayout.PAD) }
        // and every line's baseline is inside the box
        assertTrue(h >= 2 * FreeTextLayout.PAD + 16f * 0.9f + (lines.size - 1) * 16f * 1.2f)
    }

    @Test
    fun typedLineBreaksAreKept() {
        assertEquals(listOf("one", "", "two"), FreeTextLayout.wrap("one\n\ntwo", 12f, 500f))
    }

    @Test
    fun aWordWiderThanTheRoomBreaksByLetters() {
        val lines = FreeTextLayout.wrap("mmmmmmmmmm", 10f, 30f)
        assertEquals("mmmmmmmmmm", lines.joinToString(""))
        lines.forEach { assertTrue(FreeTextLayout.width(it, 10f) <= 30f) }
    }
}
