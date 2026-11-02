package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.core.CommentInfo
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import kotlin.math.roundToInt

/** Comments whose rectangle can be resized (the engine re-draws these to a new rectangle). */
private val Resizable = setOf("Square", "Circle", "FreeText", "Stamp")

/**
 * Move and resize a selected comment, as the reference lets: drag inside its frame to move it, or
 * (for rectangles, ovals, text boxes and stamps) a corner handle to resize it. The frame follows
 * the finger; the comment is changed when the finger lifts, as one undo step. Marked text has no
 * frame and isn't handled here.
 */
@Composable
fun CommentDragOverlay(
    doc: OpenDocument,
    c: CommentInfo,
    onMove: (dx: Float, dy: Float) -> Unit,
    onResize: (rect: List<Float>) -> Unit,
) {
    if (c.rect.size < 4 || c.locked) return
    if (c.subtype in setOf("Highlight", "Underline", "StrikeOut", "Squiggly")) return
    val page = c.page.toInt()
    val density = LocalDensity.current
    val slop = with(density) { 14.dp.toPx() }
    val a = doc.viewer.pageToScreen(page, c.rect[0], c.rect[1])
    val b = doc.viewer.pageToScreen(page, c.rect[2], c.rect[3])
    val frame = Rect(a, b)
    // a note's icon is drawn at a fixed size around its point: give it a finger-sized area
    val area = if (c.subtype == "Text" || c.subtype == "Sound") Rect(frame.center, with(density) { 22.dp.toPx() }) else frame.inflate(slop)
    var shift by remember(c.page, c.index, c.rect) { mutableStateOf(Offset.Zero) }
    var resized by remember(c.page, c.index, c.rect) { mutableStateOf<Rect?>(null) }
    val ppp = doc.viewer.pixelsPerPoint(page).coerceAtLeast(0.01f)
    val moveLabel = stringResource(R.string.comments_move)
    val resizeLabel = stringResource(R.string.comments_resize)

    // the frame where the comment will go
    Canvas(Modifier.fillMaxSize()) {
        val r = resized ?: frame.translate(shift)
        if (shift != Offset.Zero || resized != null) {
            drawRect(Color(0xFF448AFF), r.topLeft, r.size, style = Stroke(2.dp.toPx(), pathEffect = androidx.compose.ui.graphics.PathEffect.dashPathEffect(floatArrayOf(8.dp.toPx(), 5.dp.toPx()))))
        }
    }
    // the body: drag to move
    Box(
        Modifier
            .offset { IntOffset(area.left.roundToInt(), area.top.roundToInt()) }
            .size(with(density) { area.width.toDp() }, with(density) { area.height.toDp() })
            .pointerInput(c.page, c.index, c.rect) { detectTapGestures { } }
            .pointerInput(c.page, c.index, c.rect) {
                detectDragGestures(
                    onDrag = { change, d -> change.consume(); shift += d },
                    onDragEnd = {
                        val s = shift
                        if (s.getDistance() > 2f) onMove(s.x / ppp, s.y / ppp) else shift = Offset.Zero
                    },
                    onDragCancel = { shift = Offset.Zero },
                )
            }
            .semantics { contentDescription = moveLabel },
    )
    if (c.subtype !in Resizable) return
    // corner handles: drag to resize, the opposite corner staying put
    val handle = with(density) { 24.dp.toPx() }
    val corners = listOf(frame.topLeft, frame.topRight, frame.bottomLeft, frame.bottomRight)
    corners.forEachIndexed { i, corner ->
        Box(
            Modifier
                .offset { IntOffset((corner.x - handle / 2).roundToInt(), (corner.y - handle / 2).roundToInt()) }
                .size(24.dp)
                .pointerInput(c.page, c.index, c.rect, i) {
                    var current = frame
                    detectDragGestures(
                        onDragStart = { current = frame },
                        onDrag = { change, d ->
                            change.consume()
                            val minSide = 12.dp.toPx()
                            current = when (i) {
                                0 -> Rect(minOf(current.left + d.x, current.right - minSide), minOf(current.top + d.y, current.bottom - minSide), current.right, current.bottom)
                                1 -> Rect(current.left, minOf(current.top + d.y, current.bottom - minSide), maxOf(current.right + d.x, current.left + minSide), current.bottom)
                                2 -> Rect(minOf(current.left + d.x, current.right - minSide), current.top, current.right, maxOf(current.bottom + d.y, current.top + minSide))
                                else -> Rect(current.left, current.top, maxOf(current.right + d.x, current.left + minSide), maxOf(current.bottom + d.y, current.top + minSide))
                            }
                            resized = current
                        },
                        onDragEnd = {
                            val r = resized ?: return@detectDragGestures
                            val p0 = doc.viewer.screenToPage(r.topLeft, nearest = true)
                            val p1 = doc.viewer.screenToPage(r.bottomRight, nearest = true)
                            if (p0 != null && p1 != null && p0.page == page && p1.page == page) onResize(listOf(p0.x, p0.y, p1.x, p1.y)) else resized = null
                        },
                        onDragCancel = { resized = null },
                    )
                }
                .semantics { contentDescription = resizeLabel },
        ) {
            Canvas(Modifier.fillMaxSize()) {
                drawCircle(Color.White, size.minDimension / 2.6f)
                drawCircle(Color(0xFF448AFF), size.minDimension / 2.6f, style = Stroke(2.dp.toPx()))
            }
        }
    }
}
