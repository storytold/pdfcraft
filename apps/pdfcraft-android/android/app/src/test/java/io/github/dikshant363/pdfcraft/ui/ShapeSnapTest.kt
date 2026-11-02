package io.github.dikshant363.pdfcraft.ui

import androidx.compose.ui.geometry.Offset
import io.github.dikshant363.pdfcraft.ui.viewer.ShapeSnap
import io.github.dikshant363.pdfcraft.ui.viewer.SnappedShape
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.cos
import kotlin.math.sin

class ShapeSnapTest {
    /** A deterministic wobble, like a hand's, of up to [amp] points. */
    private fun wobble(i: Int, amp: Float) = (sin(i * 1.7) * amp).toFloat()

    /** Points along the segments joining [corners], [per] points per segment, with a wobble. */
    private fun traced(corners: List<Offset>, per: Int = 20, amp: Float = 1.5f): List<Offset> {
        val out = mutableListOf<Offset>()
        var k = 0
        for (s in 0 until corners.size - 1) {
            val a = corners[s]
            val b = corners[s + 1]
            for (i in 0 until per) {
                val t = i / per.toFloat()
                out.add(Offset(a.x + (b.x - a.x) * t + wobble(k, amp), a.y + (b.y - a.y) * t + wobble(k + 7, amp)))
                k++
            }
        }
        out.add(corners.last())
        return out
    }

    @Test
    fun aStraightishStrokeIsALine() {
        val s = ShapeSnap.recognise(traced(listOf(Offset(100f, 100f), Offset(300f, 160f))))
        assertTrue(s is SnappedShape.Line)
        s as SnappedShape.Line
        assertEquals(100f, s.from.x, 0.01f)
        assertEquals(300f, s.to.x, 0.01f)
    }

    @Test
    fun aClosedBoxIsARectangle() {
        val box = listOf(Offset(100f, 100f), Offset(300f, 100f), Offset(300f, 220f), Offset(100f, 220f), Offset(102f, 104f))
        val s = ShapeSnap.recognise(traced(box))
        assertTrue("$s", s is SnappedShape.Rectangle)
    }

    @Test
    fun aRoundLoopIsAnOval() {
        val pts = (0..80).map { i ->
            val a = i / 80.0 * 2 * Math.PI
            Offset((200 + 120 * cos(a) + wobble(i, 2f)).toFloat(), (200 + 70 * sin(a) + wobble(i + 3, 2f)).toFloat())
        }
        val s = ShapeSnap.recognise(pts)
        assertTrue("$s", s is SnappedShape.Oval)
    }

    @Test
    fun aTriangleIsAPolygonWithThreeCorners() {
        val tri = listOf(Offset(200f, 100f), Offset(320f, 300f), Offset(80f, 300f), Offset(198f, 104f))
        val s = ShapeSnap.recognise(traced(tri))
        assertTrue("$s", s is SnappedShape.Polygon)
        assertEquals(3, (s as SnappedShape.Polygon).vertices.size)
    }

    @Test
    fun straightEdgesKeepPointsCloseTogether() {
        // ink is drawn as a smooth curve through its points: sparse corners would bow the edges
        val pts = SnappedShape.Rectangle(100f, 100f, 300f, 220f).outline()
        assertTrue(pts.zipWithNext().all { (a, b) -> (b - a).getDistance() <= 1.51f })
        assertEquals(Offset(100f, 100f), pts.first())
        assertEquals(Offset(100f, 100f), pts.last())
    }

    @Test
    fun aScribbleStaysFreehand() {
        val pts = (0..120).map { i -> Offset(100f + i * 2f, 200f + (sin(i * 0.5) * 40).toFloat()) }
        assertNull(ShapeSnap.recognise(pts))
        assertNull(ShapeSnap.recognise(listOf(Offset(1f, 1f), Offset(2f, 2f))))
    }
}
