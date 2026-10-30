package com.storytold.pdfcraft.kit

import android.graphics.RectF
import org.junit.Assert.*
import org.junit.Test
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min

class PdfContentTransformTest {

    private fun rect(l: Float, t: Float, r: Float, b: Float): RectF {
        val res = RectF()
        res.left = l
        res.top = t
        res.right = r
        res.bottom = b
        return res
    }

    private fun hitTestResizeHandle(pageX: Float, pageY: Float, rect: RectF, touchRadius: Float): ResizeHandle {
        val r = touchRadius
        if (abs(pageX - rect.left) <= r && abs(pageY - rect.top) <= r) return ResizeHandle.TOP_LEFT
        if (abs(pageX - rect.right) <= r && abs(pageY - rect.top) <= r) return ResizeHandle.TOP_RIGHT
        if (abs(pageX - rect.left) <= r && abs(pageY - rect.bottom) <= r) return ResizeHandle.BOTTOM_LEFT
        if (abs(pageX - rect.right) <= r && abs(pageY - rect.bottom) <= r) return ResizeHandle.BOTTOM_RIGHT
        return ResizeHandle.NONE
    }

    @Test
    fun testHitTestResizeHandles() {
        val r = rect(100f, 100f, 300f, 300f)
        val radius = 24f

        // Exact corners
        assertEquals(ResizeHandle.TOP_LEFT, hitTestResizeHandle(100f, 100f, r, radius))
        assertEquals(ResizeHandle.TOP_RIGHT, hitTestResizeHandle(300f, 100f, r, radius))
        assertEquals(ResizeHandle.BOTTOM_LEFT, hitTestResizeHandle(100f, 300f, r, radius))
        assertEquals(ResizeHandle.BOTTOM_RIGHT, hitTestResizeHandle(300f, 300f, r, radius))

        // Slop / touch radius around corners
        assertEquals(ResizeHandle.TOP_LEFT, hitTestResizeHandle(115f, 90f, r, radius))
        assertEquals(ResizeHandle.BOTTOM_RIGHT, hitTestResizeHandle(310f, 310f, r, radius))

        // Center / interior of rect
        assertEquals(ResizeHandle.NONE, hitTestResizeHandle(200f, 200f, r, radius))

        // Outside rect completely
        assertEquals(ResizeHandle.NONE, hitTestResizeHandle(50f, 50f, r, radius))
        assertEquals(ResizeHandle.NONE, hitTestResizeHandle(400f, 400f, r, radius))
    }

    @Test
    fun testContentDraggingClamping() {
        val initial = rect(100f, 100f, 200f, 200f)
        val pageWidth = 612f
        val pageHeight = 792f
        val itemW = 100f
        val itemH = 100f

        // Move right and down
        val dx = 50f
        val dy = 75f
        val movedLeft = (initial.left + dx).coerceIn(0f, pageWidth - itemW)
        val movedTop = (initial.top + dy).coerceIn(0f, pageHeight - itemH)
        val live = rect(movedLeft, movedTop, movedLeft + itemW, movedTop + itemH)

        assertEquals(150f, live.left, 0.01f)
        assertEquals(175f, live.top, 0.01f)
        assertEquals(250f, live.right, 0.01f)
        assertEquals(275f, live.bottom, 0.01f)

        // Clamp to top-left boundary
        val clampDx = -200f
        val clampDy = -200f
        val clampLeft = (initial.left + clampDx).coerceIn(0f, pageWidth - itemW)
        val clampTop = (initial.top + clampDy).coerceIn(0f, pageHeight - itemH)
        val clampedLive = rect(clampLeft, clampTop, clampLeft + itemW, clampTop + itemH)

        assertEquals(0f, clampedLive.left, 0.01f)
        assertEquals(0f, clampedLive.top, 0.01f)
        assertEquals(100f, clampedLive.right, 0.01f)
        assertEquals(100f, clampedLive.bottom, 0.01f)
    }

    @Test
    fun testResizeHandleMinSizeEnforcement() {
        val initial = rect(100f, 100f, 200f, 200f)
        val minSize = 24f
        val pageWidth = 612f
        val pageHeight = 792f

        // Attempt to collapse bottom-right handle past minSize
        val dx = -90f // would make width 10 < 24
        val dy = -90f // would make height 10 < 24
        val newRight = max(initial.left + minSize, initial.right + dx).coerceAtMost(pageWidth)
        val newBottom = max(initial.top + minSize, initial.bottom + dy).coerceAtMost(pageHeight)
        val live = rect(initial.left, initial.top, newRight, newBottom)

        assertEquals(100f, live.left, 0.01f)
        assertEquals(100f, live.top, 0.01f)
        assertEquals(124f, live.right, 0.01f)
        assertEquals(124f, live.bottom, 0.01f)
    }
}
