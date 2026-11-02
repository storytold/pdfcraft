package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.ui.geometry.Offset
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sqrt

/** A clean shape a freehand stroke was recognised as (page points, top-left origin). */
sealed class SnappedShape {
    data class Line(val from: Offset, val to: Offset) : SnappedShape()
    data class Rectangle(val left: Float, val top: Float, val right: Float, val bottom: Float) : SnappedShape()
    data class Oval(val left: Float, val top: Float, val right: Float, val bottom: Float) : SnappedShape()
    data class Polygon(val vertices: List<Offset>) : SnappedShape()

    /**
     * The outline as the ink stroke's points. Straight edges get a point every [EDGE_STEP] points:
     * ink is drawn as a smooth curve through its points, which would bow an edge with only its ends.
     */
    fun outline(): List<Offset> = when (this) {
        is Line -> densify(listOf(from, to))
        is Rectangle -> densify(listOf(Offset(left, top), Offset(right, top), Offset(right, bottom), Offset(left, bottom), Offset(left, top)))
        is Oval -> (0..48).map { i ->
            val a = i / 48.0 * 2 * Math.PI
            Offset(((left + right) / 2 + (right - left) / 2 * Math.cos(a)).toFloat(), ((top + bottom) / 2 + (bottom - top) / 2 * Math.sin(a)).toFloat())
        }
        is Polygon -> densify(vertices + vertices.first())
    }

    private fun densify(corners: List<Offset>): List<Offset> {
        val out = mutableListOf(corners.first())
        for (i in 1 until corners.size) {
            val a = corners[i - 1]
            val b = corners[i]
            val n = ((b - a).getDistance() / EDGE_STEP).toInt().coerceIn(1, 2000)
            for (k in 1..n) out.add(Offset(a.x + (b.x - a.x) * k / n, a.y + (b.y - a.y) * k / n))
        }
        return out
    }

    private companion object {
        const val EDGE_STEP = 1.5f
    }
}

/**
 * Recognises a freehand stroke as a straight line, a rectangle, an oval or a polygon (triangle to
 * hexagon), or returns null when it is none of them clearly enough and should stay freehand.
 */
object ShapeSnap {
    fun recognise(points: List<Offset>): SnappedShape? {
        if (points.size < 2) return null
        val minX = points.minOf { it.x }
        val maxX = points.maxOf { it.x }
        val minY = points.minOf { it.y }
        val maxY = points.maxOf { it.y }
        val diag = hypot(maxX - minX, maxY - minY)
        // too small to be meant as a shape (a dot, a tick)
        if (diag < 8f) return null
        val length = pathLength(points)
        val first = points.first()
        val last = points.last()
        val chord = (last - first).getDistance()

        // a line: the path hardly longer than the distance it covers, and never far from it
        if (chord > 0.85f * length) {
            val dev = points.maxOf { distanceToSegment(it, first, last) }
            return if (dev <= max(3f, 0.06f * chord)) SnappedShape.Line(first, last) else null
        }

        // the closed shapes: the stroke ends near where it began
        if ((last - first).getDistance() > 0.2f * length || length < 3f * diag * 0.5f) return null
        val pts = resample(points, 64)
        val w = maxX - minX
        val h = maxY - minY
        if (w < 4f || h < 4f) return null

        // the corners of the stroke, from a simplification at a tolerance relative to its size
        val simplified = simplify(pts + pts.first(), 0.08f * diag).dropLast(1)
        val corners = mergeClose(simplified, 0.12f * diag)

        val ovalErr = ovalError(pts, minX, minY, maxX, maxY)
        val rectErr = rectError(pts, minX, minY, maxX, maxY)
        when {
            corners.size == 4 && rectErr < 0.05f && rectErr < ovalErr -> return SnappedShape.Rectangle(minX, minY, maxX, maxY)
            ovalErr < 0.09f && (corners.size > 4 || corners.size < 3 || ovalErr < rectErr * 0.7f) -> return SnappedShape.Oval(minX, minY, maxX, maxY)
            corners.size in 3..6 && polygonError(pts, corners) < 0.045f * diag -> return SnappedShape.Polygon(corners)
        }
        return null
    }

    private fun pathLength(p: List<Offset>): Float {
        var l = 0f
        for (i in 1 until p.size) l += (p[i] - p[i - 1]).getDistance()
        return l
    }

    private fun distanceToSegment(p: Offset, a: Offset, b: Offset): Float {
        val ab = b - a
        val len2 = ab.x * ab.x + ab.y * ab.y
        if (len2 == 0f) return (p - a).getDistance()
        val t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).coerceIn(0f, 1f)
        return (p - Offset(a.x + t * ab.x, a.y + t * ab.y)).getDistance()
    }

    /** [n] points evenly spaced along the path. */
    private fun resample(p: List<Offset>, n: Int): List<Offset> {
        val total = pathLength(p)
        if (total == 0f) return listOf(p.first())
        val step = total / (n - 1)
        val out = mutableListOf(p.first())
        var acc = 0f
        var prev = p.first()
        var i = 1
        while (i < p.size && out.size < n) {
            val cur = p[i]
            val d = (cur - prev).getDistance()
            if (acc + d >= step && d > 0f) {
                val t = (step - acc) / d
                val q = Offset(prev.x + t * (cur.x - prev.x), prev.y + t * (cur.y - prev.y))
                out.add(q)
                prev = q
                acc = 0f
            } else {
                acc += d
                prev = cur
                i++
            }
        }
        while (out.size < n) out.add(p.last())
        return out
    }

    /** Douglas–Peucker simplification. */
    private fun simplify(p: List<Offset>, tol: Float): List<Offset> {
        if (p.size < 3) return p
        var maxD = 0f
        var at = 0
        for (i in 1 until p.size - 1) {
            val d = distanceToSegment(p[i], p.first(), p.last())
            if (d > maxD) { maxD = d; at = i }
        }
        // a closed loop's first and last points coincide: split at the point farthest from them
        if (p.first() == p.last() || (p.first() - p.last()).getDistance() < 1e-3f) {
            var far = 0f
            for (i in 1 until p.size - 1) {
                val d = (p[i] - p.first()).getDistance()
                if (d > far) { far = d; at = i }
            }
            maxD = Float.MAX_VALUE
        }
        if (maxD <= tol || at == 0) return listOf(p.first(), p.last())
        val a = simplify(p.subList(0, at + 1), tol)
        val b = simplify(p.subList(at, p.size), tol)
        return a.dropLast(1) + b
    }

    /** Corners closer than [d] to the previous one are the same corner. */
    private fun mergeClose(p: List<Offset>, d: Float): List<Offset> {
        val out = mutableListOf<Offset>()
        for (q in p) if (out.isEmpty() || (q - out.last()).getDistance() >= d) out.add(q)
        while (out.size > 2 && (out.first() - out.last()).getDistance() < d) out.removeAt(out.lastIndex)
        return out
    }

    /** Mean distance from the box's edges, relative to the box's size. */
    private fun rectError(p: List<Offset>, l: Float, t: Float, r: Float, b: Float): Float {
        val size = max(r - l, b - t)
        return p.map { minOf(abs(it.x - l), abs(it.x - r), abs(it.y - t), abs(it.y - b)) }.average().toFloat() / size
    }

    /** Mean difference of the normalised radius from 1 for the ellipse inscribed in the box. */
    private fun ovalError(p: List<Offset>, l: Float, t: Float, r: Float, b: Float): Float {
        val cx = (l + r) / 2
        val cy = (t + b) / 2
        val rx = (r - l) / 2
        val ry = (b - t) / 2
        return p.map { abs(sqrt(((it.x - cx) / rx).let { x -> x * x } + ((it.y - cy) / ry).let { y -> y * y }) - 1f) }.average().toFloat()
    }

    /** Mean distance of the stroke from the polygon's edges. */
    private fun polygonError(p: List<Offset>, v: List<Offset>): Float = p.map { q ->
        var d = Float.MAX_VALUE
        for (i in v.indices) d = min(d, distanceToSegment(q, v[i], v[(i + 1) % v.size]))
        d
    }.average().toFloat()
}
