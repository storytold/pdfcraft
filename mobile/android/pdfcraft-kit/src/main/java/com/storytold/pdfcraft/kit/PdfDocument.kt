package com.storytold.pdfcraft.kit

import android.graphics.Bitmap
import android.graphics.RectF
import org.json.JSONArray
import org.json.JSONObject
import uniffi.pdfcraft_ffi.*
import java.io.Closeable
import java.io.File
import java.nio.ByteBuffer
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Represents an added content element (image or text) on a page.
 */
data class PdfAddedItem(
    val page: Int,       // 0-based page index
    val index: Int,      // 1-based index on page
    val type: String,    // "image" or "text"
    val rect: RectF,     // [left, top, right, bottom] in page coordinates
    val text: String? = null
)

/**
 * Thread-safe wrapper around a PdfEngine document.
 */
class PdfDocument(
    val engine: Engine = Engine(),
    val name: String,
    val docId: ULong,
    initialPageCount: Int
) : Closeable {
    private val isClosed = AtomicBoolean(false)
    private val geometryCache = ConcurrentHashMap<UInt, FfiPageGeometry>()

    @Volatile
    var pageCount: Int = initialPageCount
        private set

    companion object {
        /**
         * Opens a PDF document from raw in-memory bytes.
         */
        fun openBytes(engine: Engine = Engine(), name: String = "document.pdf", bytes: ByteArray, password: String? = null): PdfDocument {
            val id = engine.openBytes(name, bytes, password)
            val count = engine.pageCount(id).toInt()
            return PdfDocument(engine, name, id, count)
        }

        /**
         * Opens a PDF document from a file path.
         */
        fun openFile(engine: Engine = Engine(), path: String, password: String? = null): PdfDocument {
            val id = engine.openFile(path, password)
            val count = engine.pageCount(id).toInt()
            val fileName = File(path).name
            return PdfDocument(engine, fileName, id, count)
        }

        /**
         * Creates a new PDF document from a list of encoded images (PNG / JPEG).
         */
        fun createFromImages(
            engine: Engine = Engine(),
            name: String = "scanned.pdf",
            images: List<ByteArray>
        ): PdfDocument {
            val id = engine.createFromImages(name, images)
            val count = engine.pageCount(id).toInt()
            return PdfDocument(engine, name, id, count)
        }

        /**
         * Creates a new empty PDF document with a single standard letter-sized page.
         */
        fun createBlank(
            engine: Engine = Engine(),
            name: String = "Untitled.pdf",
            width: Float = 612f,
            height: Float = 792f
        ): PdfDocument {
            val minimalPdf = "%PDF-1.7\n" +
                "1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n" +
                "2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n" +
                "3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 ${width.toInt()} ${height.toInt()}] >> endobj\n" +
                "xref\n0 4\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000115 00000 n \n" +
                "trailer << /Size 4 /Root 1 0 R >>\nstartxref\n193\n%%EOF\n"
            return openBytes(engine = engine, bytes = minimalPdf.toByteArray(Charsets.UTF_8), name = name)
        }

        fun createSelfSignedDigitalId(
            engine: Engine = Engine(),
            name: String,
            password: String,
            keyType: String = "rsa2048",
            organization: String? = null,
            unit: String? = null,
            email: String? = null,
            country: String? = null,
            validityYears: UInt = 5u
        ): ByteArray {
            return engine.createSelfSignedDigitalId(
                name = name,
                password = password,
                keyType = keyType,
                organization = organization,
                unit = unit,
                email = email,
                country = country,
                validityYears = validityYears
            )
        }

        fun inspectDigitalId(
            engine: Engine = Engine(),
            p12Bytes: ByteArray,
            password: String? = null
        ): FfiDigitalIdInfo {
            return engine.inspectDigitalId(p12Bytes, password)
        }

        fun ocrStatus(engine: Engine = Engine()): FfiOcrStatus = engine.ocrStatus()

        fun setOcrModelsDir(engine: Engine = Engine(), dir: String) = engine.setOcrModelsDir(dir)
    }

    override fun close() {
        if (isClosed.compareAndSet(false, true)) {
            try {
                engine.close(docId)
            } catch (_: Exception) {}
        }
    }

    /**
     * Trims internal caches and unneeded memory in response to OS memory pressure (onTrimMemory).
     */
    fun trimMemory() {
        try {
            engine.trimMemory()
        } catch (_: Exception) {}
    }

    /**
     * Handles stepwise memory pressure degradation (Moderate: halves tile cache; Critical: drops all caches).
     */
    fun handleMemoryPressure(level: uniffi.pdfcraft_ffi.FfiMemoryPressureLevel) {
        try {
            engine.handleMemoryPressure(level)
        } catch (_: Exception) {}
    }

    /**
     * Retrieves geometry for a given zero-based page index.
     */
    fun geometry(page: Int): FfiPageGeometry {
        val p = page.toUInt()
        return geometryCache.computeIfAbsent(p) {
            engine.pageGeometry(docId, it)
        }
    }

    /**
     * Retrieves document metadata information.
     */
    fun info(): FfiDocInfo = engine.docInfo(docId)

    /**
     * Retrieves outline / table of contents tree.
     */
    fun outline(): List<FfiOutlineItem> = engine.outline(docId)

    /**
     * Retrieves hyperlinks on a page.
     */
    fun links(page: Int): List<FfiLink> = engine.pageLinks(docId, page.toUInt())

    /**
     * Extracts plain text from a page.
     */
    fun text(page: Int): String = engine.pageText(docId, page.toUInt())

    /**
     * Searches for query text on a specific page.
     */
    fun find(page: Int, query: String, caseSensitive: Boolean = false): List<FfiFindMatch> =
        engine.findText(docId, page.toUInt(), query, caseSensitive)

    /**
     * Searches for query text across all pages in the document.
     */
    fun findAll(query: String, caseSensitive: Boolean = false): List<FfiFindMatch> {
        val all = mutableListOf<FfiFindMatch>()
        for (p in 0 until pageCount) {
            all.addAll(find(p, query, caseSensitive))
        }
        return all
    }

    /**
     * Extracts view-space selection rectangles for a glyph range on a page.
     */
    fun textSelectionRects(page: Int, startGlyph: Int, endGlyph: Int): List<FfiRect> =
        engine.textSelectionRects(docId, page.toUInt(), startGlyph.toUInt(), endGlyph.toUInt())

    /**
     * Returns plain text for a glyph range on a page.
     */
    fun textSelectionString(page: Int, startGlyph: Int, endGlyph: Int): String =
        engine.textSelectionString(docId, page.toUInt(), startGlyph.toUInt(), endGlyph.toUInt())

    /**
     * Finds the glyph nearest to a point in view coordinates.
     */
    fun nearestGlyphAt(page: Int, x: Float, y: Float): Int? =
        engine.nearestGlyphAt(docId, page.toUInt(), x, y)?.toInt()

    /**
     * Adds a text markup annotation (highlight, underline, strikeout, squiggly) covering the specified glyph selection range.
     */
    fun addTextMarkup(
        page: Int,
        startGlyph: Int,
        endGlyph: Int,
        kind: uniffi.pdfcraft_ffi.FfiTextMarkupKind,
        colorRgb: UInt? = null,
        author: String? = null,
        contents: String? = null
    ) {
        engine.addTextMarkupSelection(
            docId,
            page.toUInt(),
            startGlyph.toUInt(),
            endGlyph.toUInt(),
            kind,
            colorRgb,
            author,
            contents
        )
    }

    /**
     * Adds a text markup annotation covering the given view-space rectangles.
     */
    fun addTextMarkupRects(
        page: Int,
        rects: List<FfiRect>,
        kind: uniffi.pdfcraft_ffi.FfiTextMarkupKind,
        colorRgb: UInt? = null,
        author: String? = null,
        contents: String? = null
    ) {
        engine.addTextMarkupRects(
            docId,
            page.toUInt(),
            rects,
            kind,
            colorRgb,
            author,
            contents
        )
    }

    /**
     * Hit-tests for an interactive annotation at point (x, y) in view coordinates.
     */
    fun hitTestAnnotation(page: Int, x: Float, y: Float): uniffi.pdfcraft_ffi.FfiAnnotationSummary? =
        engine.hitTestAnnotation(docId, page.toUInt(), x, y)

    /**
     * Moves an annotation on a page by delta view points.
     */
    fun moveAnnotation(page: Int, index: Int, dx: Float, dy: Float) {
        engine.moveAnnotation(docId, page.toUInt(), index.toUInt(), dx, dy)
    }

    /**
     * Resizes an annotation to a new bounding box in view coordinates.
     */
    fun resizeAnnotation(page: Int, index: Int, newRect: FfiRect) {
        engine.resizeAnnotation(docId, page.toUInt(), index.toUInt(), newRect)
    }

    /**
     * Changes an annotation's style (stroke color, opacity, stroke width, and fill color).
     */
    fun styleAnnotation(
        page: Int,
        index: Int,
        strokeColorRgb: UInt? = null,
        opacity: Float? = null,
        strokeWidth: Float? = null,
        fillColorRgb: UInt? = null
    ) {
        engine.styleAnnotation(
            docId,
            page.toUInt(),
            index.toUInt(),
            strokeColorRgb,
            opacity,
            strokeWidth,
            fillColorRgb
        )
    }

    /**
     * Deletes an annotation from a page.
     */
    fun deleteAnnotation(page: Int, index: Int) {
        engine.deleteAnnotation(docId, page.toUInt(), index.toUInt())
    }

    /**
     * Adds a sticky note annotation at a point in view coordinates.
     */
    fun addNoteAnnotation(
        page: Int,
        x: Float,
        y: Float,
        contents: String,
        author: String? = null,
        icon: String? = null,
        colorRgb: UInt? = null
    ) {
        engine.addNoteAnnotation(
            docId,
            page.toUInt(),
            uniffi.pdfcraft_ffi.FfiPoint(x, y),
            contents,
            author,
            icon,
            colorRgb
        )
    }

    /**
     * Adds a free text / text box annotation in view coordinates.
     */
    fun addTextBoxAnnotation(
        page: Int,
        rect: FfiRect,
        contents: String,
        fontSize: Float = 14f,
        author: String? = null,
        colorRgb: UInt? = null
    ) {
        engine.addTextBoxAnnotation(
            docId,
            page.toUInt(),
            rect,
            contents,
            fontSize,
            author,
            colorRgb
        )
    }

    /**
     * Adds a vector shape annotation (rectangle, oval, line, arrow) in view coordinates.
     */
    fun addShapeAnnotation(
        page: Int,
        kind: uniffi.pdfcraft_ffi.FfiShapeKind,
        rect: FfiRect,
        strokeColor: UInt? = null,
        fillColor: UInt? = null,
        strokeWidth: Float? = null,
        author: String? = null
    ) {
        engine.addShapeAnnotation(
            docId,
            page.toUInt(),
            kind,
            rect,
            strokeColor,
            fillColor,
            strokeWidth,
            author
        )
    }

    /**
     * Adds a rubber stamp annotation in view coordinates.
     */
    fun addStampAnnotation(
        page: Int,
        rect: FfiRect,
        stampName: String = "Approved",
        author: String? = null
    ) {
        engine.addStampAnnotation(
            docId,
            page.toUInt(),
            rect,
            stampName,
            author
        )
    }

    /**
     * Adds a multi-stroke ink drawing annotation in view coordinates.
     */
    fun addInkAnnotation(
        page: Int,
        strokes: List<List<uniffi.pdfcraft_ffi.FfiPoint>>,
        colorRgb: UInt? = null,
        strokeWidth: Float? = null,
        opacity: Float? = null,
        author: String? = null
    ) {
        engine.addInkAnnotation(
            docId,
            page.toUInt(),
            strokes,
            colorRgb,
            strokeWidth,
            opacity,
            author
        )
    }

    /**
     * Adds a single pen-up ink stroke as an annotation in view coordinates.
     */
    fun addInkStroke(
        page: Int,
        points: List<uniffi.pdfcraft_ffi.FfiPoint>,
        colorRgb: UInt = 0x0000FFu,
        width: Float = 2.0f
    ) {
        val stroke = uniffi.pdfcraft_ffi.FfiInkStroke(
            page = page.toUInt(),
            points = points,
            colorRgb = colorRgb,
            width = width
        )
        engine.addInkStroke(docId, stroke)
    }

    /**
     * Erases ink strokes intersecting a trajectory path on a page. Returns true if ink was erased.
     */
    fun eraseInk(
        page: Int,
        path: List<uniffi.pdfcraft_ffi.FfiPoint>,
        radius: Float = 16f
    ): Boolean {
        return engine.eraseInkAt(docId, page.toUInt(), path, radius)
    }

    /**
     * Returns all interactive comments on a page, including replies and status items.
     */
    fun pageComments(page: Int): List<uniffi.pdfcraft_ffi.FfiCommentItem> =
        engine.pageComments(docId, page.toUInt())

    /**
     * Returns all interactive comments across all pages in the document.
     */
    fun allComments(): List<uniffi.pdfcraft_ffi.FfiCommentItem> =
        engine.allComments(docId)

    /**
     * Adds a threaded reply to a comment annotation.
     */
    fun replyToComment(page: Int, index: Int, text: String, author: String) {
        engine.replyToComment(docId, page.toUInt(), index.toUInt(), text, author)
    }

    /**
     * Sets the review state (e.g. Accepted, Rejected, Completed) on an annotation thread.
     */
    fun setCommentReviewState(page: Int, index: Int, state: uniffi.pdfcraft_ffi.FfiReviewState, author: String) {
        engine.setCommentReviewState(docId, page.toUInt(), index.toUInt(), state, author)
    }

    /**
     * Updates the text contents of a comment annotation.
     */
    fun setCommentContents(page: Int, index: Int, text: String) {
        engine.setCommentContents(docId, page.toUInt(), index.toUInt(), text)
    }

    /**
     * Toggles or sets Acrobat's "Mark with checkmark" review flag on an annotation.
     */
    fun setCommentMarked(page: Int, index: Int, marked: Boolean, author: String) {
        engine.setCommentMarked(docId, page.toUInt(), index.toUInt(), marked, author)
    }


    /**
     * Renders a specific tile or full page as an Android Bitmap.
     */
    fun renderTile(
        page: Int,
        tile: FfiTile?,
        scale: Float,
        tag: ULong = 0u,
        preview: Boolean = false,
        readingColors: FfiReadingColors? = null,
        hideComments: Boolean = false
    ): Bitmap? {
        if (isClosed.get()) return null
        val req = FfiRenderRequest(
            page.toUInt(),
            tile,
            scale,
            tag,
            preview,
            readingColors,
            hideComments
        )
        val result = engine.renderTile(docId, req)
        if (result.error != null) {
            throw FfiException.Internal(result.error!!)
        }
        if (result.width == 0u || result.height == 0u || result.pixels.isEmpty()) {
            return null
        }
        val bmp = Bitmap.createBitmap(result.width.toInt(), result.height.toInt(), Bitmap.Config.ARGB_8888)
        bmp.copyPixelsFromBuffer(ByteBuffer.wrap(result.pixels))
        return bmp
    }

    /**
     * Renders an entire page at the specified scale.
     */
    fun renderPage(
        page: Int,
        scale: Float = 1.0f,
        preview: Boolean = false,
        readingColors: FfiReadingColors? = null,
        hideComments: Boolean = false
    ): Bitmap? {
        return renderTile(page, null, scale, 0u, preview, readingColors, hideComments)
    }

    /**
     * Fast thumbnail generation using low-detail preview mode.
     */
    fun renderThumbnail(page: Int, maxDimension: Float = 160f): Bitmap? {
        val geom = geometry(page)
        val maxSide = kotlin.math.max(geom.width, geom.height)
        val scale = if (maxSide > 0f) kotlin.math.min(1.0f, maxDimension / maxSide) else 0.2f
        return renderPage(page, scale, preview = true)
    }

    /**
     * Advances the render generation tag and cancels in-flight renders older than the new tag.
     */
    fun advanceGenerationAndCancelStale(): ULong {
        val newTag = engine.nextRenderTag()
        engine.cancelRenders(newTag)
        return newTag
    }

    /**
     * Computes a prioritized list of pages to prefetch based on current page and scroll direction.
     */
    fun planPrefetch(
        currentPage: Int,
        direction: uniffi.pdfcraft_ffi.FfiScrollDirection,
        lookahead: Int = 2
    ): List<uniffi.pdfcraft_ffi.FfiPrefetchPlan> {
        return engine.planPrefetch(currentPage.toUInt(), pageCount.toUInt(), direction, lookahead.toUInt())
    }

    // MARK: - AcroForm & Signatures

    /**
     * Returns the list of interactive form fields in the document, optionally filtered to a page.
     */
    fun formFields(page: Int? = null): List<uniffi.pdfcraft_ffi.FfiFormField> {
        return engine.formFields(docId, page?.toUInt())
    }

    /**
     * Sets the text value of a text form field.
     */
    fun setFormFieldText(name: String, text: String) {
        engine.setFormFieldText(docId, name, text)
    }

    /**
     * Sets the checked state of a check box form field.
     */
    fun setFormFieldCheck(name: String, checked: Boolean) {
        engine.setFormFieldCheck(docId, name, checked)
    }

    /**
     * Toggles a check box form field, returning the new checked state.
     */
    fun toggleFormFieldCheck(name: String): Boolean {
        return engine.toggleFormFieldCheck(docId, name)
    }

    /**
     * Selects an on-state for a radio button group (or null to deselect).
     */
    fun setFormFieldRadio(name: String, selectedOption: String?) {
        engine.setFormFieldRadio(docId, name, selectedOption)
    }

    /**
     * Sets the selected export value(s) of a combo box or list box form field.
     */
    fun setFormFieldChoice(name: String, picked: List<String>) {
        engine.setFormFieldChoice(docId, name, picked)
    }

    /**
     * General setter for form fields by string representation according to field type.
     */
    fun setFormFieldValue(name: String, value: String) {
        engine.setFormFieldValue(docId, name, value)
    }

    /**
     * Resets named form fields back to their default values (or all fields if names is null).
     */
    fun resetForm(names: List<String>? = null) {
        engine.resetForm(docId, names)
    }

    /**
     * Adds a freehand drawn visual signature annotation on a page.
     */
    fun addDrawnSignature(
        page: Int,
        strokes: List<List<uniffi.pdfcraft_ffi.FfiPoint>>,
        author: String? = null
    ) {
        engine.addDrawnSignature(docId, page.toUInt(), strokes, author)
    }

    /**
     * Adds a typed visual script signature annotation in the given bounding box on a page.
     */
    fun addTypedSignature(
        page: Int,
        text: String,
        rect: FfiRect,
        author: String? = null
    ) {
        engine.addTypedSignature(docId, page.toUInt(), text, rect, author)
    }

    /**
     * Signs a signature form field using freehand drawn normalized ink strokes (0.0..=1.0).
     */
    fun signFormFieldDrawn(
        fieldName: String,
        normalizedStrokes: List<List<uniffi.pdfcraft_ffi.FfiPoint>>,
        author: String? = null
    ) {
        engine.signFormFieldDrawn(docId, fieldName, normalizedStrokes, author)
    }

    /**
     * Signs a signature form field by rendering a typed name in the script typeface.
     */
    fun signFormFieldTyped(
        fieldName: String,
        text: String,
        author: String? = null
    ) {
        engine.signFormFieldTyped(docId, fieldName, text, author)
    }

    // MARK: - Undo, Redo, and Saving (MOB-4.6)

    /**
     * Returns the descriptive label of the next action that can be undone, or null if undo stack is empty.
     */
    val canUndo: String?
        get() = try { engine.canUndo(docId) } catch (_: Exception) { null }

    /**
     * Returns the descriptive label of the next action that can be redone, or null if redo stack is empty.
     */
    val canRedo: String?
        get() = try { engine.canRedo(docId) } catch (_: Exception) { null }

    /**
     * Undoes the last edit operation, returning the description of the undone action.
     */
    fun undo(): String {
        return engine.undo(docId)
    }

    /**
     * Redoes the last undone edit operation, returning the description of the redone action.
     */
    fun redo(): String {
        return engine.redo(docId)
    }

    /**
     * Whether the document currently has unsaved modifications.
     */
    val isDirty: Boolean
        get() = try { engine.isDirty(docId) } catch (_: Exception) { false }

    /**
     * Generates full updated PDF bytes (incremental update with /ModDate stamped) suitable for saving.
     */
    fun saveBytes(): ByteArray {
        return engine.saveBytes(docId)
    }

    /**
     * Marks the document cleanly saved after writing to disk.
     */
    fun markSaved(path: String? = null) {
        engine.markSaved(docId, path)
    }

    /**
     * Atomically saves the document to a target file path.
     */
    fun save(path: String) {
        engine.saveToPath(docId, path)
    }

    // MARK: - Tool Catalogue & Invocation (Phase 5)

    /**
     * Invokes an automation tool directly on this document with JSON arguments.
     */
    fun invokeTool(name: String, argsJson: String): FfiToolResult {
        val res = engine.invokeTool(name, argsJson)
        if (res.success) {
            geometryCache.clear()
        }
        return res
    }

    // MARK: - Page Operations (Phase 5)

    /**
     * Rotates specified page by degrees clockwise (90, 180, 270).
     */
    fun rotatePage(page: Int, degrees: Int = 90) {
        val p = page + 1
        invokeTool("page_rotate", """{"doc": $docId, "pages": [$p], "degrees": $degrees}""")
        geometryCache.clear()
        pageCount = engine.pageCount(docId).toInt()
    }

    /**
     * Deletes specified zero-based page.
     */
    fun deletePage(page: Int) {
        val p = page + 1
        invokeTool("page_delete", """{"doc": $docId, "pages": [$p]}""")
        geometryCache.clear()
        pageCount = engine.pageCount(docId).toInt()
    }

    /**
     * Inserts a blank page after the specified page index (or at beginning if afterPage is -1).
     */
    fun insertBlankPage(afterPage: Int = -1) {
        val at = if (afterPage >= 0) afterPage + 2 else 1
        invokeTool("page_insert_blank", """{"doc": $docId, "at": $at}""")
        geometryCache.clear()
        pageCount = engine.pageCount(docId).toInt()
    }

    // MARK: - Redaction & Security Audit (Phase 6 / MOB-6.2)

    fun markRedaction(page: Int, rect: FfiRect, overlayText: String? = null) {
        engine.markRedactionRect(docId, page.toUInt(), rect, overlayText)
    }

    fun markRedactionText(text: String, overlayText: String? = null): UInt {
        return engine.markRedactionText(docId, text, overlayText)
    }

    fun clearRedactions() {
        engine.clearRedactions(docId)
    }

    fun auditRedactions(): List<uniffi.pdfcraft_ffi.FfiRedactionFinding> {
        return engine.auditRedactions(docId)
    }

    fun applyRedactions(): uniffi.pdfcraft_ffi.FfiRedactionApplyResult {
        val res = engine.applyRedactions(docId)
        geometryCache.clear()
        return res
    }

    // MARK: - Digital Signatures & PKCS#12 (Phase 6 / MOB-6.3)

    fun listDigitalSignatures(): List<uniffi.pdfcraft_ffi.FfiDigitalSignature> {
        return engine.listDigitalSignatures(docId)
    }

    fun sign(p12Bytes: ByteArray, password: String? = null, options: uniffi.pdfcraft_ffi.FfiSignOptions): uniffi.pdfcraft_ffi.FfiSignResult {
        val res = engine.signDocumentP12(docId, p12Bytes, password, options)
        geometryCache.clear()
        return res
    }

    // MARK: - Scan & OCR (Phase 6 / MOB-6.4)

    fun recognizeText(page: Int, dpi: Float? = null, skipTextPages: Boolean = true): uniffi.pdfcraft_ffi.FfiOcrPageResult {
        return engine.recognizePageText(docId, page.toUInt(), dpi, skipTextPages)
    }

    fun applyOcrTextLayer(page: Int, words: List<uniffi.pdfcraft_ffi.FfiOcrWord>): UInt {
        return engine.applyOcrTextLayer(docId, page.toUInt(), words)
    }

    // MARK: - Advanced Capabilities (Phase 6 / MOB-6.5)

    fun isFormJavascriptEnabled(): Boolean = engine.isFormJavascriptEnabled()

    fun setFormJavascriptEnabled(enabled: Boolean) {
        engine.setFormJavascriptEnabled(enabled)
    }

    fun runJavascript(script: String, target: String? = null): uniffi.pdfcraft_ffi.FfiJsResult {
        return engine.runFormJavascript(docId, script, target)
    }

    fun declaredStandards(): uniffi.pdfcraft_ffi.FfiPreflightDeclared {
        return engine.declaredStandards(docId)
    }

    fun verifyPdfA(level: String = "pdfa2b"): List<uniffi.pdfcraft_ffi.FfiPreflightIssue> {
        return engine.verifyPdfa(docId, level)
    }

    fun convertToPdfA(level: String = "pdfa2b"): UInt {
        val res = engine.convertToPdfa(docId, level)
        geometryCache.clear()
        return res
    }

    fun checkAccessibility(): uniffi.pdfcraft_ffi.FfiA11yReport {
        return engine.checkAccessibility(docId)
    }

    val cjkFontStatus: uniffi.pdfcraft_ffi.FfiCjkFontStatus
        get() = engine.getCjkFontStatus()

    val xfaStatus: uniffi.pdfcraft_ffi.FfiXfaStatus
        get() = engine.getXfaStatus(docId)

    fun compare(other: PdfDocument): List<uniffi.pdfcraft_ffi.FfiDocumentChange> {
        return engine.compareDocuments(docId, other.docId)
    }

    // MARK: - PDF Conversion, Security & Editing Workflows

    /**
     * Exports the reading-order text of pages to a .txt file.
     */
    fun exportText(outPath: String, pages: List<Int>? = null) {
        val pagesJson = if (pages != null) {
            "\"pages\": [" + pages.joinToString(",") { (it + 1).toString() } + "], "
        } else ""
        invokeTool("doc_export_text", """{"doc": $docId, ${pagesJson}"path": "$outPath"}""")
    }

    /**
     * Exports the document to Microsoft Word (.docx), PowerPoint (.pptx), HTML (.html), or RTF (.rtf),
     * determined by the target file extension.
     */
    fun exportOffice(outPath: String) {
        invokeTool("doc_export_office", """{"doc": $docId, "path": "$outPath"}""")
    }

    private fun parseToolOutput(outputJson: String): JSONObject {
        val trimmed = outputJson.trim()
        return try {
            if (trimmed.startsWith("[")) {
                val arr = JSONArray(trimmed)
                if (arr.length() > 0) arr.optJSONObject(0) ?: JSONObject() else JSONObject()
            } else if (trimmed.startsWith("{")) {
                JSONObject(trimmed)
            } else {
                JSONObject()
            }
        } catch (_: Exception) {
            JSONObject()
        }
    }

    /**
     * Lists added page content (images and text) created with addImage / addPageText.
     */
    fun listAddedContent(page: Int? = null): List<PdfAddedItem> {
        val res = invokeTool("content_list", """{"doc": $docId}""")
        val items = mutableListOf<PdfAddedItem>()
        if (res.outputJson.isNullOrBlank()) return items
        val result = parseToolOutput(res.outputJson)
        val itemsArray = result.optJSONArray("items") ?: return items
        for (i in 0 until itemsArray.length()) {
            val obj = itemsArray.optJSONObject(i) ?: continue
            val itemPage = (obj.optInt("page", 1) - 1)
            if (page != null && itemPage != page) continue
            val index = obj.optInt("index", 1)
            val type = obj.optString("type", "image")
            val text = if (obj.has("text")) obj.optString("text") else null
            val rectArr = obj.optJSONArray("rect")
            val rect = if (rectArr != null && rectArr.length() == 4) {
                RectF().apply {
                    left = rectArr.getDouble(0).toFloat()
                    top = rectArr.getDouble(1).toFloat()
                    right = rectArr.getDouble(2).toFloat()
                    bottom = rectArr.getDouble(3).toFloat()
                }
            } else {
                RectF().apply { left = 0f; top = 0f; right = 100f; bottom = 100f }
            }
            items.add(PdfAddedItem(itemPage, index, type, rect, text))
        }
        return items
    }

    /**
     * Updates an added content item's position, bounding rect, or text.
     */
    fun updateContent(page: Int, index: Int, rect: RectF? = null, text: String? = null) {
        val rectArg = if (rect != null) {
            ", \"rect\": [${rect.left}, ${rect.top}, ${rect.right}, ${rect.bottom}]"
        } else ""
        val textArg = if (text != null) {
            val escText = JSONObject.quote(text)
            ", \"text\": $escText"
        } else ""
        invokeTool("content_update", """{"doc": $docId, "page": ${page + 1}, "index": $index$rectArg$textArg}""")
        geometryCache.clear()
    }

    /**
     * Deletes an added content item from a page.
     */
    fun deleteContent(page: Int, index: Int) {
        invokeTool("content_delete", """{"doc": $docId, "page": ${page + 1}, "index": $index}""")
        geometryCache.clear()
    }

    /**
     * Adds an image file (PNG, JPEG, TIFF, GIF, BMP) as permanent page content.
     */
    fun addImage(page: Int, imagePath: String, rect: List<Float>? = null): PdfAddedItem? {
        val rectArg = if (rect != null && rect.size == 4) {
            ", \"rect\": [${rect[0]}, ${rect[1]}, ${rect[2]}, ${rect[3]}]"
        } else ""
        val res = invokeTool("page_add_image", """{"doc": $docId, "page": ${page + 1}, "path": "$imagePath"$rectArg}""")
        geometryCache.clear()
        val json = parseToolOutput(res.outputJson)
        val idx = json.optInt("index", 1)
        val rectArr = json.optJSONArray("rect")
        val r = if (rectArr != null && rectArr.length() == 4) {
            RectF().apply {
                left = rectArr.getDouble(0).toFloat()
                top = rectArr.getDouble(1).toFloat()
                right = rectArr.getDouble(2).toFloat()
                bottom = rectArr.getDouble(3).toFloat()
            }
        } else {
            RectF().apply { left = 50f; top = 50f; right = 250f; bottom = 250f }
        }
        return PdfAddedItem(page, idx, "image", r)
    }

    /**
     * Adds text as permanent page content.
     */
    fun addPageText(page: Int, text: String, atX: Float = 54f, atY: Float = 72f, fontSize: Float = 14f): PdfAddedItem? {
        val res = invokeTool("page_add_text", """{"doc": $docId, "page": ${page + 1}, "text": "$text", "at": [$atX, $atY], "size": $fontSize}""")
        geometryCache.clear()
        val json = parseToolOutput(res.outputJson)
        val idx = json.optInt("index", 1)
        val list = listAddedContent(page)
        return list.firstOrNull { it.index == idx } ?: PdfAddedItem(
            page,
            idx,
            "text",
            RectF().apply { left = atX; top = atY; right = atX + 200f; bottom = atY + fontSize + 10f },
            text
        )
    }

    /**
     * Extracts all embedded images on pages into an output directory.
     */
    fun extractAllImages(outFolder: String, pages: List<Int>? = null) {
        val pagesJson = if (pages != null) {
            "\"pages\": [" + pages.joinToString(",") { (it + 1).toString() } + "], "
        } else ""
        invokeTool("doc_export_all_images", """{"doc": $docId, ${pagesJson}"folder": "$outFolder"}""")
    }

    /**
     * Exports pages as image files (PNG, JPEG or TIFF) into an output directory.
     */
    fun exportImages(outDir: String, format: String = "png", dpi: Float = 150f) {
        invokeTool("doc_export_images", """{"doc": $docId, "folder": "$outDir", "format": "$format", "dpi": $dpi}""")
    }

    /**
     * Flattens interactive annotations, comments, and form fields into page content.
     */
    fun flatten(comments: Boolean = true, fields: Boolean = true) {
        invokeTool("doc_flatten", """{"doc": $docId, "comments": $comments, "fields": $fields}""")
        geometryCache.clear()
    }

    /**
     * Updates document metadata entries (Title, Author, Subject, Keywords).
     */
    fun setMetadata(
        title: String? = null,
        author: String? = null,
        subject: String? = null,
        keywords: String? = null
    ) {
        title?.let { invokeTool("doc_set_info", """{"doc": $docId, "key": "Title", "value": "$it"}""") }
        author?.let { invokeTool("doc_set_info", """{"doc": $docId, "key": "Author", "value": "$it"}""") }
        subject?.let { invokeTool("doc_set_info", """{"doc": $docId, "key": "Subject", "value": "$it"}""") }
        keywords?.let { invokeTool("doc_set_info", """{"doc": $docId, "key": "Keywords", "value": "$it"}""") }
    }

    /**
     * Protects the document with open and/or permissions password and restricts capabilities.
     */
    fun protect(
        openPassword: String? = null,
        permissionsPassword: String? = null,
        printing: String = "full",
        changes: String = "any-except-extract",
        copy: Boolean = true,
        accessibility: Boolean = true
    ) {
        val openArg = if (openPassword != null) "\"open_password\": \"$openPassword\", " else ""
        val permArg = if (permissionsPassword != null) {
            "\"permissions_password\": \"$permissionsPassword\", \"printing\": \"$printing\", \"changes\": \"$changes\", \"copy\": $copy, \"accessibility\": $accessibility, "
        } else ""
        invokeTool("doc_protect", """{"doc": $docId, $openArg$permArg"compatibility": "aes-256"}""")
    }

    /**
     * Removes password protection from the document.
     */
    fun unprotect() {
        invokeTool("doc_unprotect", """{"doc": $docId}""")
    }

    /**
     * Adds a text watermark across pages.
     */
    fun addWatermark(
        text: String,
        opacity: Float = 0.5f,
        rotation: Double = 45.0,
        fontSize: Float? = null,
        replace: Boolean = true
    ) {
        val fontArg = if (fontSize != null) "\"font_size\": $fontSize, " else ""
        invokeTool("doc_watermark", """{"doc": $docId, "text": "$text", "opacity": $opacity, "rotation": $rotation, ${fontArg}"replace": $replace}""")
        geometryCache.clear()
    }

    /**
     * Adds headers and/or footers with optional text in left, center, and right blocks.
     */
    fun addHeaderFooter(
        headerLeft: String? = null,
        headerCenter: String? = null,
        headerRight: String? = null,
        footerLeft: String? = null,
        footerCenter: String? = null,
        footerRight: String? = null,
        shrink: Boolean = false,
        replace: Boolean = true
    ) {
        val parts = mutableListOf<String>()
        headerLeft?.let { parts.add("\"header_left\": \"$it\"") }
        headerCenter?.let { parts.add("\"header_center\": \"$it\"") }
        headerRight?.let { parts.add("\"header_right\": \"$it\"") }
        footerLeft?.let { parts.add("\"footer_left\": \"$it\"") }
        footerCenter?.let { parts.add("\"footer_center\": \"$it\"") }
        footerRight?.let { parts.add("\"footer_right\": \"$it\"") }
        parts.add("\"shrink\": $shrink")
        parts.add("\"replace\": $replace")
        val args = parts.joinToString(", ")
        invokeTool("doc_header_footer", """{"doc": $docId, $args}""")
        geometryCache.clear()
    }

    /**
     * Extracts pages into a new PDF document.
     */
    fun extractPages(pages: List<Int>, outPath: String) {
        val pagesStr = "[" + pages.joinToString(",") { (it + 1).toString() } + "]"
        invokeTool("page_extract", """{"doc": $docId, "pages": $pagesStr, "out": "$outPath"}""")
    }
}
