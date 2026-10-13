package com.storytold.pdfcraft.kit

import android.content.Context
import android.graphics.*
import android.util.AttributeSet
import android.util.LruCache
import android.view.GestureDetector
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.View
import android.widget.OverScroller
import kotlinx.coroutines.*
import uniffi.pdfcraft_ffi.FfiPageGeometry
import kotlin.math.max
import kotlin.math.min

/**
 * High-performance Android View for displaying a PDF page with tiled rendering
 * and native gestures: pinch-to-zoom (25% to 1600%), pan, momentum fling, and double-tap zoom.
 */
class PdfPageView @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
    defStyleAttr: Int = 0
) : View(context, attrs, defStyleAttr) {

    var document: PdfDocument? = null
        private set
    var pageIndex: Int = 0
        private set
    var pageGeometry: FfiPageGeometry? = null
        private set

    // Zoom and pan state
    var scale: Float = 1.0f
        private set
    var minScale: Float = 0.25f
    var maxScale: Float = 16.0f
    private var offsetX: Float = 0f
    private var offsetY: Float = 0f

    // Gestures & Physics
    private val scroller = OverScroller(context)
    private val scaleDetector: ScaleGestureDetector
    private val gestureDetector: GestureDetector

    // View settings & Overlays
    var readingColors: uniffi.pdfcraft_ffi.FfiReadingColors? = null
        set(value) {
            field = value
            tileCache.evictAll()
            loadPreview()
            invalidate()
        }
    var hideComments: Boolean = false
        set(value) {
            field = value
            tileCache.evictAll()
            loadPreview()
            invalidate()
        }
    var searchHighlights: List<uniffi.pdfcraft_ffi.FfiRect> = emptyList()
        set(value) {
            field = value
            invalidate()
        }
    var selectedTextRects: List<uniffi.pdfcraft_ffi.FfiRect> = emptyList()
        set(value) {
            field = value
            invalidate()
        }
    var selectionGlyphRange: Pair<Int, Int>? = null

    fun selectText(startGlyph: Int, endGlyph: Int) {
        val doc = document ?: return
        selectionGlyphRange = Pair(startGlyph, endGlyph)
        selectedTextRects = doc.textSelectionRects(pageIndex, startGlyph, endGlyph)
    }

    fun clearTextSelection() {
        selectionGlyphRange = null
        selectedTextRects = emptyList()
    }

    fun markupSelection(
        kind: uniffi.pdfcraft_ffi.FfiTextMarkupKind,
        colorRgb: UInt? = null,
        author: String? = null,
        contents: String? = null
    ) {
        val doc = document ?: return
        val range = selectionGlyphRange
        if (range != null) {
            doc.addTextMarkup(
                page = pageIndex,
                startGlyph = range.first,
                endGlyph = range.second,
                kind = kind,
                colorRgb = colorRgb,
                author = author,
                contents = contents
            )
        } else if (selectedTextRects.isNotEmpty()) {
            doc.addTextMarkupRects(
                page = pageIndex,
                rects = selectedTextRects,
                kind = kind,
                colorRgb = colorRgb,
                author = author,
                contents = contents
            )
        }
        clearTextSelection()
        tileCache.evictAll()
        loadPreview()
        invalidate()
    }

    var selectedAnnotation: uniffi.pdfcraft_ffi.FfiAnnotationSummary? = null
        private set

    fun selectAnnotation(x: Float, y: Float): uniffi.pdfcraft_ffi.FfiAnnotationSummary? {
        val doc = document ?: return null
        val annot = doc.hitTestAnnotation(pageIndex, x, y)
        selectedAnnotation = annot
        invalidate()
        return annot
    }

    fun clearAnnotationSelection() {
        selectedAnnotation = null
        invalidate()
    }

    fun moveSelectedAnnotation(dx: Float, dy: Float) {
        val doc = document ?: return
        val annot = selectedAnnotation ?: return
        doc.moveAnnotation(pageIndex, annot.index.toInt(), dx, dy)
        selectedAnnotation = doc.hitTestAnnotation(pageIndex, annot.rect.x + dx + annot.rect.width / 2f, annot.rect.y + dy + annot.rect.height / 2f)
        tileCache.evictAll()
        loadPreview()
        invalidate()
    }

    fun deleteSelectedAnnotation() {
        val doc = document ?: return
        val annot = selectedAnnotation ?: return
        doc.deleteAnnotation(pageIndex, annot.index.toInt())
        clearAnnotationSelection()
        tileCache.evictAll()
        loadPreview()
        invalidate()
    }

    // Rendering & Caching
    private val paint = Paint(Paint.FILTER_BITMAP_FLAG)
    private val highlightPaint = Paint().apply {
        color = Color.argb(100, 255, 235, 59) // Translucent amber/yellow
        style = Paint.Style.FILL
    }
    private val selectionPaint = Paint().apply {
        color = Color.argb(90, 33, 150, 243) // Translucent primary blue
        style = Paint.Style.FILL
    }
    private val annotationSelectionPaint = Paint().apply {
        color = Color.rgb(33, 150, 243)
        style = Paint.Style.STROKE
        strokeWidth = 3f
        pathEffect = DashPathEffect(floatArrayOf(12f, 6f), 0f)
    }
    private val handlePaint = Paint().apply {
        color = Color.WHITE
        style = Paint.Style.FILL
    }
    private val handleStrokePaint = Paint().apply {
        color = Color.rgb(33, 150, 243)
        style = Paint.Style.STROKE
        strokeWidth = 2f
    }
    var isInkMode: Boolean = false
    var isEraserMode: Boolean = false
    var inkColorRgb: UInt = 0x0000FFu
    var inkStrokeWidth: Float = 2.5f
    private val liveStrokePoints = mutableListOf<PointF>()
    private val liveInkPath = Path()
    private val liveInkPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.BLUE
        style = Paint.Style.STROKE
        strokeWidth = 5f
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
    }
    private val backgroundPaint = Paint().apply {
        color = Color.WHITE
        style = Paint.Style.FILL
    }
    private val borderPaint = Paint().apply {
        color = Color.LTGRAY
        style = Paint.Style.STROKE
        strokeWidth = 2f
    }
    private var previewBitmap: Bitmap? = null
    private val tileSize = 512

    // Memory LRU cache for tiles (32 MB budget)
    private val tileCache = object : LruCache<String, Bitmap>(32 * 1024 * 1024) {
        override fun sizeOf(key: String, value: Bitmap): Int = value.byteCount
    }

    private val scope = CoroutineScope(Dispatchers.Main + SupervisorJob())
    private val inFlightTiles = mutableSetOf<String>()

    init {
        scaleDetector = ScaleGestureDetector(context, object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
            override fun onScaleBegin(detector: ScaleGestureDetector): Boolean {
                document?.advanceGenerationAndCancelStale()
                return super.onScaleBegin(detector)
            }

            override fun onScale(detector: ScaleGestureDetector): Boolean {
                val factor = detector.scaleFactor
                val newScale = (scale * factor).coerceIn(minScale, maxScale)
                if (newScale != scale) {
                    val focusX = detector.focusX
                    val focusY = detector.focusY
                    offsetX = focusX - (focusX - offsetX) * (newScale / scale)
                    offsetY = focusY - (focusY - offsetY) * (newScale / scale)
                    scale = newScale
                    clampOffsets()
                    invalidate()
                }
                return true
            }
        })

        gestureDetector = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
            override fun onDown(e: MotionEvent): Boolean {
                scroller.forceFinished(true)
                return true
            }

            override fun onScroll(e1: MotionEvent?, e2: MotionEvent, distanceX: Float, distanceY: Float): Boolean {
                offsetX -= distanceX
                offsetY -= distanceY
                clampOffsets()
                invalidate()
                return true
            }

            override fun onFling(e1: MotionEvent?, e2: MotionEvent, velocityX: Float, velocityY: Float): Boolean {
                document?.advanceGenerationAndCancelStale()
                val geom = pageGeometry ?: return false
                val contentW = (geom.width * scale).toInt()
                val contentH = (geom.height * scale).toInt()
                val minX = min(0, width - contentW)
                val minY = min(0, height - contentH)

                scroller.fling(
                    offsetX.toInt(), offsetY.toInt(),
                    velocityX.toInt(), velocityY.toInt(),
                    minX, 0, minY, 0
                )
                postInvalidateOnAnimation()
                return true
            }

            override fun onDoubleTap(e: MotionEvent): Boolean {
                if (scale > minScale * 1.5f) {
                    zoomToFitWidth()
                } else {
                    val targetScale = min(2.5f, maxScale)
                    val focusX = e.x
                    val focusY = e.y
                    offsetX = focusX - (focusX - offsetX) * (targetScale / scale)
                    offsetY = focusY - (focusY - offsetY) * (targetScale / scale)
                    scale = targetScale
                    clampOffsets()
                    invalidate()
                }
                return true
            }
        })
    }

    fun setDocument(doc: PdfDocument, page: Int = 0) {
        this.document = doc
        this.pageIndex = page
        this.pageGeometry = try { doc.geometry(page) } catch (_: Exception) { null }
        this.previewBitmap = null
        tileCache.evictAll()
        inFlightTiles.clear()

        loadPreview()
        requestLayout()
        invalidate()
    }

    private fun loadPreview() {
        val doc = document ?: return
        val page = pageIndex
        val rc = readingColors
        val hc = hideComments
        scope.launch(Dispatchers.IO) {
            val bmp = try { doc.renderPage(page, 0.5f, preview = true, readingColors = rc, hideComments = hc) } catch (_: Exception) { null }
            withContext(Dispatchers.Main) {
                if (pageIndex == page && bmp != null) {
                    previewBitmap = bmp
                    invalidate()
                }
            }
        }
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (isInkMode || isEraserMode) {
            val vx = (event.x - offsetX) / scale
            val vy = (event.y - offsetY) / scale
            when (event.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    liveStrokePoints.clear()
                    liveStrokePoints.add(PointF(vx, vy))
                    if (isEraserMode) {
                        document?.eraseInk(pageIndex, listOf(uniffi.pdfcraft_ffi.FfiPoint(vx, vy)), 20f)
                        tileCache.evictAll()
                        loadPreview()
                    }
                    invalidate()
                    return true
                }
                MotionEvent.ACTION_MOVE -> {
                    for (h in 0 until event.historySize) {
                        val hx = (event.getHistoricalX(h) - offsetX) / scale
                        val hy = (event.getHistoricalY(h) - offsetY) / scale
                        liveStrokePoints.add(PointF(hx, hy))
                    }
                    liveStrokePoints.add(PointF(vx, vy))
                    if (isEraserMode) {
                        val pts = liveStrokePoints.map { uniffi.pdfcraft_ffi.FfiPoint(it.x, it.y) }
                        document?.eraseInk(pageIndex, pts, 20f)
                        tileCache.evictAll()
                        loadPreview()
                    }
                    invalidate()
                    return true
                }
                MotionEvent.ACTION_UP -> {
                    if (isInkMode && liveStrokePoints.size >= 2) {
                        val pts = liveStrokePoints.map { uniffi.pdfcraft_ffi.FfiPoint(it.x, it.y) }
                        document?.addInkStroke(pageIndex, pts, inkColorRgb, inkStrokeWidth)
                        tileCache.evictAll()
                        loadPreview()
                    }
                    liveStrokePoints.clear()
                    invalidate()
                    return true
                }
                MotionEvent.ACTION_CANCEL -> {
                    liveStrokePoints.clear()
                    invalidate()
                    return true
                }
            }
        }
        var ret = scaleDetector.onTouchEvent(event)
        ret = gestureDetector.onTouchEvent(event) || ret
        return ret || super.onTouchEvent(event)
    }

    override fun computeScroll() {
        if (scroller.computeScrollOffset()) {
            offsetX = scroller.currX.toFloat()
            offsetY = scroller.currY.toFloat()
            clampOffsets()
            postInvalidateOnAnimation()
        }
    }

    private fun clampOffsets() {
        val geom = pageGeometry ?: return
        val contentW = geom.width * scale
        val contentH = geom.height * scale

        if (contentW <= width) {
            offsetX = (width - contentW) / 2f
        } else {
            offsetX = offsetX.coerceIn(width - contentW, 0f)
        }

        if (contentH <= height) {
            offsetY = (height - contentH) / 2f
        } else {
            offsetY = offsetY.coerceIn(height - contentH, 0f)
        }
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        clampOffsets()
    }

    fun zoomToFitWidth() {
        val geom = pageGeometry ?: return
        if (width > 0 && geom.width > 0) {
            scale = (width / geom.width).coerceIn(minScale, maxScale)
            clampOffsets()
            invalidate()
        }
    }

    fun zoomToFitPage() {
        val geom = pageGeometry ?: return
        if (width > 0 && height > 0 && geom.width > 0 && geom.height > 0) {
            val sW = width / geom.width
            val sH = height / geom.height
            scale = min(sW, sH).coerceIn(minScale, maxScale)
            clampOffsets()
            invalidate()
        }
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val geom = pageGeometry ?: return
        val doc = document ?: return

        canvas.save()
        canvas.translate(offsetX, offsetY)

        val pageW = geom.width * scale
        val pageH = geom.height * scale

        // Draw page white background & border
        canvas.drawRect(0f, 0f, pageW, pageH, backgroundPaint)
        canvas.drawRect(0f, 0f, pageW, pageH, borderPaint)

        // Draw preview bitmap placeholder if available
        val preview = previewBitmap
        if (preview != null && !preview.isRecycled) {
            val src = Rect(0, 0, preview.width, preview.height)
            val dst = RectF(0f, 0f, pageW, pageH)
            canvas.drawBitmap(preview, src, dst, paint)
        }

        // Tiled rendering of visible tiles
        val visibleLeft = max(0f, -offsetX)
        val visibleTop = max(0f, -offsetY)
        val visibleRight = min(pageW, width.toFloat() - offsetX)
        val visibleBottom = min(pageH, height.toFloat() - offsetY)

        val tileCols = (pageW / tileSize).toInt() + 1
        val tileRows = (pageH / tileSize).toInt() + 1

        val startCol = (visibleLeft / tileSize).toInt().coerceAtLeast(0)
        val endCol = (visibleRight / tileSize).toInt().coerceAtMost(tileCols - 1)
        val startRow = (visibleTop / tileSize).toInt().coerceAtLeast(0)
        val endRow = (visibleBottom / tileSize).toInt().coerceAtMost(tileRows - 1)

        val currentScale = scale
        val currentPage = pageIndex

        for (row in startRow..endRow) {
            for (col in startCol..endCol) {
                val tileX = col * tileSize
                val tileY = row * tileSize
                val tileW = min(tileSize.toFloat(), pageW - tileX)
                val tileH = min(tileSize.toFloat(), pageH - tileY)

                val tileKey = "$currentPage@${currentScale}_${col}_$row"
                val cachedTile = tileCache.get(tileKey)

                val dstRect = RectF(tileX.toFloat(), tileY.toFloat(), tileX + tileW, tileY + tileH)

                if (cachedTile != null && !cachedTile.isRecycled) {
                    canvas.drawBitmap(cachedTile, null, dstRect, paint)
                } else if (!inFlightTiles.contains(tileKey)) {
                    inFlightTiles.add(tileKey)
                    val rc = readingColors
                    val hc = hideComments
                    // Request tile render in background
                    scope.launch(Dispatchers.IO) {
                        val rectInPoints = RectF(
                            tileX / currentScale,
                            tileY / currentScale,
                            (tileX + tileW) / currentScale,
                            (tileY + tileH) / currentScale
                        )
                        val ffiTile = PdfTileRenderer.computeTile(geom, rectInPoints, currentScale)
                        val bmp = try {
                            doc.renderTile(currentPage, ffiTile, currentScale, readingColors = rc, hideComments = hc)
                        } catch (_: Exception) {
                            null
                        }

                        withContext(Dispatchers.Main) {
                            inFlightTiles.remove(tileKey)
                            if (bmp != null) {
                                tileCache.put(tileKey, bmp)
                                if (scale == currentScale && pageIndex == currentPage) {
                                    invalidate()
                                }
                            }
                        }
                    }
                }
            }
        }

        // Search highlights overlay
        if (searchHighlights.isNotEmpty()) {
            for (hl in searchHighlights) {
                val dst = RectF(
                    hl.x * currentScale,
                    hl.y * currentScale,
                    (hl.x + hl.width) * currentScale,
                    (hl.y + hl.height) * currentScale
                )
                canvas.drawRect(dst, highlightPaint)
            }
        }

        // Text selection overlay
        if (selectedTextRects.isNotEmpty()) {
            for (sel in selectedTextRects) {
                val dst = RectF(
                    sel.x * currentScale,
                    sel.y * currentScale,
                    (sel.x + sel.width) * currentScale,
                    (sel.y + sel.height) * currentScale
                )
                canvas.drawRect(dst, selectionPaint)
            }
        }

        // Selected annotation overlay (dashed border & handles)
        val annot = selectedAnnotation
        if (annot != null) {
            val r = RectF(
                annot.rect.x * currentScale,
                annot.rect.y * currentScale,
                (annot.rect.x + annot.rect.width) * currentScale,
                (annot.rect.y + annot.rect.height) * currentScale
            )
            canvas.drawRect(r, annotationSelectionPaint)
            val handleSize = 16f
            val half = handleSize / 2f
            val corners = arrayOf(
                PointF(r.left, r.top),
                PointF(r.right, r.top),
                PointF(r.left, r.bottom),
                PointF(r.right, r.bottom)
            )
            for (c in corners) {
                val hr = RectF(c.x - half, c.y - half, c.x + half, c.y + half)
                canvas.drawRect(hr, handlePaint)
                canvas.drawRect(hr, handleStrokePaint)
            }
        }

        // Live ink stroke overlay
        if (liveStrokePoints.size >= 2) {
            val r = ((inkColorRgb shr 16) and 0xFFu).toInt()
            val g = ((inkColorRgb shr 8) and 0xFFu).toInt()
            val b = (inkColorRgb and 0xFFu).toInt()
            liveInkPaint.color = Color.rgb(r, g, b)
            liveInkPaint.strokeWidth = inkStrokeWidth * currentScale
            liveInkPath.reset()
            liveInkPath.moveTo(liveStrokePoints[0].x * currentScale, liveStrokePoints[0].y * currentScale)
            for (i in 1 until liveStrokePoints.size) {
                liveInkPath.lineTo(liveStrokePoints[i].x * currentScale, liveStrokePoints[i].y * currentScale)
            }
            canvas.drawPath(liveInkPath, liveInkPaint)
        }

        canvas.restore()
    }

    override fun onDetachedFromWindow() {
        super.onDetachedFromWindow()
        scope.cancel()
        tileCache.evictAll()
    }

    /**
     * Responds to OS memory pressure by stepwise trimming tile caches and cancelling speculative prefetch.
     */
    fun onTrimMemory(level: Int) {
        if (level >= android.content.ComponentCallbacks2.TRIM_MEMORY_RUNNING_CRITICAL) {
            tileCache.evictAll()
            document?.handleMemoryPressure(uniffi.pdfcraft_ffi.FfiMemoryPressureLevel.CRITICAL)
        } else if (level >= android.content.ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW) {
            tileCache.trimToSize(tileCache.size() / 2)
            document?.handleMemoryPressure(uniffi.pdfcraft_ffi.FfiMemoryPressureLevel.MODERATE)
        }
    }
}
