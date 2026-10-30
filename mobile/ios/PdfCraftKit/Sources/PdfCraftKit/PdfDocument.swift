import Foundation
import CoreGraphics

/// Thread-safe wrapper around a PdfEngine document.
public final class PdfDocument: @unchecked Sendable {
    public let engine: PdfEngine
    public let docId: UInt64
    public let name: String
    private let pageCountCache: UInt32
    private var geometryCache: [UInt32: PdfPageGeometry] = [:]
    private let lock = NSLock()
    private var isClosed = false

    /// Opens a PDF document from raw in-memory bytes.
    public init(engine: PdfEngine = PdfEngine(), name: String = "document.pdf", data: Data, password: String? = nil) throws {
        self.engine = engine
        self.name = name
        self.docId = try engine.openBytes(name: name, bytes: data, password: password)
        self.pageCountCache = try engine.pageCount(docId: self.docId)
    }

    /// Opens a PDF document from a sandbox file URL.
    public init(engine: PdfEngine = PdfEngine(), fileURL: URL, password: String? = nil) throws {
        self.engine = engine
        self.name = fileURL.lastPathComponent
        self.docId = try engine.openFile(pathStr: fileURL.path, password: password)
        self.pageCountCache = try engine.pageCount(docId: self.docId)
    }

    deinit {
        close()
    }

    /// Explicitly closes the document and releases native resources.
    public func close() {
        lock.lock()
        defer { lock.unlock() }
        if !isClosed {
            isClosed = true
            try? engine.close(docId: docId)
        }
    }

    /// Trims internal caches and unneeded memory in response to OS memory warnings.
    public func trimMemory() {
        try? engine.trimMemory()
    }

    /// Handles stepwise memory pressure degradation (Moderate: halves tile cache; Critical: drops all caches).
    public func handleMemoryPressure(level: FfiMemoryPressureLevel) {
        try? engine.handleMemoryPressure(level: level)
    }

    /// Number of pages in the document.
    public var pageCount: Int {
        Int(pageCountCache)
    }

    /// Retrieves geometry for a given zero-based page index.
    public func geometry(for page: Int) throws -> PdfPageGeometry {
        let p = UInt32(page)
        lock.lock()
        if let cached = geometryCache[p] {
            lock.unlock()
            return cached
        }
        lock.unlock()

        let geom = try engine.pageGeometry(docId: docId, page: p)
        lock.lock()
        geometryCache[p] = geom
        lock.unlock()
        return geom
    }

    /// Retrieves document metadata information (title, author, creation date, etc.).
    public func info() throws -> PdfDocInfo {
        try engine.docInfo(docId: docId)
    }

    /// Retrieves outline / table of contents tree.
    public func outline() throws -> [FfiOutlineItem] {
        try engine.outline(docId: docId)
    }

    /// Retrieves hyperlinks on a page.
    public func links(for page: Int) throws -> [FfiLink] {
        try engine.pageLinks(docId: docId, page: UInt32(page))
    }

    /// Extracts plain text from a page.
    public func text(for page: Int) throws -> String {
        try engine.pageText(docId: docId, page: UInt32(page))
    }

    /// Searches for query text on a specific page.
    public func find(page: Int, query: String, caseSensitive: Bool = false) throws -> [FfiFindMatch] {
        try engine.findText(docId: docId, page: UInt32(page), query: query, caseSensitive: caseSensitive)
    }

    /// Searches for query text across all pages in the document.
    public func findAll(query: String, caseSensitive: Bool = false) throws -> [FfiFindMatch] {
        var allMatches: [FfiFindMatch] = []
        for p in 0..<pageCount {
            let matches = try find(page: p, query: query, caseSensitive: caseSensitive)
            allMatches.append(contentsOf: matches)
        }
        return allMatches
    }

    /// Extracts view-space selection rectangles for a glyph range on a page.
    public func textSelectionRects(page: Int, startGlyph: Int, endGlyph: Int) throws -> [FfiRect] {
        try engine.textSelectionRects(docId: docId, page: UInt32(page), startGlyph: UInt32(startGlyph), endGlyph: UInt32(endGlyph))
    }

    /// Returns plain text for a glyph range on a page.
    public func textSelectionString(page: Int, startGlyph: Int, endGlyph: Int) throws -> String {
        try engine.textSelectionString(docId: docId, page: UInt32(page), startGlyph: UInt32(startGlyph), endGlyph: UInt32(endGlyph))
    }

    /// Finds the glyph nearest to a point in view coordinates.
    public func nearestGlyphAt(page: Int, point: CGPoint) throws -> Int? {
        let opt = try engine.nearestGlyphAt(docId: docId, page: UInt32(page), x: Float(point.x), y: Float(point.y))
        return opt.map { Int($0) }
    }

    /// Adds a text markup annotation (highlight, underline, strikeout, squiggly) covering the specified glyph selection range.
    public func addTextMarkup(
        page: Int,
        startGlyph: Int,
        endGlyph: Int,
        kind: FfiTextMarkupKind,
        colorRgb: UInt32? = nil,
        author: String? = nil,
        contents: String? = nil
    ) throws {
        try engine.addTextMarkupSelection(
            docId: docId,
            page: UInt32(page),
            startGlyph: UInt32(startGlyph),
            endGlyph: UInt32(endGlyph),
            kind: kind,
            colorRgb: colorRgb,
            author: author,
            contents: contents
        )
    }

    /// Adds a text markup annotation covering the given view-space rectangles.
    public func addTextMarkupRects(
        page: Int,
        rects: [FfiRect],
        kind: FfiTextMarkupKind,
        colorRgb: UInt32? = nil,
        author: String? = nil,
        contents: String? = nil
    ) throws {
        try engine.addTextMarkupRects(
            docId: docId,
            page: UInt32(page),
            rects: rects,
            kind: kind,
            colorRgb: colorRgb,
            author: author,
            contents: contents
        )
    }

    /// Hit-tests for an interactive annotation at point (x, y) in view coordinates.
    public func hitTestAnnotation(page: Int, point: CGPoint) throws -> FfiAnnotationSummary? {
        try engine.hitTestAnnotation(docId: docId, page: UInt32(page), x: Float(point.x), y: Float(point.y))
    }

    /// Moves an annotation on a page by delta view points.
    public func moveAnnotation(page: Int, index: Int, dx: Float, dy: Float) throws {
        try engine.moveAnnotation(docId: docId, page: UInt32(page), index: UInt32(index), dxView: dx, dyView: dy)
    }

    /// Resizes an annotation to a new bounding box in view coordinates.
    public func resizeAnnotation(page: Int, index: Int, newRect: FfiRect) throws {
        try engine.resizeAnnotation(docId: docId, page: UInt32(page), index: UInt32(index), newRectView: newRect)
    }

    /// Changes an annotation's style (stroke color, opacity, stroke width, and fill color).
    public func styleAnnotation(
        page: Int,
        index: Int,
        strokeColorRgb: UInt32? = nil,
        opacity: Float? = nil,
        strokeWidth: Float? = nil,
        fillColorRgb: UInt32? = nil
    ) throws {
        try engine.styleAnnotation(
            docId: docId,
            page: UInt32(page),
            index: UInt32(index),
            strokeColorRgb: strokeColorRgb,
            opacity: opacity,
            strokeWidth: strokeWidth,
            fillColorRgb: fillColorRgb
        )
    }

    /// Deletes an annotation from a page.
    public func deleteAnnotation(page: Int, index: Int) throws {
        try engine.deleteAnnotation(docId: docId, page: UInt32(page), index: UInt32(index))
    }

    /// Adds a sticky note annotation at a point in view coordinates.
    public func addNoteAnnotation(
        page: Int,
        at: CGPoint,
        contents: String,
        author: String? = nil,
        icon: String? = nil,
        colorRgb: UInt32? = nil
    ) throws {
        try engine.addNoteAnnotation(
            docId: docId,
            page: UInt32(page),
            at: FfiPoint(x: Float(at.x), y: Float(at.y)),
            contents: contents,
            author: author,
            icon: icon,
            colorRgb: colorRgb
        )
    }

    /// Adds a free text / text box annotation in view coordinates.
    public func addTextBoxAnnotation(
        page: Int,
        rect: FfiRect,
        contents: String,
        fontSize: Float = 14.0,
        author: String? = nil,
        colorRgb: UInt32? = nil
    ) throws {
        try engine.addTextBoxAnnotation(
            docId: docId,
            page: UInt32(page),
            rect: rect,
            contents: contents,
            fontSize: fontSize,
            author: author,
            colorRgb: colorRgb
        )
    }

    /// Adds a vector shape annotation (rectangle, oval, line, arrow) in view coordinates.
    public func addShapeAnnotation(
        page: Int,
        kind: FfiShapeKind,
        rect: FfiRect,
        strokeColor: UInt32? = nil,
        fillColor: UInt32? = nil,
        strokeWidth: Float? = nil,
        author: String? = nil
    ) throws {
        try engine.addShapeAnnotation(
            docId: docId,
            page: UInt32(page),
            kind: kind,
            rect: rect,
            strokeColor: strokeColor,
            fillColor: fillColor,
            strokeWidth: strokeWidth,
            author: author
        )
    }

    /// Adds a rubber stamp annotation in view coordinates.
    public func addStampAnnotation(
        page: Int,
        rect: FfiRect,
        stampName: String = "Approved",
        author: String? = nil
    ) throws {
        try engine.addStampAnnotation(
            docId: docId,
            page: UInt32(page),
            rect: rect,
            stampName: stampName,
            author: author
        )
    }

    /// Adds a multi-stroke ink drawing annotation in view coordinates.
    public func addInkAnnotation(
        page: Int,
        strokes: [[FfiPoint]],
        colorRgb: UInt32? = nil,
        strokeWidth: Float? = nil,
        opacity: Float? = nil,
        author: String? = nil
    ) throws {
        try engine.addInkAnnotation(
            docId: docId,
            page: UInt32(page),
            strokes: strokes,
            colorRgb: colorRgb,
            strokeWidth: strokeWidth,
            opacity: opacity,
            author: author
        )
    }

    /// Adds a single pen-up ink stroke as an annotation in view coordinates.
    public func addInkStroke(
        page: Int,
        points: [CGPoint],
        colorRgb: UInt32 = 0x0000FF,
        width: Float = 2.0
    ) throws {
        let ffiPoints = points.map { FfiPoint(x: Float($0.x), y: Float($0.y)) }
        let stroke = FfiInkStroke(page: UInt32(page), points: ffiPoints, colorRgb: colorRgb, width: width)
        try engine.addInkStroke(docId: docId, stroke: stroke)
    }

    /// Erases ink strokes intersecting a trajectory path on a page. Returns true if any ink was erased.
    @discardableResult
    public func eraseInk(
        page: Int,
        path: [CGPoint],
        radius: Float = 16.0
    ) throws -> Bool {
        let ffiPoints = path.map { FfiPoint(x: Float($0.x), y: Float($0.y)) }
        return try engine.eraseInkAt(docId: docId, page: UInt32(page), path: ffiPoints, radiusView: radius)
    }

    /// Returns all interactive comments on a page, including replies and status items.
    public func pageComments(page: Int) throws -> [FfiCommentItem] {
        try engine.pageComments(docId: docId, page: UInt32(page))
    }

    /// Returns all interactive comments across all pages in the document.
    public func allComments() throws -> [FfiCommentItem] {
        try engine.allComments(docId: docId)
    }

    /// Adds a threaded reply to a comment annotation.
    public func replyToComment(page: Int, index: Int, text: String, author: String) throws {
        try engine.replyToComment(docId: docId, page: UInt32(page), index: UInt32(index), text: text, author: author)
    }

    /// Sets the review state (e.g. Accepted, Rejected, Completed) on an annotation thread.
    public func setCommentReviewState(page: Int, index: Int, state: FfiReviewState, author: String) throws {
        try engine.setCommentReviewState(docId: docId, page: UInt32(page), index: UInt32(index), state: state, author: author)
    }

    /// Updates the text contents of a comment annotation.
    public func setCommentContents(page: Int, index: Int, text: String) throws {
        try engine.setCommentContents(docId: docId, page: UInt32(page), index: UInt32(index), text: text)
    }

    /// Toggles or sets "Mark with checkmark" review flag on an annotation.
    public func setCommentMarked(page: Int, index: Int, marked: Bool, author: String) throws {
        try engine.setCommentMarked(docId: docId, page: UInt32(page), index: UInt32(index), marked: marked, author: author)
    }


    /// Renders a specific tile or full page.
    public func renderTile(
        page: Int,
        tile: PdfTile?,
        scale: Float,
        tag: UInt64 = 0,
        preview: Bool = false,
        readingColors: FfiReadingColors? = nil,
        hideComments: Bool = false
    ) throws -> CGImage? {
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            return nil
        }
        lock.unlock()

        let req = PdfRenderRequest(
            page: UInt32(page),
            tile: tile,
            scale: scale,
            tag: tag,
            preview: preview,
            readingColors: readingColors,
            hideComments: hideComments
        )
        let result = try engine.renderTile(docId: docId, request: req)
        if let err = result.error {
            throw FfiError.Internal(reason: err)
        }
        return result.toCGImage()
    }

    /// Renders an entire page at the specified scale.
    public func renderPage(
        page: Int,
        scale: Float = 1.0,
        preview: Bool = false,
        readingColors: FfiReadingColors? = nil,
        hideComments: Bool = false
    ) throws -> CGImage? {
        try renderTile(
            page: page,
            tile: nil,
            scale: scale,
            tag: 0,
            preview: preview,
            readingColors: readingColors,
            hideComments: hideComments
        )
    }

    /// Fast thumbnail generation using low-detail preview mode.
    public func renderThumbnail(page: Int, maxDimension: Float = 160.0) throws -> CGImage? {
        let geom = try geometry(for: page)
        let maxSide = max(geom.width, geom.height)
        let scale = maxSide > 0 ? min(1.0, maxDimension / maxSide) : 0.2
        return try renderPage(page: page, scale: scale, preview: true)
    }

    /// Advances the render generation tag and cancels in-flight renders strictly older than the new tag.
    @discardableResult
    public func advanceGenerationAndCancelStale() -> UInt64 {
        let newTag = engine.nextRenderTag()
        engine.cancelRenders(olderThanTag: newTag)
        return newTag
    }

    /// Computes a prioritized list of pages to prefetch based on current page and scroll direction.
    public func planPrefetch(currentPage: Int, direction: FfiScrollDirection, lookahead: Int = 2) -> [FfiPrefetchPlan] {
        engine.planPrefetch(
            currentPage: UInt32(currentPage),
            totalPages: UInt32(pageCount),
            direction: direction,
            lookahead: UInt32(lookahead)
        )
    }

    // MARK: - AcroForm & Signatures

    /// Returns the list of interactive form fields in the document, optionally filtered to a specific page.
    public func formFields(page: Int? = nil) throws -> [FfiFormField] {
        try engine.formFields(docId: docId, page: page.map { UInt32($0) })
    }

    /// Sets the text value of a text form field.
    public func setFormFieldText(name: String, text: String) throws {
        try engine.setFormFieldText(docId: docId, name: name, text: text)
    }

    /// Sets the checked state of a check box form field.
    public func setFormFieldCheck(name: String, checked: Bool) throws {
        try engine.setFormFieldCheck(docId: docId, name: name, checked: checked)
    }

    /// Toggles a check box form field, returning the new checked state.
    @discardableResult
    public func toggleFormFieldCheck(name: String) throws -> Bool {
        try engine.toggleFormFieldCheck(docId: docId, name: name)
    }

    /// Selects an on-state for a radio button group (or nil to deselect).
    public func setFormFieldRadio(name: String, selectedOption: String?) throws {
        try engine.setFormFieldRadio(docId: docId, name: name, selectedOption: selectedOption)
    }

    /// Sets the selected export value(s) of a combo box or list box form field.
    public func setFormFieldChoice(name: String, picked: [String]) throws {
        try engine.setFormFieldChoice(docId: docId, name: name, picked: picked)
    }

    /// General setter for form fields by string representation according to field type.
    public func setFormFieldValue(name: String, value: String) throws {
        try engine.setFormFieldValue(docId: docId, name: name, value: value)
    }

    /// Resets named form fields back to their default values (or all fields if names is nil).
    public func resetForm(names: [String]? = nil) throws {
        try engine.resetForm(docId: docId, names: names)
    }

    /// Adds a freehand drawn visual signature annotation on a page.
    public func addDrawnSignature(
        page: Int,
        strokes: [[FfiPoint]],
        author: String? = nil
    ) throws {
        try engine.addDrawnSignature(
            docId: docId,
            page: UInt32(page),
            strokes: strokes,
            author: author
        )
    }

    /// Adds a typed visual script signature annotation in the given bounding box on a page.
    public func addTypedSignature(
        page: Int,
        text: String,
        rect: FfiRect,
        author: String? = nil
    ) throws {
        try engine.addTypedSignature(
            docId: docId,
            page: UInt32(page),
            text: text,
            rect: rect,
            author: author
        )
    }

    /// Signs a signature form field using freehand drawn normalized ink strokes (0.0..=1.0).
    public func signFormFieldDrawn(
        fieldName: String,
        normalizedStrokes: [[FfiPoint]],
        author: String? = nil
    ) throws {
        try engine.signFormFieldDrawn(
            docId: docId,
            fieldName: fieldName,
            normalizedStrokes: normalizedStrokes,
            author: author
        )
    }

    /// Signs a signature form field by rendering a typed name in the script typeface.
    public func signFormFieldTyped(
        fieldName: String,
        text: String,
        author: String? = nil
    ) throws {
        try engine.signFormFieldTyped(
            docId: docId,
            fieldName: fieldName,
            text: text,
            author: author
        )
    }

    // MARK: - Undo, Redo, and Saving (MOB-4.6)

    /// Returns the descriptive label of the next action that can be undone, or nil if undo stack is empty.
    public var canUndo: String? {
        (try? engine.canUndo(docId: docId)) ?? nil
    }

    /// Returns the descriptive label of the next action that can be redone, or nil if redo stack is empty.
    public var canRedo: String? {
        (try? engine.canRedo(docId: docId)) ?? nil
    }

    /// Invalidate internal document caches after edits.
    public func invalidateCaches() {
        lock.lock()
        geometryCache.removeAll()
        lock.unlock()
    }

    /// Undoes the last edit operation, returning the description of the undone action.
    @discardableResult
    public func undo() throws -> String {
        let res = try engine.undo(docId: docId)
        invalidateCaches()
        return res
    }

    /// Redoes the last undone edit operation, returning the description of the redone action.
    @discardableResult
    public func redo() throws -> String {
        let res = try engine.redo(docId: docId)
        invalidateCaches()
        return res
    }

    /// Whether the document currently has unsaved modifications.
    public var isDirty: Bool {
        (try? engine.isDirty(docId: docId)) ?? false
    }

    /// Generates full updated PDF bytes (incremental update with /ModDate stamped) suitable for saving.
    public func saveBytes() throws -> Data {
        let bytes = try engine.saveBytes(docId: docId)
        return Data(bytes)
    }

    /// Marks the document cleanly saved after writing to disk.
    public func markSaved(path: String? = nil) throws {
        try engine.markSaved(docId: docId, path: path)
    }

    /// Atomically saves the document to a target file path.
    public func save(to path: String) throws {
        try engine.saveToPath(docId: docId, targetPath: path)
    }

    // MARK: - Tool Catalogue & Invocation (Phase 5)

    /// Invokes an automation tool directly on this document.
    /// Injects `"doc": docId` into the argument dictionary if not already present.
    @discardableResult
    public func invokeTool(name: String, args: [String: Any] = [:]) throws -> FfiToolResult {
        var mergedArgs = args
        if mergedArgs["doc"] == nil {
            mergedArgs["doc"] = docId
        }
        let data = try JSONSerialization.data(withJSONObject: mergedArgs, options: [])
        guard let jsonString = String(data: data, encoding: .utf8) else {
            throw FfiError.InvalidArgs(reason: "failed to serialize tool arguments to JSON")
        }
        return try invokeTool(name: name, argsJson: jsonString)
    }

    /// Invokes an automation tool with raw JSON string on this document.
    @discardableResult
    public func invokeTool(name: String, argsJson: String) throws -> FfiToolResult {
        let res = try engine.invokeTool(name: name, argsJson: argsJson)
        if res.success {
            invalidateCaches()
        }
        return res
    }

    // MARK: - Redaction & Security Audit (Phase 6 / MOB-6.2)

    /// Marks a rectangle on a page for permanent redaction.
    /// Does not remove content until `applyRedactions` is called.
    public func markRedaction(page: Int, rect: FfiRect, overlayText: String? = nil) throws {
        try engine.markRedactionRect(docId: docId, page: UInt32(page), rect: rect, overlayText: overlayText)
    }

    /// Marks all occurrences of search text on this document for redaction.
    @discardableResult
    public func markRedaction(text: String, overlayText: String? = nil) throws -> UInt32 {
        try engine.markRedactionText(docId: docId, text: text, overlayText: overlayText)
    }

    /// Clears all pending redaction marks without applying them.
    public func clearRedactions() throws {
        try engine.clearRedactions(docId: docId)
    }

    /// Runs a security audit for unapplied redaction marks or faux redactions (opaque covers over text).
    public func auditRedactions() throws -> [FfiRedactionFinding] {
        try engine.auditRedactions(docId: docId)
    }

    /// Permanently applies all pending redaction marks, removes underlying content,
    /// verifies removal with the audit crate, and invalidates tile caches.
    @discardableResult
    public func applyRedactions() throws -> FfiRedactionApplyResult {
        let result = try engine.applyRedactions(docId: docId)
        invalidateCaches()
        return result
    }

    // MARK: - Digital Signatures & PKCS#12

    /// Lists digital signatures in the document along with their validation verdict.
    public func listDigitalSignatures() throws -> [FfiDigitalSignature] {
        try engine.listDigitalSignatures(docId: docId)
    }

    /// Cryptographically signs this document using a PKCS#12 digital identity (PAdES B-B).
    @discardableResult
    public func sign(
        p12Data: Data,
        password: String? = nil,
        options: FfiSignOptions
    ) throws -> FfiSignResult {
        let result = try engine.signDocumentP12(
            docId: docId,
            p12Bytes: p12Data,
            password: password,
            options: options
        )
        invalidateCaches()
        return result
    }

    /// Inspects a PKCS#12 (.p12 / .pfx) certificate bundle and extracts identity details.
    public static func inspectDigitalId(
        p12Data: Data,
        password: String? = nil,
        engine: Engine = PdfEngine()
    ) throws -> FfiDigitalIdInfo {
        try engine.inspectDigitalId(p12Bytes: p12Data, password: password)
    }

    /// Creates a self-signed digital ID certificate and private key, packaged as PKCS#12 (.p12) bytes.
    public static func createSelfSignedDigitalId(
        name: String,
        password: String,
        keyType: String = "rsa2048",
        organization: String? = nil,
        unit: String? = nil,
        email: String? = nil,
        country: String? = nil,
        validityYears: UInt32 = 5,
        engine: Engine = PdfEngine()
    ) throws -> Data {
        try engine.createSelfSignedDigitalId(
            name: name,
            password: password,
            keyType: keyType,
            organization: organization,
            unit: unit,
            email: email,
            country: country,
            validityYears: validityYears
        )
    }

    // MARK: - Scan & OCR (Phase 6 / MOB-6.4)

    /// Queries OCR subsystem status, search directories, and supported language codes.
    public static func ocrStatus(engine: Engine = PdfEngine()) throws -> FfiOcrStatus {
        try engine.ocrStatus()
    }

    /// Checks if text recognition models are installed and accessible.
    public static func isOcrAvailable(engine: Engine = PdfEngine()) -> Bool {
        engine.isOcrAvailable()
    }

    /// Sets or overrides the directory where OCR models (`text-detection.rten` and `text-recognition.rten`) are loaded from.
    public static func setOcrModelsDir(_ path: String, engine: Engine = PdfEngine()) throws {
        try engine.setOcrModelsDir(dirPath: path)
    }

    /// Runs text recognition on a document page and injects recognized words into an invisible searchable text layer.
    @discardableResult
    public func recognizeText(
        page: Int,
        dpi: Float? = nil,
        skipTextPages: Bool = true
    ) throws -> FfiOcrPageResult {
        let result = try engine.recognizePageText(
            docId: docId,
            page: UInt32(page),
            dpi: dpi,
            skipTextPages: skipTextPages
        )
        invalidateCaches()
        return result
    }

    /// Injects an invisible searchable text layer into a page using externally recognized words
    /// (e.g. from Apple Vision `VNRecognizeTextRequest`).
    @discardableResult
    public func applyOcrTextLayer(
        page: Int,
        words: [FfiOcrWord]
    ) throws -> UInt32 {
        let count = try engine.applyOcrTextLayer(
            docId: docId,
            page: UInt32(page),
            words: words
        )
        invalidateCaches()
        return count
    }

    // MARK: - Advanced Capabilities (Phase 6 / MOB-6.5)

    // Form JavaScript
    public var isFormJavascriptEnabled: Bool {
        get throws {
            try engine.isFormJavascriptEnabled()
        }
    }

    public func setFormJavascriptEnabled(_ enabled: Bool) throws {
        try engine.setFormJavascriptEnabled(enabled: enabled)
    }

    public static func setFormJavascriptEnabled(_ enabled: Bool, engine: Engine) throws {
        try engine.setFormJavascriptEnabled(enabled: enabled)
    }

    public static func isFormJavascriptEnabled(engine: Engine) throws -> Bool {
        try engine.isFormJavascriptEnabled()
    }

    @discardableResult
    public func runJavascript(_ script: String, target: String? = nil) throws -> FfiJsResult {
        let result = try engine.runFormJavascript(docId: docId, script: script, target: target)
        invalidateCaches()
        return result
    }

    public func takeJavascriptOutput() throws -> FfiJsOutput {
        try engine.takeFormJavascriptOutput(docId: docId)
    }

    // XFA
    public var xfaStatus: FfiXfaStatus? {
        try? engine.getXfaStatus(docId: docId)
    }

    public func exportXfaDatasets() throws -> String {
        try engine.exportXfaDatasets(docId: docId)
    }

    public func importXfaDatasets(_ xml: String) throws {
        try engine.importXfaDatasets(docId: docId, datasetsXml: xml)
        invalidateCaches()
    }

    // PDF/A Preflight
    public func declaredStandards() throws -> FfiPreflightDeclared {
        try engine.declaredStandards(docId: docId)
    }

    public func verifyPdfA(level: String = "pdfa2b") throws -> [FfiPreflightIssue] {
        try engine.verifyPdfa(docId: docId, level: level)
    }

    @discardableResult
    public func convertToPdfA(level: String = "pdfa2b") throws -> UInt32 {
        let remaining = try engine.convertToPdfa(docId: docId, level: level)
        invalidateCaches()
        return remaining
    }

    // Compare
    public func compare(with other: PdfDocument) throws -> [FfiDocumentChange] {
        try engine.compareDocuments(oldDocId: docId, newDocId: other.docId)
    }

    public func compareVisual(with other: PdfDocument, dpi: Float = 72.0) throws -> [FfiVisualDiff] {
        try engine.compareDocumentsVisual(oldDocId: docId, newDocId: other.docId, dpi: dpi)
    }

    @discardableResult
    public func markDifferences(from other: PdfDocument) throws -> UInt32 {
        let count = try engine.markDocumentDifferences(oldDocId: other.docId, newDocId: docId)
        invalidateCaches()
        return count
    }

    // Accessibility
    public func checkAccessibility() throws -> FfiA11yReport {
        try engine.checkAccessibility(docId: docId)
    }

    // MARK: - CJK Fallback Fonts (Phase 6 / MOB-6.6)

    /// Returns status of CJK fallback fonts embedded in this engine build.
    public var cjkFontStatus: FfiCjkFontStatus {
        engine.getCjkFontStatus()
    }

    /// Returns status of CJK fallback fonts for the given engine.
    public static func cjkFontStatus(engine: Engine = PdfEngine()) -> FfiCjkFontStatus {
        engine.getCjkFontStatus()
    }
}

