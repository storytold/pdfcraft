package com.storytold.pdfcraft.kit

import uniffi.pdfcraft_ffi.FfiPageGeometry
import kotlin.math.min

enum class PdfLayoutMode {
    SINGLE_PAGE,
    CONTINUOUS_SCROLL,
    TWO_PAGE_SPREAD
}

enum class PdfViewRotation(val degrees: Int) {
    ROTATION_0(0),
    ROTATION_90(90),
    ROTATION_180(180),
    ROTATION_270(270);

    fun rotatedClockwise(): PdfViewRotation = when (this) {
        ROTATION_0 -> ROTATION_90
        ROTATION_90 -> ROTATION_180
        ROTATION_180 -> ROTATION_270
        ROTATION_270 -> ROTATION_0
    }

    fun rotatedCounterClockwise(): PdfViewRotation = when (this) {
        ROTATION_0 -> ROTATION_270
        ROTATION_90 -> ROTATION_0
        ROTATION_180 -> ROTATION_90
        ROTATION_270 -> ROTATION_180
    }
}

data class PdfDimensions(val width: Float, val height: Float)

data class PdfViewportState(
    var layoutMode: PdfLayoutMode = PdfLayoutMode.SINGLE_PAGE,
    var rotation: PdfViewRotation = PdfViewRotation.ROTATION_0,
    var currentPage: Int = 0,
    var zoomScale: Float = 1.0f
) {
    companion object {
        const val MIN_ZOOM_SCALE: Float = 0.25f
        const val MAX_ZOOM_SCALE: Float = 16.0f
    }

    fun effectiveSize(geometry: FfiPageGeometry, scale: Float = 1.0f): PdfDimensions {
        val totalRotation = (geometry.rotation.toInt() + rotation.degrees) % 360
        val isTransposed = totalRotation == 90 || totalRotation == 270
        val w = if (isTransposed) geometry.height else geometry.width
        val h = if (isTransposed) geometry.width else geometry.height
        return PdfDimensions(w * scale, h * scale)
    }

    fun scaleToFitWidth(geometry: FfiPageGeometry, viewportWidth: Float): Float {
        if (viewportWidth <= 0f) return 1.0f
        val natural = effectiveSize(geometry, 1.0f)
        if (natural.width <= 0f) return 1.0f
        return (viewportWidth / natural.width).coerceIn(MIN_ZOOM_SCALE, MAX_ZOOM_SCALE)
    }

    fun scaleToFitPage(geometry: FfiPageGeometry, viewportWidth: Float, viewportHeight: Float): Float {
        if (viewportWidth <= 0f || viewportHeight <= 0f) return 1.0f
        val natural = effectiveSize(geometry, 1.0f)
        if (natural.width <= 0f || natural.height <= 0f) return 1.0f
        val sW = viewportWidth / natural.width
        val sH = viewportHeight / natural.height
        return min(sW, sH).coerceIn(MIN_ZOOM_SCALE, MAX_ZOOM_SCALE)
    }
}
