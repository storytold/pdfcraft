package io.github.dikshant363.pdfcraft.ui

import androidx.compose.ui.geometry.Offset
import io.github.dikshant363.pdfcraft.ui.viewer.DocLayout
import io.github.dikshant363.pdfcraft.ui.viewer.PageMode
import io.github.dikshant363.pdfcraft.ui.viewer.PageSize
import io.github.dikshant363.pdfcraft.ui.viewer.ViewerState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ViewerGeometryTest {
    private val letter = PageSize(612f, 792f)
    private val landscape = PageSize(792f, 612f)

    private fun state(pages: List<PageSize>, w: Float = 1080f, h: Float = 2000f) = ViewerState(pages).apply {
        gap = 20f
        setViewport(w, h)
    }

    @Test
    fun continuousStacksPagesFittedToTheWidestPage() {
        val l = DocLayout(listOf(letter, landscape, letter), PageMode.Continuous, 1080f, 2000f, gap = 20f, margin = 0f, padTop = 100f, padBottom = 50f)
        // the widest page (landscape, 792 pt) fills the width; the others share its scale
        assertEquals(1080f, l.rects[1].width, 0.01f)
        assertEquals(612f * 1080f / 792f, l.rects[0].width, 0.01f)
        assertEquals(100f, l.rects[0].top, 0.01f)
        assertEquals(l.rects[0].bottom + 20f, l.rects[1].top, 0.01f)
        assertEquals(l.rects[2].bottom + 50f, l.height, 0.01f)
        // narrower pages are centred
        assertEquals((1080f - l.rects[0].width) / 2f, l.rects[0].left, 0.01f)
    }

    @Test
    fun visibleAndHitTesting() {
        val pages = List(100) { letter }
        val l = DocLayout(pages, PageMode.Continuous, 1080f, 2000f, 20f, 0f, 0f, 0f)
        val h = l.rects[0].height
        assertEquals(0..0, l.visible(0f, 0f, 1080f, h + 10f))
        assertEquals(0..1, l.visible(0f, 0f, 1080f, h + 30f))
        assertEquals(50..51, l.visible(0f, l.rects[50].top + 1f, 1080f, l.rects[51].top + 1f))
        assertEquals(3, l.pageAt(500f, l.rects[3].top + 5f))
        // the gap between pages belongs to none
        assertNull(l.pageAt(500f, l.rects[3].bottom + 10f))
        assertEquals(4, l.nearestPage(500f, l.rects[3].bottom + 15f))
        assertEquals(3, l.nearestPage(500f, l.rects[3].bottom + 5f))
    }

    @Test
    fun singlePageFitsEachPageInItsOwnSlot() {
        val l = DocLayout(listOf(letter, landscape), PageMode.SinglePage, 1000f, 2000f, 20f, 0f, 0f, 0f)
        assertEquals(1000f, l.rects[0].width, 0.01f)
        assertTrue(l.rects[1].left >= 1000f)
        assertEquals(1000f, l.rects[1].width, 0.01f)
        assertEquals(1000f, (l.rects[0].top + l.rects[0].bottom) - 1000f, 1f) // centred vertically in 2000
        assertEquals(2000f, l.width, 0.01f)
    }

    @Test
    fun zoomKeepsTheFocusPointStill() {
        val s = state(List(5) { letter })
        s.scrollBy(0f, -900f)
        val focus = Offset(400f, 700f)
        val before = s.screenToPage(focus)
        s.zoomBy(2f, focus)
        val after = s.screenToPage(focus)
        assertNotNull(before)
        assertEquals(before!!.page, after!!.page)
        assertEquals(before.x, after.x, 0.5f)
        assertEquals(before.y, after.y, 0.5f)
        assertEquals(2f, s.zoom, 0.0001f)
    }

    @Test
    fun zoomAndScrollStayInBounds() {
        val s = state(List(3) { letter })
        s.zoomTo(100f, Offset.Zero)
        assertEquals(ViewerState.MAX_ZOOM, s.zoom, 0.0001f)
        s.zoomTo(0.01f, Offset.Zero)
        assertEquals(ViewerState.ZOOM_OUT_LIMIT, s.zoom, 0.0001f)
        s.zoomTo(1f, Offset.Zero)
        s.scrollBy(0f, 1_000_000f)
        assertEquals(0f, s.scrollY, 0.0001f)
        s.scrollBy(0f, -1_000_000f)
        assertEquals(s.layout.height - s.viewH, s.scrollY, 0.5f)
    }

    @Test
    fun continuousZoomsOutPastFitWidthAndCentresTheDocument() {
        val s = state(List(3) { letter })
        s.zoomTo(0.5f, Offset(540f, 700f))
        assertEquals(0.5f, s.zoom, 0.0001f)
        // the document is narrower than the screen: centred horizontally
        assertEquals(-(s.viewW - s.layout.width * 0.5f) / 2f, s.scrollX, 0.5f)
        // pages are mapped back correctly while zoomed out
        val p = s.screenToPage(s.pageToScreen(1, 100f, 200f))!!
        assertEquals(1, p.page)
        assertEquals(100f, p.x, 0.1f)
        assertEquals(200f, p.y, 0.1f)
    }

    @Test
    fun singlePageStaysAtFitWidth() {
        val s = state(List(3) { letter })
        s.switchMode(PageMode.SinglePage)
        s.zoomTo(0.3f, Offset.Zero)
        assertEquals(ViewerState.MIN_ZOOM, s.zoom, 0.0001f)
    }

    @Test
    fun aPinchEndingJustShortOfFitWidthSpringsBack() {
        val s = state(List(3) { letter })
        s.zoomTo(0.95f, Offset(540f, 700f))
        assertEquals(ViewerState.MIN_ZOOM, s.pinchRest()!!, 0.0001f)
        s.zoomTo(0.5f, Offset(540f, 700f))
        assertEquals(null, s.pinchRest())
        s.zoomTo(1.5f, Offset(540f, 700f))
        assertEquals(null, s.pinchRest())
    }

    @Test
    fun zoomedOutPagesRenderAtHalfScale() {
        val s = state(List(20) { letter })
        val full = s.requests(1, 100f).first()
        assertEquals(0, full.key.level)
        s.zoomTo(0.3f, Offset(540f, 700f))
        val out = s.requests(1, 100f)
        assertTrue(out.isNotEmpty())
        assertTrue(out.all { it.key.level == ViewerState.LOWRES_LEVEL && it.tile == null })
        assertEquals(full.scale * 0.5f, out.first().scale, 0.001f)
    }

    @Test
    fun pageMappingRoundTrips() {
        val s = state(List(4) { letter })
        s.zoomTo(3f, Offset(300f, 300f))
        s.goToPage(2, 400f)
        val screen = s.pageToScreen(2, 100f, 400f)
        val back = s.screenToPage(screen)!!
        assertEquals(2, back.page)
        assertEquals(100f, back.x, 0.1f)
        assertEquals(400f, back.y, 0.1f)
    }

    @Test
    fun zoomStepsAddAQuarterAndStopAtTheEnds() {
        val s = state(List(3) { letter })
        s.zoomStep(1, Offset.Zero)
        assertEquals(1.25f, s.zoom, 0.0001f)
        s.zoomStep(-2, Offset.Zero)
        assertEquals(0.75f, s.zoom, 0.0001f)
        assertTrue(s.canZoomIn && s.canZoomOut)
        s.zoomTo(ViewerState.MAX_ZOOM, Offset.Zero)
        assertTrue(!s.canZoomIn)
        // a single page zoomed back out to fit-width settles on its own slot
        s.switchMode(PageMode.SinglePage)
        s.goToPage(1)
        s.zoomStep(2, Offset(300f, 300f))
        s.zoomStep(-2, Offset(300f, 300f))
        assertEquals(1f, s.zoom, 0.0001f)
        assertTrue(!s.canZoomOut)
        assertEquals(s.viewW, s.scrollX, 0.5f)
    }

    @Test
    fun aDragOffItsPageIsHeldToThatPage() {
        val s = state(List(4) { letter })
        s.goToPage(1)
        val top = s.pageToScreen(1, 0f, 0f)
        // the gap above page 1 belongs to no page, but a drag that began on page 1 stays on it
        val above = Offset(top.x + 50f, top.y - 10f)
        assertNull(s.screenToPage(above))
        val held = s.screenToPageOf(1, above)!!
        assertEquals(1, held.page)
        assertEquals(0f, held.y, 0.001f)
        // and past the page's far corner, it is held to that corner
        val far = s.pageToScreen(1, 612f, 792f) + Offset(40f, 40f)
        val corner = s.screenToPageOf(1, far)!!
        assertEquals(612f, corner.x, 0.001f)
        assertEquals(792f, corner.y, 0.001f)
        assertNull(s.screenToPageOf(9, above))
    }

    @Test
    fun currentPageFollowsScroll() {
        val s = state(List(10) { letter })
        assertEquals(0, s.currentPage)
        s.goToPage(6)
        assertEquals(6, s.currentPage)
    }

    @Test
    fun zoomedInViewAsksForTilesOfTheVisibleAreaOnly() {
        val s = state(listOf(letter))
        assertEquals(0, s.detailLevel())
        assertTrue(s.requests(1, 100f).all { it.tile == null })
        s.zoomTo(4f, Offset(540f, 700f))
        val level = s.detailLevel()
        assertTrue(level >= 3)
        val tiles = s.requests(1, 100f).filter { it.tile != null }
        assertTrue(tiles.isNotEmpty())
        // tiles cover about one screen at the level's scale, not the whole page
        val pagePixels = 612f * s.levelScale(0, level, 100f) * 792f * s.levelScale(0, level, 100f)
        val tilePixels = tiles.sumOf { (it.tile!!.width * it.tile!!.height).toDouble() }
        assertTrue(tilePixels < pagePixels / 4)
        // every tile is within the page
        val pw = kotlin.math.ceil(612f * s.levelScale(0, level, 100f)).toInt()
        assertTrue(tiles.all { (it.tile!!.x + it.tile!!.width).toInt() <= pw })
    }

    @Test
    fun switchingToSinglePageKeepsThePage() {
        val s = state(List(8) { letter })
        s.goToPage(5)
        s.switchMode(PageMode.SinglePage)
        assertEquals(5, s.currentPage)
        assertEquals(5 * 1080f, s.scrollX, 0.5f)
        assertEquals(6 * 1080f, s.snapTarget(-2000f), 0.5f)
        assertEquals(5 * 1080f, s.snapTarget(0f), 0.5f)
    }
}
