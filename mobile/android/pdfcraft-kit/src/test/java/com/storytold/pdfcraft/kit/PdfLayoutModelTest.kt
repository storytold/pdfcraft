package com.storytold.pdfcraft.kit

import org.junit.Assert.*
import org.junit.Test
import uniffi.pdfcraft_ffi.FfiPageGeometry

class PdfLayoutModelTest {

    @Test
    fun testViewportStateFitCalculations() {
        val geom = FfiPageGeometry(width = 612f, height = 792f, rotation = 0u, label = "1")
        val state = PdfViewportState(
            layoutMode = PdfLayoutMode.SINGLE_PAGE,
            rotation = PdfViewRotation.ROTATION_0,
            currentPage = 0,
            zoomScale = 1.0f
        )

        // Fit width at 1224px viewport
        val scaleW = state.scaleToFitWidth(geom, 1224f)
        assertEquals(2.0f, scaleW, 0.001f)

        // Fit page at 612x396 viewport
        val scalePage = state.scaleToFitPage(geom, 612f, 396f)
        assertEquals(0.5f, scalePage, 0.001f)

        // Rotation 90 degrees transposes dimensions
        state.rotation = PdfViewRotation.ROTATION_90
        val rotated = state.effectiveSize(geom, 1.0f)
        assertEquals(792f, rotated.width, 0.001f)
        assertEquals(612f, rotated.height, 0.001f)

        // Clamping checks
        val clampedMin = state.scaleToFitWidth(geom, 10f)
        assertEquals(PdfViewportState.MIN_ZOOM_SCALE, clampedMin, 0.001f)

        val clampedMax = state.scaleToFitWidth(geom, 50000f)
        assertEquals(PdfViewportState.MAX_ZOOM_SCALE, clampedMax, 0.001f)
    }

    @Test
    fun testViewRotationCycles() {
        var rot = PdfViewRotation.ROTATION_0
        rot = rot.rotatedClockwise()
        assertEquals(PdfViewRotation.ROTATION_90, rot)
        rot = rot.rotatedClockwise()
        assertEquals(PdfViewRotation.ROTATION_180, rot)
        rot = rot.rotatedClockwise()
        assertEquals(PdfViewRotation.ROTATION_270, rot)
        rot = rot.rotatedClockwise()
        assertEquals(PdfViewRotation.ROTATION_0, rot)

        rot = rot.rotatedCounterClockwise()
        assertEquals(PdfViewRotation.ROTATION_270, rot)
    }
}
