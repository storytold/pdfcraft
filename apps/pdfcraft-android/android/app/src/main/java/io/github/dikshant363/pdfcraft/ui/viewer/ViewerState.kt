package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import io.github.dikshant363.pdfcraft.core.TileRect
import kotlin.math.ceil
import kotlin.math.floor
import kotlin.math.ln
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.roundToInt

/** A point on a page: its 0-based index and the position in points from the page's top-left. */
data class PagePoint(val page: Int, val x: Float, val y: Float)

/**
 * The view of one document: its layout, zoom and scroll, in screen pixels. Screen = layout ×
 * [zoom] − scroll. Every tool that acts on the page maps through [screenToPage] and [pageToScreen].
 */
@Stable
class ViewerState(pages: List<PageSize>) {
    var pages by mutableStateOf(pages)
    var mode by mutableStateOf(PageMode.Continuous)

    var viewW by mutableFloatStateOf(0f)
        private set
    var viewH by mutableFloatStateOf(0f)
        private set

    /** Layout spacing, in pixels (set from dp by the canvas). */
    var gap = 0f
    var margin = 0f
    var padTop by mutableFloatStateOf(0f)
    var padBottom by mutableFloatStateOf(0f)

    var zoom by mutableFloatStateOf(1f)
        private set
    var scrollX by mutableFloatStateOf(0f)
        private set
    var scrollY by mutableFloatStateOf(0f)
        private set

    private var cachedLayout: DocLayout? = null

    val layout: DocLayout
        get() {
            val c = cachedLayout
            if (c != null && c.pages === pages && c.mode == mode && c.viewW == viewW && c.viewH == viewH && c.padTop == padTop && c.padBottom == padBottom) return c
            return DocLayout(pages, mode, viewW, viewH, gap, margin, padTop, padBottom).also { cachedLayout = it }
        }

    /** Continuous pages zoom out past fit-width; single-page mode rests on one page filling the width. */
    val minZoom: Float get() = if (mode == PageMode.Continuous) ZOOM_OUT_LIMIT else MIN_ZOOM
    val maxZoom: Float get() = MAX_ZOOM

    /**
     * Where a pinch that ended here comes to rest: fit-width when it stopped just short of it (a
     * slight two-finger touch shouldn't leave thin margins at the sides), else where it is (null).
     */
    fun pinchRest(): Float? = if (mode == PageMode.Continuous && zoom >= SNAP_TO_FIT && zoom < MIN_ZOOM) MIN_ZOOM else null

    /** The page the reader is on: the one at the middle of the screen. */
    val currentPage: Int
        get() {
            val l = layout
            if (l.rects.isEmpty()) return 0
            return when (mode) {
                PageMode.Continuous -> {
                    // the page under the top third, as a reader's eye is
                    val y = (scrollY + viewH * 0.33f) / zoom
                    l.nearestPage(viewW / 2f, y)
                }
                PageMode.SinglePage -> l.nearestPage((scrollX + viewW / 2f) / zoom, 0f)
            }
        }

    /** The viewport changed size: keep the same spot of the document in view. */
    fun setViewport(w: Float, h: Float) {
        if (w == viewW && h == viewH) return
        val first = viewW == 0f
        val anchorPage = if (first) 0 else currentPage
        val l0 = if (first) null else layout
        val within = if (l0 != null && l0.rects.isNotEmpty()) {
            val r = l0.rects[anchorPage]
            ((scrollY / zoom) - r.top) / max(1f, r.height)
        } else 0f
        viewW = w
        viewH = h
        if (!first && layout.rects.isNotEmpty()) {
            val r = layout.rects[anchorPage]
            when (mode) {
                PageMode.Continuous -> { scrollY = (r.top + within * r.height) * zoom; scrollX = scrollX }
                PageMode.SinglePage -> { zoom = 1f; scrollX = anchorPage * viewW; scrollY = 0f }
            }
        }
        clamp()
    }

    /** Change the layout spacing; scroll stays on the same page. */
    fun setPadding(top: Float, bottom: Float) {
        if (top == padTop && bottom == padBottom) return
        val dy = top - padTop
        padTop = top
        padBottom = bottom
        if (mode == PageMode.Continuous && scrollY > 0f) scrollY += dy * zoom
        clamp()
    }

    fun switchMode(m: PageMode) {
        if (m == mode) return
        val page = currentPage
        mode = m
        zoom = 1f
        goToPage(page)
    }

    /** Scroll by a screen delta (a finger moving by [dx], [dy]); returns what was not consumed. */
    fun scrollBy(dx: Float, dy: Float): Offset {
        val ox = scrollX
        val oy = scrollY
        scrollX -= dx
        scrollY -= dy
        clamp()
        return Offset(dx + (scrollX - ox), dy + (scrollY - oy))
    }

    /**
     * Zoom in ([steps] > 0) or out by [ZOOM_STEP] a step about [focus], as the zoom buttons, Ctrl
     * and the wheel, and Ctrl and + or − do; a single page back at fit-width settles on its slot.
     */
    fun zoomStep(steps: Int, focus: Offset) {
        zoomTo(zoom + steps * ZOOM_STEP, focus)
        if (mode == PageMode.SinglePage && zoom <= MIN_ZOOM + 0.001f) goToPage(currentPage)
    }

    val canZoomIn: Boolean get() = zoom < maxZoom - 0.001f
    val canZoomOut: Boolean get() = zoom > minZoom + 0.001f

    /** Zoom by [factor] about screen point [focus]. */
    fun zoomBy(factor: Float, focus: Offset) = zoomTo(zoom * factor, focus)

    fun zoomTo(target: Float, focus: Offset) {
        val z = target.coerceIn(minZoom, maxZoom)
        val lx = (focus.x + scrollX) / zoom
        val ly = (focus.y + scrollY) / zoom
        zoom = z
        scrollX = lx * z - focus.x
        scrollY = ly * z - focus.y
        clamp()
    }

    /** Show page [index] from its top (continuous) or by itself (single page). */
    fun goToPage(index: Int, yPoints: Float? = null) {
        val l = layout
        if (l.rects.isEmpty()) return
        val i = index.coerceIn(0, l.rects.lastIndex)
        val r = l.rects[i]
        when (mode) {
            PageMode.Continuous -> {
                val within = yPoints?.let { it * l.scaleOf(i) } ?: 0f
                scrollY = (r.top + within) * zoom - (if (yPoints == null) padTop * zoom.coerceAtMost(1f) else padTop)
            }
            PageMode.SinglePage -> {
                zoom = 1f
                scrollX = i * viewW
                scrollY = 0f
            }
        }
        clamp()
    }

    /** Show the spot [x], [y] (points) of page [index] at the screen's top-left, at [level] zoom (continuous pages). */
    fun showSpot(index: Int, x: Float, y: Float, level: Float) {
        val l = layout
        if (mode != PageMode.Continuous || l.rects.isEmpty() || !x.isFinite() || !y.isFinite() || !level.isFinite()) {
            goToPage(index)
            return
        }
        val i = index.coerceIn(0, l.rects.lastIndex)
        val r = l.rects[i]
        val s = l.scaleOf(i)
        zoom = level.coerceIn(minZoom, maxZoom)
        scrollX = (r.left + x * s) * zoom
        scrollY = (r.top + y * s) * zoom - padTop
        clamp()
    }

    /** Where single-page mode should rest after a swipe with velocity [vx] (pixels/s). */
    fun snapTarget(vx: Float): Float {
        val page = (scrollX / max(1f, viewW))
        val target = when {
            // a swipe goes to the next (or previous) page even from exactly on a page
            vx < -SNAP_VELOCITY -> floor(page + 1e-3f) + 1f
            vx > SNAP_VELOCITY -> ceil(page - 1e-3f) - 1f
            else -> page.roundToInt().toFloat()
        }
        return target.coerceIn(0f, (pages.size - 1).coerceAtLeast(0).toFloat()) * viewW
    }

    fun setScroll(x: Float, y: Float) {
        scrollX = x
        scrollY = y
        clamp()
    }

    /** Keep the document in view: centred when smaller than the screen, else within its edges. */
    fun clamp() {
        val l = layout
        val cw = l.width * zoom
        val ch = l.height * zoom
        scrollX = if (cw <= viewW) -(viewW - cw) / 2f else scrollX.coerceIn(0f, cw - viewW)
        scrollY = if (ch <= viewH) -(viewH - ch) / 2f else scrollY.coerceIn(0f, ch - viewH)
        if (mode == PageMode.SinglePage && zoom <= 1.001f) scrollY = 0f
    }

    /** Page [i]'s rectangle on screen. */
    fun pageRectOnScreen(i: Int): Rect {
        val r = layout.rects[i]
        return Rect(r.left * zoom - scrollX, r.top * zoom - scrollY, r.right * zoom - scrollX, r.bottom * zoom - scrollY)
    }

    /** The page under screen point [p], in the page's points (top-left origin). */
    fun screenToPage(p: Offset, nearest: Boolean = false): PagePoint? {
        val l = layout
        val lx = (p.x + scrollX) / zoom
        val ly = (p.y + scrollY) / zoom
        val i = l.pageAt(lx, ly) ?: if (nearest && l.rects.isNotEmpty()) l.nearestPage(lx, ly) else return null
        return screenToPageOf(i, p)
    }

    /** Screen point [p] in page [page]'s points, held to the page (a drag that leaves the page). */
    fun screenToPageOf(page: Int, p: Offset): PagePoint? {
        val l = layout
        val r = l.rects.getOrNull(page) ?: return null
        val size = pages.getOrNull(page) ?: return null
        val s = l.scaleOf(page)
        if (s <= 0f) return null
        val lx = (p.x + scrollX) / zoom
        val ly = (p.y + scrollY) / zoom
        return PagePoint(page, ((lx - r.left) / s).coerceIn(0f, size.width), ((ly - r.top) / s).coerceIn(0f, size.height))
    }

    /** Screen position of a point on page [page] given in its points. */
    fun pageToScreen(page: Int, x: Float, y: Float): Offset {
        val l = layout
        val r = l.rects[page]
        val s = l.scaleOf(page)
        return Offset((r.left + x * s) * zoom - scrollX, (r.top + y * s) * zoom - scrollY)
    }

    /** Screen pixels per point on page [page] at the current zoom. */
    fun pixelsPerPoint(page: Int): Float = layout.scaleOf(page) * zoom

    /** Visible pages, plus [extra] screens' worth beyond, in reading order from the current one. */
    fun visiblePages(extra: Float = 0f): IntRange {
        val l = layout
        val mx = viewW * extra
        val my = viewH * extra
        return l.visible((scrollX - mx) / zoom, (scrollY - my) / zoom, (scrollX + viewW + mx) / zoom, (scrollY + viewH + my) / zoom)
    }

    /**
     * What to render for the current view, most wanted first: the visible pages whole, the tiles
     * of the visible area at the current zoom level when zoomed in, then the pages around.
     */
    fun requests(gen: Long, maxScale: Float): List<ImageRequest> {
        if (viewW <= 0f || pages.isEmpty()) return emptyList()
        val out = ArrayList<ImageRequest>()
        val visible = visiblePages()
        for (i in visible) out.add(baseRequest(i, gen, maxScale))
        val level = detailLevel()
        if (level > 0) {
            for (i in visible) tilesFor(i, gen, level, maxScale, out)
        }
        val around = visiblePages(if (zoom < LOWRES_BELOW) 0.5f else 1.5f)
        for (i in around) if (i !in visible) out.add(baseRequest(i, gen, maxScale))
        return out
    }

    /** Zoomed far out, whole pages are drawn from half-scale renders ([LOWRES_LEVEL]); many fit on screen. */
    fun baseLevel(): Int = if (zoom < LOWRES_BELOW) LOWRES_LEVEL else 0

    private fun baseRequest(i: Int, gen: Long, maxScale: Float): ImageRequest {
        val level = baseLevel()
        val factor = if (level == LOWRES_LEVEL) 0.5f else 1f
        return ImageRequest(ImageKey(i, gen, level), min(layout.scaleOf(i) * factor, maxScale), null)
    }

    /** 0 at about zoom 1; then one level per half octave of zoom. */
    fun detailLevel(): Int = if (zoom < 1.2f) 0 else ceil(ln(zoom) / ln(LEVEL_STEP)).toInt().coerceAtLeast(1)

    /** The render scale (pixels per point) of tiles at [level] on page [i]. */
    fun levelScale(i: Int, level: Int, maxScale: Float): Float = min(layout.scaleOf(i) * LEVEL_STEP.pow(level), maxScale)

    private fun tilesFor(i: Int, gen: Long, level: Int, maxScale: Float, out: MutableList<ImageRequest>) {
        val s = levelScale(i, level, maxScale)
        val page = pages[i]
        val pw = ceil(page.width * s).toInt()
        val ph = ceil(page.height * s).toInt()
        val screen = pageRectOnScreen(i)
        // the visible part of the page in its own pixels at scale s
        val k = s / pixelsPerPoint(i)
        val vx0 = max(0f, (0f - screen.left) * k)
        val vy0 = max(0f, (0f - screen.top) * k)
        val vx1 = min(pw.toFloat(), (viewW - screen.left) * k)
        val vy1 = min(ph.toFloat(), (viewH - screen.top) * k)
        if (vx1 <= vx0 || vy1 <= vy0) return
        val cols = (pw + TILE - 1) / TILE
        for (ty in (vy0.toInt() / TILE)..(min(ph - 1, vy1.toInt()) / TILE)) {
            for (tx in (vx0.toInt() / TILE)..(min(pw - 1, vx1.toInt()) / TILE)) {
                val x = tx * TILE
                val y = ty * TILE
                val w = min(TILE, pw - x)
                val h = min(TILE, ph - y)
                if (w <= 0 || h <= 0) continue
                out.add(ImageRequest(ImageKey(i, gen, level, ty * cols + tx), s, TileRect(x.toUInt(), y.toUInt(), w.toUInt(), h.toUInt())))
            }
        }
    }

    companion object {
        /** Fit-width: the least single-page mode zooms to. */
        const val MIN_ZOOM = 1f

        /** How far continuous mode zooms out (a quarter of fit-width). */
        const val ZOOM_OUT_LIMIT = 0.25f

        /** A pinch ending between this and fit-width springs back to fit-width. */
        const val SNAP_TO_FIT = 0.9f
        const val LOWRES_BELOW = 0.6f
        const val LOWRES_LEVEL = -1
        const val MAX_ZOOM = 10f

        /** What a zoom step adds or takes away (fit-width is 1), as the reference's step. */
        const val ZOOM_STEP = 0.25f
        const val TILE = 512
        const val LEVEL_STEP = 1.4142135f
        private const val SNAP_VELOCITY = 400f
    }
}
