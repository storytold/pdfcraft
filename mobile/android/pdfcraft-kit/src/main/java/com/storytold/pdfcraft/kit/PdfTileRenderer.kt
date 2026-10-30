package com.storytold.pdfcraft.kit

import android.graphics.RectF
import uniffi.pdfcraft_ffi.FfiPageGeometry
import uniffi.pdfcraft_ffi.FfiTile
import kotlin.math.ceil
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min

/**
 * Coordinate transformations between page points and device pixel tiles.
 */
object PdfTileRenderer {
    /**
     * Computes the tile rect in device pixels for a given dirty rect in page points.
     */
    fun computeTile(
        pageGeometry: FfiPageGeometry,
        rect: RectF,
        scale: Float
    ): FfiTile? {
        if (scale <= 0f) return null

        val totalPixelWidth = ceil(pageGeometry.width * scale).toDouble()
        val totalPixelHeight = ceil(pageGeometry.height * scale).toDouble()

        val rawX = floor(rect.left * scale).toDouble()
        val rawY = floor(rect.top * scale).toDouble()
        val rawW = ceil((rect.right - rect.left) * scale).toDouble()
        val rawH = ceil((rect.bottom - rect.top) * scale).toDouble()

        if (rawX >= totalPixelWidth || rawY >= totalPixelHeight) return null
        if (rawW <= 0.0 || rawH <= 0.0) return null

        val clampedX = max(0.0, rawX)
        val clampedY = max(0.0, rawY)
        val clampedW = min(rawW, totalPixelWidth - clampedX)
        val clampedH = min(rawH, totalPixelHeight - clampedY)

        if (clampedW <= 0.0 || clampedH <= 0.0) return null

        return FfiTile(
            x = clampedX.toUInt(),
            y = clampedY.toUInt(),
            width = clampedW.toUInt(),
            height = clampedH.toUInt()
        )
    }
}
