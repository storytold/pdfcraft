import XCTest
@testable import PdfCraftKit

final class PdfCraftKitTests: XCTestCase {
    private let samplePdfString = """
    %PDF-1.7
    1 0 obj
    << /Type /Catalog /Pages 2 0 R >>
    endobj
    2 0 obj
    << /Type /Pages /Kids [3 0 R] /Count 1 >>
    endobj
    3 0 obj
    << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>
    endobj
    xref
    0 4
    0000000000 65535 f 
    0000000009 00000 n 
    0000000058 00000 n 
    0000000115 00000 n 
    trailer
    << /Size 4 /Root 1 0 R >>
    startxref
    193
    %%EOF
    """

    func testEngineInitAndOpenBytes() throws {
        let engine = PdfEngine()
        let data = samplePdfString.data(using: .utf8)!
        let docId = try engine.openBytes(name: "test.pdf", bytes: data, password: nil)

        let pageCount = try engine.pageCount(docId: docId)
        XCTAssertEqual(pageCount, 1)

        let geom = try engine.pageGeometry(docId: docId, page: 0)
        XCTAssertEqual(geom.width, 612.0)
        XCTAssertEqual(geom.height, 792.0)

        let req = PdfRenderRequest(page: 0, tile: nil, scale: 1.0, tag: 1, preview: false, readingColors: nil, hideComments: false)
        let renderResult = try engine.renderTile(docId: docId, request: req)
        XCTAssertEqual(renderResult.width, 612)
        XCTAssertEqual(renderResult.height, 792)
        XCTAssertNil(renderResult.error)

        try engine.close(docId: docId)
    }

    func testPdfDocumentLifecycleAndMetadata() throws {
        let data = samplePdfString.data(using: .utf8)!
        let doc = try PdfDocument(name: "sample.pdf", data: data)

        XCTAssertEqual(doc.pageCount, 1)
        let geom = try doc.geometry(for: 0)
        XCTAssertEqual(geom.width, 612.0)
        XCTAssertEqual(geom.height, 792.0)

        let info = try doc.info()
        XCTAssertNil(info.title)

        let outline = try doc.outline()
        XCTAssertEqual(outline.count, 0)

        let links = try doc.links(for: 0)
        XCTAssertEqual(links.count, 0)

        let text = try doc.text(for: 0)
        XCTAssertTrue(text.isEmpty)

        let fullPageImage = try doc.renderPage(page: 0, scale: 1.0)
        XCTAssertNotNil(fullPageImage)
        XCTAssertEqual(fullPageImage?.width, 612)
        XCTAssertEqual(fullPageImage?.height, 792)

        doc.close()
    }

    func testTileRendererComputeTile() {
        let geom = PdfPageGeometry(width: 612.0, height: 792.0, rotation: 0, label: "1")

        // 1x scale inside page
        let tile1 = PdfTileRenderer.computeTile(
            pageGeometry: geom,
            rect: CGRect(x: 0, y: 0, width: 256, height: 256),
            scale: 1.0
        )
        XCTAssertNotNil(tile1)
        XCTAssertEqual(tile1?.x, 0)
        XCTAssertEqual(tile1?.y, 0)
        XCTAssertEqual(tile1?.width, 256)
        XCTAssertEqual(tile1?.height, 256)

        // 2x scale
        let tile2 = PdfTileRenderer.computeTile(
            pageGeometry: geom,
            rect: CGRect(x: 100, y: 100, width: 256, height: 256),
            scale: 2.0
        )
        XCTAssertNotNil(tile2)
        XCTAssertEqual(tile2?.x, 200)
        XCTAssertEqual(tile2?.y, 200)
        XCTAssertEqual(tile2?.width, 512)
        XCTAssertEqual(tile2?.height, 512)

        // Clamping at right edge (page width 612 * 1.0 = 612)
        let tileEdge = PdfTileRenderer.computeTile(
            pageGeometry: geom,
            rect: CGRect(x: 500, y: 0, width: 200, height: 100),
            scale: 1.0
        )
        XCTAssertNotNil(tileEdge)
        XCTAssertEqual(tileEdge?.x, 500)
        XCTAssertEqual(tileEdge?.width, 112) // 612 - 500 = 112

        // Out of bounds completely
        let tileOutOfBounds = PdfTileRenderer.computeTile(
            pageGeometry: geom,
            rect: CGRect(x: 700, y: 900, width: 100, height: 100),
            scale: 1.0
        )
        XCTAssertNil(tileOutOfBounds)
    }

    func testConcurrentTileRendering() throws {
        let data = samplePdfString.data(using: .utf8)!
        let doc = try PdfDocument(name: "concurrent.pdf", data: data)

        let tiles = [
            PdfTile(x: 0, y: 0, width: 256, height: 256),
            PdfTile(x: 256, y: 0, width: 256, height: 256),
            PdfTile(x: 0, y: 256, width: 256, height: 256),
            PdfTile(x: 256, y: 256, width: 256, height: 256),
        ]

        DispatchQueue.concurrentPerform(iterations: tiles.count) { index in
            let tile = tiles[index]
            do {
                let img = try doc.renderTile(page: 0, tile: tile, scale: 1.0)
                XCTAssertNotNil(img)
                XCTAssertEqual(img?.width, Int(tile.width))
                XCTAssertEqual(img?.height, Int(tile.height))
            } catch {
                XCTFail("Concurrent render failed: \(error)")
            }
        }

        doc.close()
    }

    func testPdfTiledLayerProperties() {
        XCTAssertEqual(PdfTiledLayer.fadeDuration(), 0.0)
        let layer = PdfTiledLayer()
        XCTAssertEqual(layer.tileSize, CGSize(width: 512, height: 512))
        XCTAssertEqual(layer.levelsOfDetail, 5)
        XCTAssertEqual(layer.levelsOfDetailBias, 4)
    }

    func testViewportStateLayoutAndFitCalculations() {
        let geom = PdfPageGeometry(width: 612.0, height: 792.0, rotation: 0, label: "1")
        var state = PdfViewportState(layoutMode: .singlePage, fitMode: .fitPage, rotation: .rotation0, currentPage: 0, zoomScale: 1.0)

        // Fit width at 1224pt viewport
        let scaleW = state.scaleToFitWidth(geometry: geom, viewportWidth: 1224.0)
        XCTAssertEqual(scaleW, 2.0)

        // Fit page at 612x396 viewport
        let scalePage = state.scaleToFitPage(geometry: geom, viewportSize: CGSize(width: 612.0, height: 396.0))
        XCTAssertEqual(scalePage, 0.5)

        // Rotation 90 degrees transposes dimensions
        state.rotation = .rotation90
        let rotatedSize = state.effectiveSize(for: geom, scale: 1.0)
        XCTAssertEqual(rotatedSize.width, 792.0)
        XCTAssertEqual(rotatedSize.height, 612.0)

        // Zoom clamping
        let clampedLow = state.scaleToFitWidth(geometry: geom, viewportWidth: 10.0)
        XCTAssertEqual(clampedLow, PdfViewportState.minZoomScale)

        let clampedHigh = state.scaleToFitWidth(geometry: geom, viewportWidth: 50000.0)
        XCTAssertEqual(clampedHigh, PdfViewportState.maxZoomScale)
    }

    func testPrefetchPlanningAndTagCancellation() throws {
        let data = samplePdfString.data(using: .utf8)!
        let doc = try PdfDocument(name: "sample.pdf", data: data)

        let plan = doc.planPrefetch(currentPage: 0, direction: .forward, lookahead: 2)
        XCTAssertEqual(plan.count, 1) // Only 1 page in document
        XCTAssertEqual(plan.first?.page, 0)
        XCTAssertEqual(plan.first?.priority, .visible)

        let tag1 = doc.advanceGenerationAndCancelStale()
        XCTAssertGreaterThan(tag1, 0)
        let tag2 = doc.advanceGenerationAndCancelStale()
        XCTAssertGreaterThan(tag2, tag1)

        doc.close()
    }

    func testLaunchAndStaticInitAudit() throws {
        let startTime = CFAbsoluteTimeGetCurrent()
        let engine = PdfEngine()
        let initElapsed = CFAbsoluteTimeGetCurrent() - startTime
        // Static init and engine instantiation must take less than 50ms
        XCTAssertLessThan(initElapsed, 0.05, "Cold engine instantiation took \(initElapsed)s, exceeding 50ms budget")

        let data = samplePdfString.data(using: .utf8)!
        let openStart = CFAbsoluteTimeGetCurrent()
        let docId = try engine.openBytes(name: "audit.pdf", bytes: data, password: nil)
        let openElapsed = CFAbsoluteTimeGetCurrent() - openStart
        // Open time must take less than 100ms
        XCTAssertLessThan(openElapsed, 0.1, "Cold open took \(openElapsed)s, exceeding 100ms budget")

        try engine.close(docId: docId)
    }

    func testTextMarkupHighlightAndSelection() throws {
        let textPdfString = """
        %PDF-1.7
        1 0 obj
        << /Type /Catalog /Pages 2 0 R >>
        endobj
        2 0 obj
        << /Type /Pages /Kids [3 0 R] /Count 1 >>
        endobj
        3 0 obj
        << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>
        endobj
        4 0 obj
        << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>
        endobj
        5 0 obj
        << /Length 58 >>
        stream
        BT /F1 24 Tf 100 700 Td (Hello PdfCraft Mobile Text Markup) Tj ET
        endstream
        endobj
        xref
        0 6
        0000000000 65535 f 
        0000000009 00000 n 
        0000000058 00000 n 
        0000000115 00000 n 
        0000000243 00000 n 
        0000000318 00000 n 
        trailer
        << /Size 6 /Root 1 0 R >>
        startxref
        427
        %%EOF
        """
        let data = textPdfString.data(using: .utf8)!
        let doc = try PdfDocument(data: data)

        let matches = try doc.find(page: 0, query: "PdfCraft")
        XCTAssertFalse(matches.isEmpty)
        let m = matches[0]

        let rects = try doc.textSelectionRects(page: 0, startGlyph: Int(m.startGlyph), endGlyph: Int(m.endGlyph - 1))
        XCTAssertFalse(rects.isEmpty)

        let selText = try doc.textSelectionString(page: 0, startGlyph: Int(m.startGlyph), endGlyph: Int(m.endGlyph - 1))
        XCTAssertEqual(selText, "PdfCraft")

        try doc.addTextMarkup(
            page: 0,
            startGlyph: Int(m.startGlyph),
            endGlyph: Int(m.endGlyph - 1),
            kind: .highlight,
            colorRgb: 0xFFFF00
        )

        doc.close()
    }

    func testAnnotationLifecycleAndHitTesting() throws {
        let pdfData = samplePdfString.data(using: .utf8)!
        let doc = try PdfDocument(data: pdfData)

        // Hit test initially returns nil
        let hitNone = try doc.hitTestAnnotation(page: 0, point: CGPoint(x: 100, y: 100))
        XCTAssertNil(hitNone)

        // Add note annotation
        try doc.addNoteAnnotation(page: 0, at: CGPoint(x: 100, y: 100), contents: "Note comment", author: "Tester")
        let hitNote = try doc.hitTestAnnotation(page: 0, point: CGPoint(x: 102, y: 102))
        XCTAssertNotNil(hitNote)
        XCTAssertEqual(hitNote?.subtype, "Text")
        XCTAssertEqual(hitNote?.author, "Tester")

        // Add shape rectangle
        let shapeRect = FfiRect(x: 150, y: 150, width: 100, height: 80)
        try doc.addShapeAnnotation(page: 0, kind: .rectangle, rect: shapeRect, strokeColor: 0x00FF00, fillColor: 0xFFFF00)
        let hitRect = try doc.hitTestAnnotation(page: 0, point: CGPoint(x: 180, y: 180))
        XCTAssertNotNil(hitRect)
        XCTAssertEqual(hitRect?.subtype, "Square")

        // Move shape annotation
        try doc.moveAnnotation(page: 0, index: Int(hitRect!.index), dx: 50, dy: 50)

        // Delete shape annotation
        try doc.deleteAnnotation(page: 0, index: Int(hitRect!.index))

        doc.close()
    }

    func testInkDrawingPenUpAndEraser() throws {
        let pdfData = samplePdfString.data(using: .utf8)!
        let doc = try PdfDocument(data: pdfData)

        let strokePoints = [
            CGPoint(x: 100, y: 150),
            CGPoint(x: 130, y: 180),
            CGPoint(x: 160, y: 190),
            CGPoint(x: 200, y: 200),
        ]

        // 1. Commit stroke
        try doc.addInkStroke(page: 0, points: strokePoints, colorRgb: 0x0000FF, width: 3.0)

        // 2. Hit-test ink
        let hitInk = try doc.hitTestAnnotation(page: 0, point: CGPoint(x: 135, y: 185))
        XCTAssertNotNil(hitInk)
        XCTAssertEqual(hitInk?.subtype, "Ink")

        // 3. Erase ink along intersecting path
        let erased = try doc.eraseInk(page: 0, path: [CGPoint(x: 125, y: 175), CGPoint(x: 145, y: 195)], radius: 15.0)
        XCTAssertTrue(erased)

        doc.close()
    }

    func testCommentsListAndReviewState() throws {
        let pdfData = samplePdfString.data(using: .utf8)!
        let doc = try PdfDocument(data: pdfData)

        let initialComments = try doc.allComments()
        XCTAssertTrue(initialComments.isEmpty)

        // 1. Add sticky note
        try doc.addNoteAnnotation(page: 0, at: CGPoint(x: 100, y: 150), contents: "Initial note", author: "Reviewer")
        let comments = try doc.pageComments(page: 0)
        XCTAssertEqual(comments.count, 1)
        let root = comments[0]
        XCTAssertEqual(root.author, "Reviewer")

        // 2. Reply to comment
        try doc.replyToComment(page: 0, index: Int(root.index), text: "Reply from author", author: "Author")
        let afterReply = try doc.pageComments(page: 0)
        XCTAssertEqual(afterReply.count, 2)

        // 3. Set review state
        try doc.setCommentReviewState(page: 0, index: Int(root.index), state: .accepted, author: "Reviewer")
        let afterState = try doc.pageComments(page: 0)
        let statusItem = afterState.first { $0.reviewState != nil }
        XCTAssertNotNil(statusItem)
        XCTAssertEqual(statusItem?.reviewState, .accepted)

        doc.close()
    }

    func testAcroFormFillingAndSignatures() throws {
        let formPdfString = """
        %PDF-1.7
        1 0 obj
        << /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>
        endobj
        2 0 obj
        << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>
        endobj
        3 0 obj
        << /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R 7 0 R 8 0 R] >>
        endobj
        4 0 obj
        << /Fields [5 0 R 6 0 R 7 0 R 8 0 R] /DA (/Helv 0 Tf 0 g) >>
        endobj
        5 0 obj
        << /Type /Annot /Subtype /Widget /FT /Tx /T (fullname) /V (Ada) /DV (Ada) /Rect [50 700 250 720] /P 3 0 R >>
        endobj
        6 0 obj
        << /Type /Annot /Subtype /Widget /FT /Btn /T (subscribe) /V /Off /AS /Off /Rect [50 650 65 665] /P 3 0 R /AP << /N << /Yes 9 0 R /Off 9 0 R >> >> >>
        endobj
        7 0 obj
        << /Type /Annot /Subtype /Widget /FT /Ch /T (tier) /Ff 131072 /Opt [[(free) (Free)] [(pro) (Pro)]] /Rect [50 600 150 620] /P 3 0 R >>
        endobj
        8 0 obj
        << /Type /Annot /Subtype /Widget /FT /Sig /T (signature) /Rect [50 500 250 550] /P 3 0 R >>
        endobj
        9 0 obj
        << /Length 0 >>
        stream
        endstream
        endobj
        xref
        0 10
        0000000000 65535 f 
        0000000009 00000 n 
        0000000067 00000 n 
        0000000143 00000 n 
        0000000216 00000 n 
        0000000284 00000 n 
        0000000382 00000 n 
        0000000508 00000 n 
        0000000609 00000 n 
        0000000694 00000 n 
        trailer
        << /Size 10 /Root 1 0 R >>
        startxref
        742
        %%EOF
        """
        let data = formPdfString.data(using: .utf8)!
        let doc = try PdfDocument(data: data)

        // 1. Inspect form fields
        let fields = try doc.formFields(page: 0)
        XCTAssertEqual(fields.count, 4)

        let nameField = fields.first { $0.name == "fullname" }
        XCTAssertNotNil(nameField)
        XCTAssertEqual(nameField?.kind, .text)
        XCTAssertEqual(nameField?.value, ["Ada"])

        let checkField = fields.first { $0.name == "subscribe" }
        XCTAssertNotNil(checkField)
        XCTAssertEqual(checkField?.kind, .checkBox)

        let choiceField = fields.first { $0.name == "tier" }
        XCTAssertNotNil(choiceField)
        XCTAssertEqual(choiceField?.kind, .choice)
        XCTAssertEqual(choiceField?.options.count, 2)

        let sigField = fields.first { $0.name == "signature" }
        XCTAssertNotNil(sigField)
        XCTAssertEqual(sigField?.kind, .signature)

        // 2. Set text field
        try doc.setFormFieldText(name: "fullname", text: "Charles Babbage")
        let fieldsAfterText = try doc.formFields(page: 0)
        XCTAssertEqual(fieldsAfterText.first { $0.name == "fullname" }?.value, ["Charles Babbage"])

        // 3. Toggle check box
        let isChecked = try doc.toggleFormFieldCheck(name: "subscribe")
        XCTAssertTrue(isChecked)

        // 4. Set choice dropdown
        try doc.setFormFieldChoice(name: "tier", picked: ["pro"])
        let fieldsAfterChoice = try doc.formFields(page: 0)
        XCTAssertEqual(fieldsAfterChoice.first { $0.name == "tier" }?.value, ["pro"])

        // 5. Reset form
        try doc.resetForm()
        let fieldsAfterReset = try doc.formFields(page: 0)
        XCTAssertEqual(fieldsAfterReset.first { $0.name == "fullname" }?.value, ["Ada"])

        // 6. Sign signature field with drawn normalized strokes
        let drawnNorm: [[FfiPoint]] = [[
            FfiPoint(x: 0.1, y: 0.5),
            FfiPoint(x: 0.5, y: 0.8),
            FfiPoint(x: 0.9, y: 0.2),
        ]]
        try doc.signFormFieldDrawn(fieldName: "signature", normalizedStrokes: drawnNorm, author: "Ada")

        // 7. Add typed visual signature
        let sigRect = FfiRect(x: 50, y: 400, width: 150, height: 40)
        try doc.addTypedSignature(page: 0, text: "Ada Lovelace", rect: sigRect, author: "Ada")

        // 8. Render tile verify
        let image = try doc.renderPage(page: 0)
        XCTAssertNotNil(image)

        doc.close()
    }

    func testUndoRedoAndAtomicSave() throws {
        let data = samplePdfString.data(using: .utf8)!
        let doc = try PdfDocument(data: data)

        // 1. Initial pristine state
        XCTAssertFalse(doc.isDirty)
        XCTAssertNil(doc.canUndo)
        XCTAssertNil(doc.canRedo)

        // 2. Add an annotation
        try doc.addNoteAnnotation(page: 0, at: CGPoint(x: 100, y: 150), contents: "Note 1", author: "User")
        XCTAssertTrue(doc.isDirty)
        XCTAssertNotNil(doc.canUndo)
        XCTAssertNil(doc.canRedo)
        let commentsAfterAdd = try doc.pageComments(page: 0)
        XCTAssertEqual(commentsAfterAdd.count, 1)

        // 3. Undo
        let undone = try doc.undo()
        XCTAssertFalse(undone.isEmpty)
        XCTAssertNil(doc.canUndo)
        XCTAssertNotNil(doc.canRedo)
        let commentsAfterUndo = try doc.pageComments(page: 0)
        XCTAssertEqual(commentsAfterUndo.count, 0)

        // 4. Redo
        let redone = try doc.redo()
        XCTAssertFalse(redone.isEmpty)
        XCTAssertNotNil(doc.canUndo)
        let commentsAfterRedo = try doc.pageComments(page: 0)
        XCTAssertEqual(commentsAfterRedo.count, 1)

        // 5. Generate saved bytes
        let savedBytes = try doc.saveBytes()
        XCTAssertFalse(savedBytes.isEmpty)

        // 6. Atomic save to disk
        let tempDir = FileManager.default.temporaryDirectory
        let targetUrl = tempDir.appendingPathComponent("test_save_\(UUID().uuidString).pdf")
        try doc.save(to: targetUrl.path)
        XCTAssertTrue(FileManager.default.fileExists(atPath: targetUrl.path))
        XCTAssertFalse(doc.isDirty)

        // 7. Re-open saved file to verify persistence
        let reopenedDoc = try PdfDocument(fileURL: targetUrl)
        XCTAssertEqual(reopenedDoc.pageCount, 1)
        let reComments = try reopenedDoc.pageComments(page: 0)
        XCTAssertEqual(reComments.count, 1)
        reopenedDoc.close()

        // Clean up
        try? FileManager.default.removeItem(at: targetUrl)
        doc.close()

        // 8. Test autosave snapshot query on engine
        let engine = PdfEngine()
        let snapshots = try engine.autosaveSnapshots()
        XCTAssertNotNil(snapshots)
    }

    func testToolCatalogueAndSchemaParsing() throws {
        let tools = listTools()
        XCTAssertFalse(tools.isEmpty)

        // 1. Verify classifications
        let bespoke = tools.filter { $0.classification == .bespokeScreen }
        let schemaDriven = tools.filter { $0.classification == .schemaDriven }
        let notOnMobile = tools.filter { $0.classification == .notOnMobile }

        XCTAssertFalse(bespoke.isEmpty)
        XCTAssertFalse(schemaDriven.isEmpty)
        XCTAssertFalse(notOnMobile.isEmpty)

        // 2. NotOnMobile must have explicit reason
        for tool in notOnMobile {
            XCTAssertNotNil(tool.mobileReason)
            XCTAssertFalse(tool.mobileReason?.isEmpty ?? true)
        }

        // 3. Schema parsing on a schema-driven tool
        if let firstSchemaTool = schemaDriven.first {
            let fields = try parseToolSchema(inputSchemaJson: firstSchemaTool.inputSchemaJson)
            XCTAssertNotNil(fields)
        }
    }

    func testToolInvocationOnDocument() throws {
        let data = samplePdfString.data(using: .utf8)!
        let doc = try PdfDocument(data: data)

        // 1. Inspect initial geometry
        let geomBefore = try doc.geometry(for: 0)
        XCTAssertEqual(geomBefore.rotation, 0)

        // 2. Invoke page_rotate tool directly on the document
        let res = try doc.invokeTool(name: "page_rotate", args: ["degrees": 90])
        XCTAssertTrue(res.success)
        XCTAssertTrue(res.outputJson.contains("\"rotated\":1"))

        // 3. Verify rotation was applied to document model
        let geomAfter = try doc.geometry(for: 0)
        XCTAssertEqual(geomAfter.rotation, 90)

        // 4. Invoke doc_info
        let infoRes = try doc.invokeTool(name: "doc_info")
        XCTAssertTrue(infoRes.success)
        XCTAssertTrue(infoRes.outputJson.contains("\"pdf_version\""))

        doc.close()
    }

    private let samplePdfWithTextString = """
    %PDF-1.7
    1 0 obj
    << /Type /Catalog /Pages 2 0 R >>
    endobj
    2 0 obj
    << /Type /Pages /Kids [3 0 R] /Count 1 >>
    endobj
    3 0 obj
    << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>
    endobj
    4 0 obj
    << /Length 55 >>
    stream
    BT /F1 12 Tf 100 700 Td (CONFIDENTIAL SECRET AGENT) Tj ET
    endstream
    endobj
    5 0 obj
    << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>
    endobj
    xref
    0 6
    0000000000 65535 f 
    0000000009 00000 n 
    0000000058 00000 n 
    0000000115 00000 n 
    0000000234 00000 n 
    0000000339 00000 n 
    trailer
    << /Size 6 /Root 1 0 R >>
    startxref
    424
    %%EOF
    """

    func testRedactionMarkAuditAndApply() throws {
        let pdfData = samplePdfWithTextString.data(using: .utf8)!
        let doc = try PdfDocument(data: pdfData)

        // 1. Initial page text contains secret phrase
        let initialText = try doc.text(for: 0)
        XCTAssertTrue(initialText.contains("CONFIDENTIAL SECRET AGENT"))

        // 2. Initial audit should be clean (0 unapplied marks)
        let initialFindings = try doc.auditRedactions()
        let unapplied = initialFindings.filter { $0.kind == "unapplied-mark" }
        XCTAssertEqual(unapplied.count, 0)

        // 3. Mark the text for redaction
        let markedCount = try doc.markRedaction(text: "CONFIDENTIAL SECRET AGENT", overlayText: "SECRET")
        XCTAssertGreaterThanOrEqual(markedCount, 1)

        // 4. Audit should now detect the unapplied mark over the covered text
        let afterMarkFindings = try doc.auditRedactions()
        XCTAssertTrue(afterMarkFindings.contains { $0.kind == "unapplied-mark" })

        // 5. Apply redactions permanently
        let applyResult = try doc.applyRedactions()
        XCTAssertGreaterThanOrEqual(applyResult.appliedCount, 1)
        XCTAssertTrue(applyResult.auditPassed)
        XCTAssertTrue(applyResult.auditFindings.isEmpty)

        // 6. Audit after apply should have 0 unapplied marks
        let afterApplyFindings = try doc.auditRedactions()
        let unappliedAfter = afterApplyFindings.filter { $0.kind == "unapplied-mark" }
        XCTAssertEqual(unappliedAfter.count, 0)

        // 7. Text extraction should no longer contain the redacted phrase
        let afterText = try doc.text(for: 0)
        XCTAssertFalse(afterText.contains("CONFIDENTIAL SECRET AGENT"))

        doc.close()
    }

    func testDigitalIdCreationInspectionAndDocumentSigning() throws {
        // 1. Create a self-signed RSA 2048 Digital ID
        let p12Data = try PdfDocument.createSelfSignedDigitalId(
            name: "Ada Lovelace",
            password: "supersecret123",
            keyType: "rsa2048",
            organization: "Storytold",
            unit: "Security",
            email: "ada@storytold.internal",
            country: "US",
            validityYears: 5
        )
        XCTAssertFalse(p12Data.isEmpty)

        // 2. Inspect the Digital ID
        let idInfo = try PdfDocument.inspectDigitalId(p12Data: p12Data, password: "supersecret123")
        XCTAssertEqual(idInfo.commonName, "Ada Lovelace")
        XCTAssertEqual(idInfo.organization, "Storytold")
        XCTAssertEqual(idInfo.unit, "Security")
        XCTAssertEqual(idInfo.email, "ada@storytold.internal")
        XCTAssertEqual(idInfo.country, "US")
        XCTAssertTrue(idInfo.keyAlgorithm.contains("RSA 2048-bit"))
        XCTAssertFalse(idInfo.sha256Fingerprint.isEmpty)

        // 3. Open a sample PDF
        let pdfData = samplePdfWithTextString.data(using: .utf8)!
        let doc = try PdfDocument(data: pdfData)

        // 4. Initially 0 signatures
        let initialSigs = try doc.listDigitalSignatures()
        XCTAssertEqual(initialSigs.count, 0)

        // 5. Sign document with visible rect, reason, certify level, and script graphic
        let signOptions = FfiSignOptions(
            fieldName: nil,
            page: 0,
            rect: FfiRect(x: 50.0, y: 50.0, width: 220.0, height: 60.0),
            reason: "I certify and approve this document",
            location: "San Francisco, CA",
            contact: "ada@storytold.internal",
            certifyLevel: 1, // NoChanges
            graphicText: "Ada Lovelace",
            graphicStrokes: nil,
            outPath: nil
        )

        let signResult = try doc.sign(p12Data: p12Data, password: "supersecret123", options: signOptions)
        XCTAssertTrue(signResult.success)
        XCTAssertFalse(signResult.bytes.isEmpty)
        XCTAssertEqual(signResult.signerName, "Ada Lovelace")
        XCTAssertEqual(signResult.revision, 1)

        // 6. Verify signatures list on the document
        let sigsAfter = try doc.listDigitalSignatures()
        XCTAssertEqual(sigsAfter.count, 1)
        let sig = sigsAfter[0]
        XCTAssertTrue(sig.signed)
        XCTAssertEqual(sig.signer, "Ada Lovelace")
        XCTAssertEqual(sig.reason, "I certify and approve this document")
        XCTAssertEqual(sig.location, "San Francisco, CA")
        XCTAssertEqual(sig.certify, 1)
        XCTAssertTrue(sig.status == "valid" || sig.status == "unknown")

        doc.close()
    }

    func testOcrStatusAndSearchableTextLayer() throws {
        // 1. Query OCR status
        let status = try PdfDocument.ocrStatus()
        XCTAssertFalse(status.searchDirs.isEmpty)
        XCTAssertTrue(status.languages.contains { $0.contains("en: English") })

        // 2. Set custom OCR models dir to temp directory
        let tempDir = FileManager.default.temporaryDirectory.path
        try PdfDocument.setOcrModelsDir(tempDir)
        let updatedStatus = try PdfDocument.ocrStatus()
        XCTAssertTrue(updatedStatus.searchDirs.contains(tempDir))

        // 3. Open sample document
        let pdfData = samplePdfWithTextString.data(using: .utf8)!
        let doc = try PdfDocument(data: pdfData)

        // 4. Inject externally recognized words (e.g. from Apple Vision VNRecognizeTextRequest)
        let words = [
            FfiOcrWord(text: "SEARCHABLE", rect: FfiRect(x: 50.0, y: 150.0, width: 90.0, height: 18.0)),
            FfiOcrWord(text: "KEYWORD", rect: FfiRect(x: 145.0, y: 150.0, width: 75.0, height: 18.0)),
        ]
        let applied = try doc.applyOcrTextLayer(page: 0, words: words)
        XCTAssertEqual(applied, 2)

        // 5. Verify text extraction includes the injected words
        let extracted = try doc.text(for: 0)
        XCTAssertTrue(extracted.contains("SEARCHABLE"))
        XCTAssertTrue(extracted.contains("KEYWORD"))

        // 6. Verify find locates the injected words
        let matches = try doc.find(page: 0, query: "SEARCHABLE")
        XCTAssertEqual(matches.count, 1)

        // 7. Render page succeeds after OCR text layer injection
        let image = try doc.renderPage(page: 0)
        XCTAssertNotNil(image)

        doc.close()
    }

    func testAdvancedCapabilitiesFormJsPreflightCompareAndA11y() throws {
        // 1. Form JavaScript toggle & evaluation
        let engine = PdfEngine()
        XCTAssertTrue(try engine.isFormJavascriptEnabled())
        try engine.setFormJavascriptEnabled(enabled: false)
        XCTAssertFalse(try engine.isFormJavascriptEnabled())
        try engine.setFormJavascriptEnabled(enabled: true)
        XCTAssertTrue(try engine.isFormJavascriptEnabled())

        let pdfData = samplePdfWithTextString.data(using: .utf8)!
        let doc = try PdfDocument(engine: engine, data: pdfData)
        XCTAssertTrue(try doc.isFormJavascriptEnabled)
        try doc.setFormJavascriptEnabled(false)
        XCTAssertFalse(try doc.isFormJavascriptEnabled)
        try doc.setFormJavascriptEnabled(true)
        XCTAssertTrue(try doc.isFormJavascriptEnabled)

        let jsRes = try doc.runJavascript("console.println('Swift test JS'); 1 + 1;")
        XCTAssertTrue(jsRes.success)
        XCTAssertTrue(jsRes.output.contains("Swift test JS"))

        // 2. XFA Status
        let xfa = doc.xfaStatus
        XCTAssertNotNil(xfa)
        XCTAssertEqual(xfa?.kind, "none")

        // 3. Preflight & PDF/A
        let issues = try doc.verifyPdfA(level: "pdfa2b")
        XCTAssertFalse(issues.isEmpty)

        let remaining = try doc.convertToPdfA(level: "pdfa2b")
        let declared = try doc.declaredStandards()
        XCTAssertEqual(declared.pdfaPart, 2)
        XCTAssertEqual(declared.pdfaConformance, "B")

        // 4. Compare
        let otherPdfData = samplePdfWithTextString.data(using: .utf8)!
        let otherDoc = try PdfDocument(data: otherPdfData)
        let changes = try doc.compare(with: otherDoc)
        XCTAssertNotNil(changes)

        // 5. Accessibility full check (32 rules)
        let a11y = try doc.checkAccessibility()
        XCTAssertEqual(a11y.results.count, 32)
        XCTAssertEqual(a11y.passedCount + a11y.failedCount + a11y.manualCheckCount + a11y.skippedCount, 32)

        // 6. CJK fallback font status (MOB-6.6)
        let fontStatus = doc.cjkFontStatus
        XCTAssertEqual(fontStatus.craftFontsCount, 0, "mobile build default does not bundle heavy craft-fonts")
        XCTAssertFalse(fontStatus.hasJapaneseFont)
        XCTAssertFalse(fontStatus.hasChineseFont)

        otherDoc.close()
        doc.close()
    }
}

