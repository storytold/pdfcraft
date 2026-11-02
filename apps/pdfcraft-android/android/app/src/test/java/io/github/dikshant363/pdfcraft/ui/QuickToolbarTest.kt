package io.github.dikshant363.pdfcraft.ui

import io.github.dikshant363.pdfcraft.ui.viewer.QuickTool
import io.github.dikshant363.pdfcraft.ui.viewer.QuickToolbarPrefs
import io.github.dikshant363.pdfcraft.ui.viewer.toolbarSlots
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class QuickToolbarTest {
    @Test
    fun slotsGrowWithTheScreen() {
        assertEquals(6, toolbarSlots(411))
        assertEquals(7, toolbarSlots(460))
        assertEquals(8, toolbarSlots(800))
        // the default bar fits a phone held upright
        assertTrue(QuickTool.DEFAULT_BAR.size <= toolbarSlots(360))
    }

    @Test
    fun moreToolsHoldsEveryToolNotOnTheBar() {
        val bar = listOf(QuickTool.Comment, QuickTool.Scan)
        val grid = QuickToolbarPrefs.complete(listOf(QuickTool.ReadAloud, QuickTool.Scan, QuickTool.ReadAloud), bar)
        // the saved order first, without the bar's tools or repeats, then the rest
        assertEquals(QuickTool.ReadAloud, grid.first())
        assertEquals(QuickTool.entries.size - bar.size, grid.size)
        assertTrue(grid.none { it in bar })
        assertEquals(grid.size, grid.toSet().size)
    }

    @Test
    fun idsAreStable() {
        assertEquals(QuickTool.entries.size, QuickTool.entries.map { it.id }.toSet().size)
        for (t in QuickTool.entries) assertEquals(t, QuickTool.of(t.id))
    }
}
