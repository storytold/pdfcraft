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
import uniffi.pdfcraft_ffi.FfiRect
import kotlin.math.ceil
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min
import kotlin.math.round

enum class ResizeHandle {
    NONE,
    TOP_LEFT,
    TOP_RIGHT,
    BOTTOM_LEFT,
    BOTTOM_RIGHT
}

/**
 * High-performance, high-DPI Android View for rendering PDF pages with crisp vector fidelity,
 * native gestures (pinch-zoom, momentum pan, double-tap zoom), interactive AcroForm editing,
 * link navigation, and text selection.
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
    var zoomLevel: Float = 1.0f
        private set
    val scale: Float
        get() = fitWidthScale * zoomLevel

    var minScale: Float = 0.5f
    var maxScale: Float = 16.0f
    private var offsetX: Float = 0f
    private var offsetY: Float = 0f

    val fitWidthScale: Float
        get() {
            val geom = pageGeometry ?: return 1.5f
            return if (width > 0 && geom.width > 0f) {
                width.toFloat() / geom.width
            } else {
                context.resources.displayMetrics.density * 1.5f
            }
        }

    // Callbacks for shell integration
    var onNavigateToPage: ((Int) -> Unit)? = null
    var onOpenUri: ((String) -> Unit)? = null
    var onEditTextField: ((uniffi.pdfcraft_ffi.FfiFormField) -> Unit)? = null
    var onSelectChoiceField: ((uniffi.pdfcraft_ffi.FfiFormField) -> Unit)? = null
    var onToggleChrome: (() -> Unit)? = null
    var onTextSelected: ((String) -> Unit)? = null
    var onContentItemSelected: ((PdfAddedItem?) -> Unit)? = null
    var onContentItemUpdated: ((PdfAddedItem) -> Unit)? = null
    var onContentItemDeleted: ((PdfAddedItem) -> Unit)? = null
    var onRedactionMarked: (() -> Unit)? = null

    // Touch Redaction Mode
    var isRedactMode: Boolean = false
        set(value) {
            field = value
            if (!value) {
                isDraggingRedact = false
                liveRedactRect.setEmpty()
            }
            invalidate()
        }
    private var isDraggingRedact = false
    private val liveRedactRect = RectF()
    private var redactStartTouchX = 0f
    private var redactStartTouchY = 0f

    // Content Addition & Manipulation State
    var isEditContentMode: Boolean = false
        set(value) {
            field = value
            if (!value) {
                selectedContentItem = null
            }
            invalidate()
        }

    var addedContent: List<PdfAddedItem> = emptyList()
        private set
    var selectedContentItem: PdfAddedItem? = null
        private set
    val liveItemRect = RectF()
    private val dragInitialRect = RectF()
    private var isDraggingContent = false
    private var activeResizeHandle = ResizeHandle.NONE
    private var dragStartTouchX = 0f
    private var dragStartTouchY = 0f

    // Gestures & Physics
    private val scroller = OverScroller(context)
    private val scaleDetector: ScaleGestureDetector
    private val gestureDetector: GestureDetector

    // View settings & Overlays
    var readingColors: uniffi.pdfcraft_ffi.FfiReadingColors? = null
        set(value) {
            field = value
            tileCache.evictAll()
            loadBaseBitmap()
            invalidate()
        }
    var hideComments: Boolean = false
        set(value) {
            field = value
            tileCache.evictAll()
            loadBaseBitmap()
            invalidate()
        }
    var searchHighlights: List<FfiRect> = emptyList()
        set(value) {
            field = value
            invalidate()
        }
    var selectedTextRects: List<FfiRect> = emptyList()
        set(value) {
            field = value
            invalidate()
        }
    var selectionGlyphRange: Pair<Int, Int>? = null

    fun selectText(startGlyph: Int, endGlyph: Int) {
        val doc = document ?: return
        selectionGlyphRange = Pair(startGlyph, endGlyph)
        selectedTextRects = doc.textSelectionRects(pageIndex, startGlyph, endGlyph)
        val text = doc.textSelectionString(pageIndex, startGlyph, endGlyph)
        if (text.isNotBlank()) {
            onTextSelected?.invoke(text)
        }
        invalidate()
    }

    fun clearTextSelection() {
        selectionGlyphRange = null
        selectedTextRects = emptyList()
        invalidate()
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
        loadBaseBitmap()
        invalidate()
    }

    fun redactSelection(overlayText: String = "REDACTED") {
        val doc = document ?: return
        val rects = selectedTextRects
        if (rects.isEmpty()) return
        scope.launch(Dispatchers.IO) {
            try {
                for (r in rects) {
                    doc.markRedaction(pageIndex, r, overlayText)
                }
                withContext(Dispatchers.Main) {
                    clearTextSelection()
                    refreshPage()
                    onRedactionMarked?.invoke()
                }
            } catch (_: Exception) {}
        }
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
        loadBaseBitmap()
        invalidate()
    }

    fun deleteSelectedAnnotation() {
        val doc = document ?: return
        val annot = selectedAnnotation ?: return
        doc.deleteAnnotation(pageIndex, annot.index.toInt())
        clearAnnotationSelection()
        tileCache.evictAll()
        loadBaseBitmap()
        invalidate()
    }

    // MARK: - Added Content Selection & Manipulation
    fun reloadAddedContent() {
        val doc = document ?: return
        val page = pageIndex
        scope.launch(Dispatchers.IO) {
            val items = try { doc.listAddedContent(page) } catch (_: Exception) { emptyList() }
            withContext(Dispatchers.Main) {
                if (pageIndex == page) {
                    addedContent = items
                    val sel = selectedContentItem
                    if (sel != null) {
                        val matching = items.firstOrNull { it.index == sel.index }
                        selectedContentItem = matching
                        if (matching != null) {
                            liveItemRect.set(matching.rect)
                        }
                    }
                    invalidate()
                }
            }
        }
    }

    fun selectContentItem(item: PdfAddedItem?) {
        selectedContentItem = item
        if (item != null) {
            liveItemRect.set(item.rect)
        }
        onContentItemSelected?.invoke(item)
        invalidate()
    }

    fun deleteSelectedContent() {
        val doc = document ?: return
        val item = selectedContentItem ?: return
        val page = pageIndex
        scope.launch(Dispatchers.IO) {
            try {
                doc.deleteContent(page, item.index)
                withContext(Dispatchers.Main) {
                    selectedContentItem = null
                    refreshPage()
                    onContentItemDeleted?.invoke(item)
                }
            } catch (_: Exception) {}
        }
    }

    private fun hitTestResizeHandle(pageX: Float, pageY: Float, rect: RectF, touchRadius: Float): ResizeHandle {
        val r = touchRadius
        if (kotlin.math.abs(pageX - rect.left) <= r && kotlin.math.abs(pageY - rect.top) <= r) return ResizeHandle.TOP_LEFT
        if (kotlin.math.abs(pageX - rect.right) <= r && kotlin.math.abs(pageY - rect.top) <= r) return ResizeHandle.TOP_RIGHT
        if (kotlin.math.abs(pageX - rect.left) <= r && kotlin.math.abs(pageY - rect.bottom) <= r) return ResizeHandle.BOTTOM_LEFT
        if (kotlin.math.abs(pageX - rect.right) <= r && kotlin.math.abs(pageY - rect.bottom) <= r) return ResizeHandle.BOTTOM_RIGHT
        return ResizeHandle.NONE
    }

    // Rendering & Caching
    private val paint = Paint(Paint.FILTER_BITMAP_FLAG)
    private val highlightPaint = Paint().apply {
        color = Color.argb(120, 255, 235, 59) // Translucent amber
        style = Paint.Style.FILL
    }
    private val selectionPaint = Paint().apply {
        color = Color.argb(90, 33, 150, 243) // Translucent blue
        style = Paint.Style.FILL
    }
    private val annotationSelectionPaint = Paint().apply {
        color = Color.rgb(33, 150, 243)
        style = Paint.Style.STROKE
        strokeWidth = 3f
        pathEffect = DashPathEffect(floatArrayOf(10f, 10f), 0f)
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
    private val contentSelectionBorderPaint = Paint().apply {
        color = Color.rgb(25, 118, 210)
        style = Paint.Style.STROKE
        strokeWidth = 3f
    }
    private val contentDashedPaint = Paint().apply {
        color = Color.argb(140, 33, 150, 243)
        style = Paint.Style.STROKE
        strokeWidth = 2f
        pathEffect = DashPathEffect(floatArrayOf(8f, 8f), 0f)
    }
    private val contentDragFillPaint = Paint().apply {
        color = Color.argb(35, 33, 150, 243)
        style = Paint.Style.FILL
    }
    private val redactLiveStrokePaint = Paint().apply {
        color = Color.rgb(229, 57, 53)
        style = Paint.Style.STROKE
        strokeWidth = 3f
    }
    private val redactLiveFillPaint = Paint().apply {
        color = Color.argb(60, 229, 57, 53)
        style = Paint.Style.FILL
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

    private val tileSize = 512
    private val maxCacheMemoryBytes = (Runtime.getRuntime().maxMemory() / 4).toInt().coerceAtMost(64 * 1024 * 1024)
    private val tileCache = object : LruCache<String, Bitmap>(maxCacheMemoryBytes) {
        override fun sizeOf(key: String, value: Bitmap): Int = value.byteCount
    }
    private val inFlightTiles = mutableSetOf<String>()

    private var baseBitmap: Bitmap? = null
    private var baseBitmapScale: Float = 1.0f
    private val scope = CoroutineScope(Dispatchers.Main + SupervisorJob())

    init {
        scaleDetector = ScaleGestureDetector(context, object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
            override fun onScale(detector: ScaleGestureDetector): Boolean {
                if (isInkMode || isEraserMode) return false
                val factor = detector.scaleFactor
                val newZoom = (zoomLevel * factor).coerceIn(minScale, maxScale)
                if (newZoom != zoomLevel) {
                    val focusX = detector.focusX
                    val focusY = detector.focusY
                    offsetX = focusX - (focusX - offsetX) * (newZoom / zoomLevel)
                    offsetY = focusY - (focusY - offsetY) * (newZoom / zoomLevel)
                    zoomLevel = newZoom
                    clampOffsets()
                    invalidate()
                }
                return true
            }

            override fun onScaleEnd(detector: ScaleGestureDetector) {
                super.onScaleEnd(detector)
                // When pinch-zoom ends, refresh high-DPI tiles for the settled zoom level
                requestVisibleTiles()
            }
        })

        gestureDetector = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
            override fun onDown(e: MotionEvent): Boolean {
                scroller.forceFinished(true)
                return true
            }

            override fun onScroll(e1: MotionEvent?, e2: MotionEvent, distanceX: Float, distanceY: Float): Boolean {
                if (isInkMode || isEraserMode) return false
                offsetX -= distanceX
                offsetY -= distanceY
                clampOffsets()
                invalidate()
                return true
            }

            override fun onFling(e1: MotionEvent?, e2: MotionEvent, velocityX: Float, velocityY: Float): Boolean {
                if (isInkMode || isEraserMode) return false
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
                if (isInkMode || isEraserMode) return false
                if (zoomLevel > 1.2f) {
                    // Reset to 1.0x fit width
                    zoomLevel = 1.0f
                    clampOffsets()
                    invalidate()
                } else {
                    // Zoom in 2.5x centered on double-tap focus
                    val targetZoom = 2.5f
                    val focusX = e.x
                    val focusY = e.y
                    offsetX = focusX - (focusX - offsetX) * (targetZoom / zoomLevel)
                    offsetY = focusY - (focusY - offsetY) * (targetZoom / zoomLevel)
                    zoomLevel = targetZoom
                    clampOffsets()
                    requestVisibleTiles()
                    invalidate()
                }
                return true
            }

            override fun onSingleTapConfirmed(e: MotionEvent): Boolean {
                if (isInkMode || isEraserMode || isRedactMode) return false
                val doc = document ?: return false
                val geom = pageGeometry ?: return false

                val pageX = (e.x - offsetX) / scale
                val pageY = (e.y - offsetY) / scale

                if (pageX < 0f || pageX > geom.width || pageY < 0f || pageY > geom.height) {
                    onToggleChrome?.invoke()
                    return true
                }

                // 1. Check Links
                val links = try { doc.links(pageIndex) } catch (_: Exception) { emptyList() }
                val tappedLink = links.firstOrNull { it.rect.contains(pageX, pageY) }
                if (tappedLink != null) {
                    tappedLink.targetPage?.let { target ->
                        onNavigateToPage?.invoke(target.toInt())
                        return true
                    }
                    tappedLink.targetUri?.let { uri ->
                        onOpenUri?.invoke(uri)
                        return true
                    }
                }

                // 2. Check Interactive Form Fields
                val fields = try { doc.formFields(pageIndex) } catch (_: Exception) { emptyList() }
                val tappedField = fields.firstOrNull { it.rect.contains(pageX, pageY) }
                if (tappedField != null) {
                    when (tappedField.kind) {
                        uniffi.pdfcraft_ffi.FfiFieldKind.CHECK_BOX,
                        uniffi.pdfcraft_ffi.FfiFieldKind.RADIO -> {
                            val currentVal = tappedField.value.firstOrNull() ?: "Off"
                            val newVal = if (currentVal == "Off" || currentVal.isEmpty()) "Yes" else "Off"
                            try {
                                doc.setFormFieldValue(tappedField.name, newVal)
                                tileCache.evictAll()
                                loadBaseBitmap()
                                invalidate()
                            } catch (_: Exception) {}
                            return true
                        }
                        uniffi.pdfcraft_ffi.FfiFieldKind.TEXT -> {
                            onEditTextField?.invoke(tappedField)
                            return true
                        }
                        uniffi.pdfcraft_ffi.FfiFieldKind.CHOICE -> {
                            onSelectChoiceField?.invoke(tappedField)
                            return true
                        }
                        else -> {}
                    }
                }

                // 3. Check Annotations
                val annot = selectAnnotation(pageX, pageY)
                if (annot != null) {
                    return true
                }

                // 4. Default: Toggle Chrome (Immersive Reading Mode)
                onToggleChrome?.invoke()
                return true
            }

            override fun onLongPress(e: MotionEvent) {
                if (isInkMode || isEraserMode || isRedactMode || isDraggingContent || activeResizeHandle != ResizeHandle.NONE) return
                val doc = document ?: return
                val pageX = (e.x - offsetX) / scale
                val pageY = (e.y - offsetY) / scale

                // Find text at long press position
                try {
                    val pageText = doc.text(pageIndex)
                    if (pageText.isNotBlank()) {
                        val words = pageText.split(Regex("\\s+")).filter { it.length >= 2 }.take(30)
                        for (word in words) {
                            val matches = try { doc.find(pageIndex, word) } catch (_: Exception) { emptyList() }
                            val match = matches.firstOrNull { it.rect.contains(pageX, pageY) }
                            if (match != null) {
                                selectText(match.startGlyph.toInt(), match.endGlyph.toInt())
                                return
                            }
                        }
                    }
                } catch (_: Exception) {}
            }
        })
    }

    private fun FfiRect.contains(px: Float, py: Float): Boolean {
        return px >= x && px <= (x + width) && py >= y && py <= (y + height)
    }

    fun setDocument(doc: PdfDocument, page: Int = 0) {
        this.document = doc
        this.pageIndex = page
        this.pageGeometry = try { doc.geometry(page) } catch (_: Exception) { null }
        this.zoomLevel = 1.0f
        this.baseBitmap = null
        this.selectedContentItem = null
        tileCache.evictAll()
        inFlightTiles.clear()

        loadBaseBitmap()
        reloadAddedContent()
        requestLayout()
        invalidate()
    }

    fun refreshPage() {
        tileCache.evictAll()
        inFlightTiles.clear()
        loadBaseBitmap()
        reloadAddedContent()
        requestVisibleTiles()
        invalidate()
    }

    private fun loadBaseBitmap() {
        val doc = document ?: return
        val page = pageIndex
        val geom = pageGeometry ?: return
        val rc = readingColors
        val hc = hideComments

        val targetScale = (if (width > 0) width.toFloat() / geom.width else fitWidthScale)
            .coerceIn(1.2f, 3.5f)

        scope.launch(Dispatchers.IO) {
            val bmp = try {
                doc.renderPage(page, targetScale, preview = false, readingColors = rc, hideComments = hc)
            } catch (_: Exception) {
                null
            }
            withContext(Dispatchers.Main) {
                if (pageIndex == page && bmp != null) {
                    baseBitmap = bmp
                    baseBitmapScale = targetScale
                    clampOffsets()
                    invalidate()
                }
            }
        }
    }

    private fun requestVisibleTiles() {
        val doc = document ?: return
        val geom = pageGeometry ?: return
        if (zoomLevel <= 1.15f) return

        val pageW = geom.width * scale
        val pageH = geom.height * scale

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
        val qScale = (round(currentScale * 10f) / 10f)
        val currentPage = pageIndex
        val rc = readingColors
        val hc = hideComments

        for (row in startRow..endRow) {
            for (col in startCol..endCol) {
                val tileX = col * tileSize
                val tileY = row * tileSize
                val tileW = min(tileSize.toFloat(), pageW - tileX)
                val tileH = min(tileSize.toFloat(), pageH - tileY)

                val tileKey = "$currentPage@${qScale}_${col}_$row"
                if (tileCache.get(tileKey) == null && !inFlightTiles.contains(tileKey)) {
                    inFlightTiles.add(tileKey)
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
                                if (pageIndex == currentPage) {
                                    invalidate()
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (isRedactMode) {
            val geom = pageGeometry ?: return super.onTouchEvent(event)
            val pageX = ((event.x - offsetX) / scale).coerceIn(0f, geom.width)
            val pageY = ((event.y - offsetY) / scale).coerceIn(0f, geom.height)

            when (event.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    redactStartTouchX = pageX
                    redactStartTouchY = pageY
                    liveRedactRect.set(pageX, pageY, pageX, pageY)
                    isDraggingRedact = true
                    parent?.requestDisallowInterceptTouchEvent(true)
                    invalidate()
                    return true
                }
                MotionEvent.ACTION_MOVE -> {
                    if (isDraggingRedact) {
                        val l = min(redactStartTouchX, pageX)
                        val t = min(redactStartTouchY, pageY)
                        val r = max(redactStartTouchX, pageX)
                        val b = max(redactStartTouchY, pageY)
                        liveRedactRect.set(l, t, r, b)
                        invalidate()
                        return true
                    }
                }
                MotionEvent.ACTION_UP -> {
                    if (isDraggingRedact) {
                        isDraggingRedact = false
                        val w = liveRedactRect.width()
                        val h = liveRedactRect.height()
                        if (w >= 8f && h >= 8f) {
                            val doc = document
                            val page = pageIndex
                            val rectToMark = FfiRect(
                                x = liveRedactRect.left,
                                y = liveRedactRect.top,
                                width = w,
                                height = h
                            )
                            liveRedactRect.setEmpty()
                            scope.launch(Dispatchers.IO) {
                                try {
                                    doc?.markRedaction(page, rectToMark, overlayText = "REDACTED")
                                    withContext(Dispatchers.Main) {
                                        refreshPage()
                                        onRedactionMarked?.invoke()
                                    }
                                } catch (_: Exception) {}
                            }
                        } else {
                            liveRedactRect.setEmpty()
                            invalidate()
                        }
                        return true
                    }
                }
                MotionEvent.ACTION_CANCEL -> {
                    isDraggingRedact = false
                    liveRedactRect.setEmpty()
                    invalidate()
                    return true
                }
            }
            return true
        }

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
                        loadBaseBitmap()
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
                        loadBaseBitmap()
                    }
                    invalidate()
                    return true
                }
                MotionEvent.ACTION_UP -> {
                    if (isInkMode && liveStrokePoints.size >= 2) {
                        val pts = liveStrokePoints.map { uniffi.pdfcraft_ffi.FfiPoint(it.x, it.y) }
                        document?.addInkStroke(pageIndex, pts, inkColorRgb, inkStrokeWidth)
                        tileCache.evictAll()
                        loadBaseBitmap()
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

        val geom = pageGeometry
        if (geom != null && (isEditContentMode || addedContent.isNotEmpty())) {
            val pageX = (event.x - offsetX) / scale
            val pageY = (event.y - offsetY) / scale
            val handleRadius = 24f / scale

            when (event.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    val sel = selectedContentItem
                    if (sel != null) {
                        val handle = hitTestResizeHandle(pageX, pageY, liveItemRect, handleRadius)
                        if (handle != ResizeHandle.NONE) {
                            activeResizeHandle = handle
                            dragStartTouchX = event.x
                            dragStartTouchY = event.y
                            dragInitialRect.set(liveItemRect)
                            parent?.requestDisallowInterceptTouchEvent(true)
                            invalidate()
                            return true
                        }
                        if (liveItemRect.contains(pageX, pageY)) {
                            isDraggingContent = true
                            activeResizeHandle = ResizeHandle.NONE
                            dragStartTouchX = event.x
                            dragStartTouchY = event.y
                            dragInitialRect.set(liveItemRect)
                            parent?.requestDisallowInterceptTouchEvent(true)
                            invalidate()
                            return true
                        }
                    }

                    // Check if tapped another added content item
                    val hitItem = addedContent.find { it.rect.contains(pageX, pageY) }
                    if (hitItem != null) {
                        selectContentItem(hitItem)
                        isDraggingContent = true
                        activeResizeHandle = ResizeHandle.NONE
                        dragStartTouchX = event.x
                        dragStartTouchY = event.y
                        dragInitialRect.set(hitItem.rect)
                        parent?.requestDisallowInterceptTouchEvent(true)
                        invalidate()
                        return true
                    } else if (selectedContentItem != null && !isDraggingContent && activeResizeHandle == ResizeHandle.NONE) {
                        if (!isEditContentMode) {
                            selectContentItem(null)
                        }
                    }
                }
                MotionEvent.ACTION_MOVE -> {
                    if (isDraggingContent) {
                        val dx = (event.x - dragStartTouchX) / scale
                        val dy = (event.y - dragStartTouchY) / scale
                        val w = dragInitialRect.width()
                        val h = dragInitialRect.height()
                        val newLeft = (dragInitialRect.left + dx).coerceIn(0f, geom.width - w)
                        val newTop = (dragInitialRect.top + dy).coerceIn(0f, geom.height - h)
                        liveItemRect.set(newLeft, newTop, newLeft + w, newTop + h)
                        invalidate()
                        return true
                    } else if (activeResizeHandle != ResizeHandle.NONE) {
                        val dx = (event.x - dragStartTouchX) / scale
                        val dy = (event.y - dragStartTouchY) / scale
                        val minSize = 20f

                        when (activeResizeHandle) {
                            ResizeHandle.TOP_LEFT -> {
                                val newLeft = min(dragInitialRect.right - minSize, dragInitialRect.left + dx).coerceAtLeast(0f)
                                val newTop = min(dragInitialRect.bottom - minSize, dragInitialRect.top + dy).coerceAtLeast(0f)
                                liveItemRect.set(newLeft, newTop, dragInitialRect.right, dragInitialRect.bottom)
                            }
                            ResizeHandle.TOP_RIGHT -> {
                                val newRight = max(dragInitialRect.left + minSize, dragInitialRect.right + dx).coerceAtMost(geom.width)
                                val newTop = min(dragInitialRect.bottom - minSize, dragInitialRect.top + dy).coerceAtLeast(0f)
                                liveItemRect.set(dragInitialRect.left, newTop, newRight, dragInitialRect.bottom)
                            }
                            ResizeHandle.BOTTOM_LEFT -> {
                                val newLeft = min(dragInitialRect.right - minSize, dragInitialRect.left + dx).coerceAtLeast(0f)
                                val newBottom = max(dragInitialRect.top + minSize, dragInitialRect.bottom + dy).coerceAtMost(geom.height)
                                liveItemRect.set(newLeft, dragInitialRect.top, dragInitialRect.right, newBottom)
                            }
                            ResizeHandle.BOTTOM_RIGHT -> {
                                val newRight = max(dragInitialRect.left + minSize, dragInitialRect.right + dx).coerceAtMost(geom.width)
                                val newBottom = max(dragInitialRect.top + minSize, dragInitialRect.bottom + dy).coerceAtMost(geom.height)
                                liveItemRect.set(dragInitialRect.left, dragInitialRect.top, newRight, newBottom)
                            }
                            ResizeHandle.NONE -> {}
                        }
                        invalidate()
                        return true
                    }
                }
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                    if (isDraggingContent || activeResizeHandle != ResizeHandle.NONE) {
                        val doc = document
                        val sel = selectedContentItem
                        val page = pageIndex
                        val hasMoved = (kotlin.math.abs(liveItemRect.left - dragInitialRect.left) > 1f ||
                                kotlin.math.abs(liveItemRect.top - dragInitialRect.top) > 1f ||
                                kotlin.math.abs(liveItemRect.width() - dragInitialRect.width()) > 1f ||
                                kotlin.math.abs(liveItemRect.height() - dragInitialRect.height()) > 1f)

                        isDraggingContent = false
                        activeResizeHandle = ResizeHandle.NONE

                        if (hasMoved && doc != null && sel != null) {
                            val committedRect = RectF(liveItemRect)
                            selectedContentItem = sel.copy(rect = committedRect)
                            scope.launch(Dispatchers.IO) {
                                try {
                                    doc.updateContent(page, sel.index, committedRect)
                                    withContext(Dispatchers.Main) {
                                        refreshPage()
                                        onContentItemUpdated?.invoke(sel.copy(rect = committedRect))
                                    }
                                } catch (_: Exception) {}
                            }
                        }
                        invalidate()
                        return true
                    }
                }
            }
        }

        return try {
            var ret = scaleDetector.onTouchEvent(event)
            ret = gestureDetector.onTouchEvent(event) || ret
            ret || super.onTouchEvent(event)
        } catch (_: Exception) {
            true
        }
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
        if (w > 0 && oldw <= 0) {
            loadBaseBitmap()
        }
        clampOffsets()
    }

    fun zoomToFitWidth() {
        zoomLevel = 1.0f
        clampOffsets()
        invalidate()
    }

    fun zoomToFitPage() {
        val geom = pageGeometry ?: return
        if (width > 0 && height > 0 && geom.width > 0 && geom.height > 0) {
            val fitW = width / geom.width
            val fitH = height / geom.height
            zoomLevel = (min(fitW, fitH) / fitWidthScale).coerceIn(minScale, maxScale)
            clampOffsets()
            invalidate()
        }
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val geom = pageGeometry ?: return

        canvas.save()
        canvas.translate(offsetX, offsetY)

        val pageW = geom.width * scale
        val pageH = geom.height * scale

        // Draw page white background & subtle shadow/border
        canvas.drawRect(0f, 0f, pageW, pageH, backgroundPaint)
        canvas.drawRect(0f, 0f, pageW, pageH, borderPaint)

        // 1. Draw High-DPI Base Bitmap scaled to current viewport
        val base = baseBitmap
        if (base != null && !base.isRecycled) {
            val src = Rect(0, 0, base.width, base.height)
            val dst = RectF(0f, 0f, pageW, pageH)
            canvas.drawBitmap(base, src, dst, paint)
        }

        // 2. Draw High-Res Tiles if zoomed in
        if (zoomLevel > 1.15f) {
            val currentScale = scale
            val qScale = (round(currentScale * 10f) / 10f)
            val currentPage = pageIndex

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

            for (row in startRow..endRow) {
                for (col in startCol..endCol) {
                    val tileX = col * tileSize
                    val tileY = row * tileSize
                    val tileW = min(tileSize.toFloat(), pageW - tileX)
                    val tileH = min(tileSize.toFloat(), pageH - tileY)

                    val tileKey = "$currentPage@${qScale}_${col}_$row"
                    val cachedTile = tileCache.get(tileKey)
                    if (cachedTile != null && !cachedTile.isRecycled) {
                        val dstRect = RectF(tileX.toFloat(), tileY.toFloat(), tileX + tileW, tileY + tileH)
                        canvas.drawBitmap(cachedTile, null, dstRect, paint)
                    }
                }
            }
        }

        // 3. Search Highlights overlay
        if (searchHighlights.isNotEmpty()) {
            for (hl in searchHighlights) {
                val dst = RectF(
                    hl.x * scale,
                    hl.y * scale,
                    (hl.x + hl.width) * scale,
                    (hl.y + hl.height) * scale
                )
                canvas.drawRect(dst, highlightPaint)
            }
        }

        // 4. Text Selection overlay
        if (selectedTextRects.isNotEmpty()) {
            for (sel in selectedTextRects) {
                val dst = RectF(
                    sel.x * scale,
                    sel.y * scale,
                    (sel.x + sel.width) * scale,
                    (sel.y + sel.height) * scale
                )
                canvas.drawRect(dst, selectionPaint)
            }
        }

        // 5. Selected annotation overlay (dashed border & handles)
        val annot = selectedAnnotation
        if (annot != null) {
            val r = RectF(
                annot.rect.x * scale,
                annot.rect.y * scale,
                (annot.rect.x + annot.rect.width) * scale,
                (annot.rect.y + annot.rect.height) * scale
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

        // 6. Live ink stroke overlay
        if (liveStrokePoints.size >= 2) {
            liveInkPath.reset()
            liveInkPath.moveTo(liveStrokePoints[0].x * scale, liveStrokePoints[0].y * scale)
            for (i in 1 until liveStrokePoints.size) {
                val pt = liveStrokePoints[i]
                liveInkPath.lineTo(pt.x * scale, pt.y * scale)
            }
            liveInkPaint.strokeWidth = inkStrokeWidth * scale
            liveInkPaint.color = Color.rgb(
                ((inkColorRgb shr 16) and 0xFFu).toInt(),
                ((inkColorRgb shr 8) and 0xFFu).toInt(),
                (inkColorRgb and 0xFFu).toInt()
            )
            canvas.drawPath(liveInkPath, liveInkPaint)
        }

        // 7. Added Content Selection & Transform Handles Overlay
        if (isEditContentMode) {
            for (item in addedContent) {
                if (item.index != selectedContentItem?.index) {
                    val r = RectF(
                        item.rect.left * scale,
                        item.rect.top * scale,
                        item.rect.right * scale,
                        item.rect.bottom * scale
                    )
                    canvas.drawRect(r, contentDashedPaint)
                }
            }
        }

        val selectedContent = selectedContentItem
        if (selectedContent != null) {
            val r = RectF(
                liveItemRect.left * scale,
                liveItemRect.top * scale,
                liveItemRect.right * scale,
                liveItemRect.bottom * scale
            )
            canvas.drawRect(r, contentDragFillPaint)
            canvas.drawRect(r, contentSelectionBorderPaint)

            val handleRadius = 8f * context.resources.displayMetrics.density
            val corners = arrayOf(
                PointF(r.left, r.top),
                PointF(r.right, r.top),
                PointF(r.left, r.bottom),
                PointF(r.right, r.bottom)
            )
            for (c in corners) {
                canvas.drawCircle(c.x, c.y, handleRadius, handlePaint)
                canvas.drawCircle(c.x, c.y, handleRadius, handleStrokePaint)
            }
        }

        // 8. Live Redaction Drag Rectangle
        if (isRedactMode && !liveRedactRect.isEmpty) {
            val r = RectF(
                liveRedactRect.left * scale,
                liveRedactRect.top * scale,
                liveRedactRect.right * scale,
                liveRedactRect.bottom * scale
            )
            canvas.drawRect(r, redactLiveFillPaint)
            canvas.drawRect(r, redactLiveStrokePaint)
        }

        canvas.restore()
    }
}
