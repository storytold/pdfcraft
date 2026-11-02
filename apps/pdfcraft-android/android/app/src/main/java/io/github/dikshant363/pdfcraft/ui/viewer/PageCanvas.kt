package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.AnimationVector2D
import androidx.compose.animation.core.VectorConverter
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.splineBasedDecay
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateCentroid
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.input.pointer.PointerEventType
import androidx.compose.ui.input.pointer.PointerInputChange
import androidx.compose.ui.input.pointer.PointerType
import androidx.compose.ui.input.pointer.isCtrlPressed
import androidx.compose.ui.input.pointer.isSecondaryPressed
import androidx.compose.ui.input.pointer.isTertiaryPressed
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import kotlin.math.abs
import kotlin.math.roundToInt
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

/** How the canvas draws pages: their surround and paper, and the page colour mode. */
data class CanvasLook(
    val background: Color,
    val pageShadow: Color,
    /** Draw pages inverted (night mode): white paper becomes black. */
    val invert: Boolean = false,
)

/** Screen pixels a mouse wheel's notch scrolls (the reference's step). */
private const val WHEEL_STEP = 100f

/** How soon after one the wheel may turn another single page. */
private const val WHEEL_TURN_MS = 800L

/**
 * The document view: pages laid out by [state], pinch to zoom, drag and fling to scroll, double
 * tap to zoom in or back out (and [onDoubleTap]), tap for [onTap] (bars), long press for [onLongPress]. [overlay]
 * draws over the pages in screen space (selection, search hits, comments being made). When
 * [gesturesEnabled] is false a tool above takes the touches itself. A mouse wheel scrolls (zooms
 * with Ctrl, then [onWheelZoom]), a mouse drag goes to [onMouseSelect] when there is one, and a
 * hovering mouse to [onMouseHover].
 */
@Composable
fun PageCanvas(
    state: ViewerState,
    images: PageImages,
    generation: Long,
    look: CanvasLook,
    modifier: Modifier = Modifier,
    gap: Dp = 8.dp,
    margin: Dp = 0.dp,
    gesturesEnabled: Boolean = true,
    onTap: (Offset, PagePoint?) -> Unit = { _, _ -> },
    onLongPress: (Offset, PagePoint?) -> Unit = { _, _ -> },
    onDoubleTap: () -> Unit = {},
    onGestureStart: () -> Unit = {},
    /** A drag with the mouse's main button: from and to, on the page it began on (text selection). */
    onMouseSelect: ((PagePoint, PagePoint) -> Unit)? = null,
    /** The mouse moved over the pages with no button down (screen position). */
    onMouseHover: (Offset) -> Unit = {},
    /** Ctrl and the wheel zoomed. */
    onWheelZoom: () -> Unit = {},
    overlay: DrawScope.() -> Unit = {},
) {
    val density = LocalDensity.current
    val scope = rememberCoroutineScope()
    state.gap = with(density) { gap.toPx() }
    state.margin = with(density) { margin.toPx() }
    // device pixels per point beyond which a whole page isn't worth more pixels (tiles take over)
    val maxScale = with(density) { 4.dp.toPx() }
    val flinger = remember { arrayOfNulls<Job>(1) }
    val mouseSelect by rememberUpdatedState(onMouseSelect)
    val mouseHover by rememberUpdatedState(onMouseHover)
    val wheelZoom by rememberUpdatedState(onWheelZoom)

    // keep the renderers working on what is in view
    LaunchedEffect(state, images, generation) {
        snapshotFlow { Triple(state.zoom, state.scrollX to state.scrollY, state.layout) }.collect {
            images.want(state.requests(generation, maxScale * 4f))
        }
    }

    Box(
        modifier
            .fillMaxSize()
            .onSizeChanged { s: IntSize -> state.setViewport(s.width.toFloat(), s.height.toFloat()) }
            .pointerInput(state, gesturesEnabled) {
                if (!gesturesEnabled) return@pointerInput
                detectTapGestures(
                    onDoubleTap = { p ->
                        onDoubleTap()
                        flinger[0]?.cancel()
                        val from = state.zoom
                        // zoomed out: back to fit-width; fit-width: in; zoomed in: back to fit-width
                        val to = if (from < 0.99f) 1f else if (from < 1.6f) 2.5f else 1f
                        flinger[0] = scope.launch {
                            val anim = Animatable(from)
                            anim.animateTo(to, tween(DOUBLE_TAP_MS)) { state.zoomTo(value, p) }
                            if (state.mode == PageMode.SinglePage && to <= 1f) state.goToPage(state.currentPage)
                        }
                    },
                    onTap = { p -> onTap(p, state.screenToPage(p)) },
                    onLongPress = { p -> onLongPress(p, state.screenToPage(p)) },
                    onPress = { flinger[0]?.cancel() },
                )
            }
            // a mouse wheel scrolls, a single page turning at its edge, and with Ctrl zooms about
            // the pointer, as the reference's viewer does
            .pointerInput(state) {
                var lastTurn = 0L
                awaitPointerEventScope {
                    while (true) {
                        val event = awaitPointerEvent()
                        val change = event.changes.firstOrNull() ?: continue
                        if (event.type == PointerEventType.Move && change.type == PointerType.Mouse && !change.pressed) mouseHover(change.position)
                        if (event.type != PointerEventType.Scroll) continue
                        val d = change.scrollDelta
                        if (d == Offset.Zero) continue
                        flinger[0]?.cancel()
                        when {
                            event.keyboardModifiers.isCtrlPressed -> {
                                state.zoomStep(if (d.y < 0f) 1 else -1, change.position)
                                wheelZoom()
                            }
                            state.mode == PageMode.SinglePage -> {
                                val rest = state.scrollBy(0f, -d.y * WHEEL_STEP)
                                if (rest.y != 0f && change.uptimeMillis - lastTurn > WHEEL_TURN_MS) {
                                    lastTurn = change.uptimeMillis
                                    state.goToPage(state.currentPage + if (d.y > 0f) 1 else -1)
                                }
                            }
                            else -> state.scrollBy(-d.x * WHEEL_STEP, -d.y * WHEEL_STEP)
                        }
                        change.consume()
                    }
                }
            }
            .pointerInput(state, gesturesEnabled) {
                if (!gesturesEnabled) return@pointerInput
                val touchSlop = viewConfiguration.touchSlop
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    flinger[0]?.cancel()
                    // a drag with the mouse's main button selects text instead of moving the page; the
                    // right and middle buttons still move it
                    val buttons = currentEvent.buttons
                    val select = mouseSelect
                        ?.takeIf { down.type == PointerType.Mouse && !buttons.isSecondaryPressed && !buttons.isTertiaryPressed }
                        ?.let { f -> state.screenToPage(down.position)?.let { from -> from to f } }
                    if (select != null) {
                        val (from, report) = select
                        while (true) {
                            val event = awaitPointerEvent()
                            val change = event.changes.firstOrNull { it.id == down.id } ?: break
                            if (!change.pressed) break
                            if ((change.position - down.position).getDistance() > touchSlop) {
                                state.screenToPageOf(from.page, change.position)?.let { report(from, it) }
                                change.consume()
                            }
                        }
                        return@awaitEachGesture
                    }
                    val tracker = VelocityTracker()
                    tracker.addPosition(down.uptimeMillis, down.position)
                    var moved = Offset.Zero
                    var zoomed = 1f
                    var started = false
                    var pinched = false
                    var centroid = down.position
                    var lastChange: PointerInputChange = down
                    while (true) {
                        val event = awaitPointerEvent()
                        val pressed = event.changes.filter { it.pressed }
                        if (pressed.isEmpty()) break
                        val zoomChange = event.calculateZoom()
                        val pan = event.calculatePan()
                        if (!started) {
                            moved += pan
                            zoomed *= zoomChange
                            if (moved.getDistance() > touchSlop || abs(1f - zoomed) * size.width / 2f > touchSlop) {
                                started = true
                                onGestureStart()
                            }
                        }
                        if (started) {
                            if (pressed.size > 1) {
                                pinched = true
                                val c = event.calculateCentroid()
                                centroid = c
                                if (zoomChange != 1f) state.zoomBy(zoomChange, c)
                            }
                            if (pan != Offset.Zero) state.scrollBy(pan.x, pan.y)
                            event.changes.forEach { if (it.positionChanged()) it.consume() }
                        }
                        lastChange = pressed.first()
                        tracker.addPosition(lastChange.uptimeMillis, lastChange.position)
                    }
                    if (!started) return@awaitEachGesture
                    val v = tracker.calculateVelocity()
                    val velocity = if (pinched) Offset.Zero else Offset(v.x, v.y)
                    val rest = if (pinched) state.pinchRest() else null
                    flinger[0] = scope.launch {
                        if (rest != null) {
                            val anim = Animatable(state.zoom)
                            anim.animateTo(rest, tween(DOUBLE_TAP_MS)) { state.zoomTo(value, centroid) }
                        } else {
                            settle(state, velocity, density.density)
                        }
                    }
                }
            },
    ) {
        Canvas(Modifier.fillMaxSize()) {
            // read the image version so arrivals redraw
            images.version
            drawRect(look.background)
            drawPages(state, images, generation, look, maxScale * 4f)
            overlay()
        }
    }
}

/** After a gesture: fling (with decay), or in single-page mode come to rest on a page. */
private suspend fun settle(state: ViewerState, velocity: Offset, density: Float) {
    if (state.mode == PageMode.SinglePage && state.zoom <= 1.01f) {
        val target = state.snapTarget(velocity.x)
        val anim = Animatable(state.scrollX)
        anim.animateTo(target, spring(stiffness = 600f), initialVelocity = -velocity.x) { state.setScroll(value, 0f) }
        return
    }
    if (velocity.getDistance() < 50f) return
    val anim: Animatable<Offset, AnimationVector2D> = Animatable(Offset.Zero, Offset.VectorConverter)
    var last = Offset.Zero
    anim.animateDecay(velocity, splineBasedDecay(androidx.compose.ui.unit.Density(density))) {
        val d = value - last
        last = value
        val left = state.scrollBy(d.x, d.y)
        // stop at the ends instead of decaying against them
        if (abs(left.x) > 0.5f && abs(left.y) > 0.5f) throw CancellationException("at the edge")
    }
}

private fun DrawScope.drawPages(state: ViewerState, images: PageImages, gen: Long, look: CanvasLook, maxScale: Float) {
    val level = state.detailLevel()
    for (i in state.visiblePages()) {
        val r = state.pageRectOnScreen(i)
        // a thin shadow line around the paper, as a printed page on a grey desk
        drawRect(look.pageShadow, topLeft = Offset(r.left - 1f, r.top - 1f), size = Size(r.width + 2f, r.height + 2f))
        drawRect(if (look.invert) Color.Black else Color.White, topLeft = r.topLeft, size = r.size)
        val base = images.get(ImageKey(i, gen, state.baseLevel())) ?: images.get(ImageKey(i, gen, 0)) ?: images.latestBase(i)
        if (base != null) drawImageIn(base, r, look.invert)
        if (level > 0) {
            val s = state.levelScale(i, level, maxScale)
            val k = state.pixelsPerPoint(i) / s
            // every tile of this page at this level that is ready
            val page = state.pages[i]
            val cols = (kotlin.math.ceil(page.width * s).toInt() + ViewerState.TILE - 1) / ViewerState.TILE
            val rows = (kotlin.math.ceil(page.height * s).toInt() + ViewerState.TILE - 1) / ViewerState.TILE
            val vx0 = ((0f - r.left) / k).coerceAtLeast(0f).toInt() / ViewerState.TILE
            val vy0 = ((0f - r.top) / k).coerceAtLeast(0f).toInt() / ViewerState.TILE
            val vx1 = ((size.width - r.left) / k).toInt() / ViewerState.TILE
            val vy1 = ((size.height - r.top) / k).toInt() / ViewerState.TILE
            for (ty in vy0..minOf(vy1, rows - 1)) for (tx in vx0..minOf(vx1, cols - 1)) {
                val t = images.get(ImageKey(i, gen, level, ty * cols + tx)) ?: continue
                val dst = Rect(r.left + t.x * k, r.top + t.y * k, r.left + (t.x + t.bitmap.width) * k, r.top + (t.y + t.bitmap.height) * k)
                drawImageIn(t, dst, look.invert)
            }
        }
    }
}

/** Night Mode: each pixel becomes inverted grey (255 − the mean of R, G and B), as the reference draws pages. */
private const val THIRD = -1f / 3f
private val invertFilter = androidx.compose.ui.graphics.ColorFilter.colorMatrix(
    androidx.compose.ui.graphics.ColorMatrix(
        floatArrayOf(
            THIRD, THIRD, THIRD, 0f, 255f,
            THIRD, THIRD, THIRD, 0f, 255f,
            THIRD, THIRD, THIRD, 0f, 255f,
            0f, 0f, 0f, 1f, 0f,
        ),
    ),
)

private fun DrawScope.drawImageIn(img: PageImage, dst: Rect, invert: Boolean) {
    val l = dst.left.roundToInt()
    val t = dst.top.roundToInt()
    drawImage(
        img.bitmap,
        srcOffset = IntOffset.Zero,
        srcSize = IntSize(img.bitmap.width, img.bitmap.height),
        dstOffset = IntOffset(l, t),
        dstSize = IntSize((dst.right.roundToInt() - l).coerceAtLeast(1), (dst.bottom.roundToInt() - t).coerceAtLeast(1)),
        filterQuality = FilterQuality.Medium,
        colorFilter = if (invert) invertFilter else null,
    )
}

private const val DOUBLE_TAP_MS = 250
