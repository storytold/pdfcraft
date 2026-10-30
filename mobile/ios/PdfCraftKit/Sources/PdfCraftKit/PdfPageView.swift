import Foundation
import CoreGraphics
import QuartzCore

#if canImport(UIKit)
import UIKit

/// Native UIView backed by PdfTiledLayer that renders PDF page tiles asynchronously.
public final class PdfPageView: UIView {
    public weak var document: PdfDocument?
    public var pageIndex: Int = 0
    public var pageGeometry: PdfPageGeometry?

    private var previewImageView: UIImageView?

    public override class var layerClass: AnyClass {
        PdfTiledLayer.self
    }

    public init(document: PdfDocument? = nil, pageIndex: Int = 0) {
        self.document = document
        self.pageIndex = pageIndex
        super.init(frame: .zero)
        setupView()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        setupView()
    }

    private func setupView() {
        backgroundColor = .white
        contentMode = .redraw
        layer.borderColor = UIColor.systemGray4.cgColor
        layer.borderWidth = 0.5
        layer.shadowColor = UIColor.black.cgColor
        layer.shadowOpacity = 0.1
        layer.shadowOffset = CGSize(width: 0, height: 2)
        layer.shadowRadius = 4

        let imageView = UIImageView()
        imageView.contentMode = .scaleAspectFit
        imageView.translatesAutoresizingMaskIntoConstraints = false
        addSubview(imageView)
        NSLayoutConstraint.activate([
            imageView.leadingAnchor.constraint(equalTo: leadingAnchor),
            imageView.trailingAnchor.constraint(equalTo: trailingAnchor),
            imageView.topAnchor.constraint(equalTo: topAnchor),
            imageView.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
        self.previewImageView = imageView

        updateGeometry()
    }

    public func update(document: PdfDocument, pageIndex: Int) {
        self.document = document
        self.pageIndex = pageIndex
        updateGeometry()
        loadPreview()
        setNeedsDisplay()
    }

    public func setPreviewImage(_ image: UIImage?) {
        previewImageView?.image = image
    }

    private func updateGeometry() {
        guard let document = document,
              let geom = try? document.geometry(for: pageIndex) else {
            return
        }
        self.pageGeometry = geom
        bounds = CGRect(x: 0, y: 0, width: CGFloat(geom.width), height: CGFloat(geom.height))
        isAccessibilityElement = true
        accessibilityLabel = "PDF Page \(pageIndex + 1) of \(document.pageCount)"
        accessibilityTraits = .image
    }

    private func loadPreview() {
        guard let document = document else { return }
        let page = self.pageIndex
        Task.detached(priority: .userInitiated) { [weak self] in
            guard let cg = try? document.renderPage(page: page, scale: 0.5, preview: true) else { return }
            let uiImage = UIImage(cgImage: cg)
            await MainActor.run {
                if self?.pageIndex == page {
                    self?.setPreviewImage(uiImage)
                }
            }
        }
    }

    public var readingColors: FfiReadingColors? {
        didSet {
            loadPreview()
            setNeedsDisplay()
        }
    }
    public var hideComments: Bool = false {
        didSet {
            loadPreview()
            setNeedsDisplay()
        }
    }
    public var searchHighlights: [FfiRect] = [] {
        didSet { setNeedsDisplay() }
    }
    public var selectedTextRects: [FfiRect] = [] {
        didSet { setNeedsDisplay() }
    }
    public var selectionGlyphRange: (start: Int, end: Int)?

    public func selectText(startGlyph: Int, endGlyph: Int) {
        guard let document = document else { return }
        self.selectionGlyphRange = (startGlyph, endGlyph)
        if let rects = try? document.textSelectionRects(page: pageIndex, startGlyph: startGlyph, endGlyph: endGlyph) {
            self.selectedTextRects = rects
        }
    }

    public func clearTextSelection() {
        self.selectionGlyphRange = nil
        self.selectedTextRects = []
    }

    /// Applies markup annotation (highlight, underline, strikeout, squiggly) to the active selection.
    public func markupSelection(
        kind: FfiTextMarkupKind,
        colorRgb: UInt32? = nil,
        author: String? = nil,
        contents: String? = nil
    ) throws {
        guard let document = document else { return }
        if let range = selectionGlyphRange {
            try document.addTextMarkup(
                page: pageIndex,
                startGlyph: range.start,
                endGlyph: range.end,
                kind: kind,
                colorRgb: colorRgb,
                author: author,
                contents: contents
            )
        } else if !selectedTextRects.isEmpty {
            try document.addTextMarkupRects(
                page: pageIndex,
                rects: selectedTextRects,
                kind: kind,
                colorRgb: colorRgb,
                author: author,
                contents: contents
            )
        }
        clearTextSelection()
        loadPreview()
        setNeedsDisplay()
    }

    public var selectedAnnotation: FfiAnnotationSummary? {
        didSet { setNeedsDisplay() }
    }

    /// Hit-tests at point and selects the annotation if found.
    @discardableResult
    public func selectAnnotation(at point: CGPoint) -> FfiAnnotationSummary? {
        guard let document = document else { return nil }
        let annot = try? document.hitTestAnnotation(page: pageIndex, point: point)
        self.selectedAnnotation = annot
        return annot
    }

    public func clearAnnotationSelection() {
        self.selectedAnnotation = nil
    }

    /// Moves the currently selected annotation by delta points.
    public func moveSelectedAnnotation(dx: Float, dy: Float) throws {
        guard let document = document, let annot = selectedAnnotation else { return }
        try document.moveAnnotation(page: pageIndex, index: Int(annot.index), dx: dx, dy: dy)
        self.selectedAnnotation = try? document.hitTestAnnotation(
            page: pageIndex,
            point: CGPoint(x: CGFloat(annot.rect.x + dx + annot.rect.width / 2), y: CGFloat(annot.rect.y + dy + annot.rect.height / 2))
        )
        loadPreview()
        setNeedsDisplay()
    }

    /// Deletes the currently selected annotation.
    public func deleteSelectedAnnotation() throws {
        guard let document = document, let annot = selectedAnnotation else { return }
        try document.deleteAnnotation(page: pageIndex, index: Int(annot.index))
        clearAnnotationSelection()
        loadPreview()
        setNeedsDisplay()
    }

    // MARK: - Ink & Stylus Drawing Mode
    public var isInkMode: Bool = false {
        didSet { isUserInteractionEnabled = true }
    }
    public var isEraserMode: Bool = false {
        didSet { isUserInteractionEnabled = true }
    }
    public var inkColorRgb: UInt32 = 0x0000FF // Blue
    public var inkStrokeWidth: Float = 2.5
    public var liveStrokePoints: [CGPoint] = []

    // Touches handling for live pencil / stylus strokes (Apple Pencil coalesced & predicted touches)
    public override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard isInkMode || isEraserMode, let touch = touches.first else {
            super.touchesBegan(touches, with: event)
            return
        }
        let loc = touch.location(in: self)
        if isEraserMode {
            _ = try? document?.eraseInk(page: pageIndex, path: [loc], radius: 20.0)
            loadPreview()
            setNeedsDisplay()
        } else {
            liveStrokePoints = [loc]
            setNeedsDisplay()
        }
    }

    public override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard isInkMode || isEraserMode, let touch = touches.first else {
            super.touchesMoved(touches, with: event)
            return
        }
        let coalesced = event?.coalescedTouches(for: touch) ?? [touch]
        let points = coalesced.map { $0.location(in: self) }

        if isEraserMode {
            _ = try? document?.eraseInk(page: pageIndex, path: points, radius: 20.0)
            loadPreview()
            setNeedsDisplay()
        } else {
            liveStrokePoints.append(contentsOf: points)
            setNeedsDisplay()
        }
    }

    public override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard isInkMode || isEraserMode else {
            super.touchesEnded(touches, with: event)
            return
        }
        if isEraserMode {
            loadPreview()
            setNeedsDisplay()
            return
        }
        if liveStrokePoints.count >= 2 {
            try? document?.addInkStroke(
                page: pageIndex,
                points: liveStrokePoints,
                colorRgb: inkColorRgb,
                width: inkStrokeWidth
            )
        }
        liveStrokePoints = []
        loadPreview()
        setNeedsDisplay()
    }

    public override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        liveStrokePoints = []
        setNeedsDisplay()
        super.touchesCancelled(touches, with: event)
    }

    public override func draw(_ rect: CGRect) {
        guard let document = document,
              let geom = self.pageGeometry ?? (try? document.geometry(for: pageIndex)),
              let ctx = UIGraphicsGetCurrentContext() else {
            return
        }

        // Determine effective scale from graphics context CTM
        let scale = max(abs(ctx.ctm.a), abs(ctx.ctm.d))
        guard scale > 0 else { return }

        guard let tile = PdfTileRenderer.computeTile(pageGeometry: geom, rect: rect, scale: scale) else {
            return
        }

        do {
            if let cgImage = try document.renderTile(
                page: pageIndex,
                tile: tile,
                scale: Float(scale),
                readingColors: readingColors,
                hideComments: hideComments
            ) {
                // In UIKit draw(_:), UIImage.draw(in:) properly respects UIKit coordinate orientation
                let image = UIImage(cgImage: cgImage)
                image.draw(in: rect)
            }
        } catch {
            // Render error handled gracefully: background preview image remains visible
        }

        // Draw search highlights overlay (translucent amber)
        if !searchHighlights.isEmpty {
            ctx.setFillColor(UIColor.systemYellow.withAlphaComponent(0.4).cgColor)
            for hl in searchHighlights {
                let r = CGRect(x: CGFloat(hl.x), y: CGFloat(hl.y), width: CGFloat(hl.width), height: CGFloat(hl.height))
                if rect.intersects(r) {
                    ctx.fill(r)
                }
            }
        }

        // Draw text selection overlay (translucent blue)
        if !selectedTextRects.isEmpty {
            ctx.setFillColor(UIColor.systemBlue.withAlphaComponent(0.35).cgColor)
            for sel in selectedTextRects {
                let r = CGRect(x: CGFloat(sel.x), y: CGFloat(sel.y), width: CGFloat(sel.width), height: CGFloat(sel.height))
                if rect.intersects(r) {
                    ctx.fill(r)
                }
            }
        }

        // Draw selected annotation selection rectangle and handles (dashed border)
        if let annot = selectedAnnotation {
            let r = CGRect(x: CGFloat(annot.rect.x), y: CGFloat(annot.rect.y), width: CGFloat(annot.rect.width), height: CGFloat(annot.rect.height))
            if rect.intersects(r.insetBy(dx: -8, dy: -8)) {
                ctx.saveGState()
                ctx.setStrokeColor(UIColor.systemBlue.cgColor)
                ctx.setLineWidth(1.5)
                ctx.setLineDash(phase: 0, lengths: [4.0, 2.0])
                ctx.stroke(r)
                // Draw 4 corner handles
                ctx.setFillColor(UIColor.white.cgColor)
                let handleSize: CGFloat = 8.0
                let corners = [
                    CGPoint(x: r.minX - handleSize / 2, y: r.minY - handleSize / 2),
                    CGPoint(x: r.maxX - handleSize / 2, y: r.minY - handleSize / 2),
                    CGPoint(x: r.minX - handleSize / 2, y: r.maxY - handleSize / 2),
                    CGPoint(x: r.maxX - handleSize / 2, y: r.maxY - handleSize / 2),
                ]
                for c in corners {
                    let hr = CGRect(origin: c, size: CGSize(width: handleSize, height: handleSize))
                    ctx.fill(hr)
                    ctx.setLineDash(phase: 0, lengths: [])
                    ctx.stroke(hr)
                }
                ctx.restoreGState()
            }
        }

        // Draw live ink drawing stroke overlay (smooth round path)
        if liveStrokePoints.count >= 2 {
            ctx.saveGState()
            let r = CGFloat((inkColorRgb >> 16) & 0xFF) / 255.0
            let g = CGFloat((inkColorRgb >> 8) & 0xFF) / 255.0
            let b = CGFloat(inkColorRgb & 0xFF) / 255.0
            ctx.setStrokeColor(UIColor(red: r, green: g, blue: b, alpha: 1.0).cgColor)
            ctx.setLineWidth(CGFloat(inkStrokeWidth))
            ctx.setLineCap(.round)
            ctx.setLineJoin(.round)
            let path = CGMutablePath()
            path.move(to: liveStrokePoints[0])
            for pt in liveStrokePoints.dropFirst() {
                path.addLine(to: pt)
            }
            ctx.addPath(path)
            ctx.strokePath()
            ctx.restoreGState()
        }
    }
}
#elseif canImport(AppKit)
import AppKit

/// macOS AppKit fallback view for PdfPageView.
public final class PdfPageView: NSView {
    public weak var document: PdfDocument?
    public var pageIndex: Int = 0
    public var pageGeometry: PdfPageGeometry?

    public init(document: PdfDocument? = nil, pageIndex: Int = 0) {
        self.document = document
        self.pageIndex = pageIndex
        super.init(frame: .zero)
        wantsLayer = true
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        wantsLayer = true
    }

    public override func makeBackingLayer() -> CALayer {
        PdfTiledLayer()
    }

    public func update(document: PdfDocument, pageIndex: Int) {
        self.document = document
        self.pageIndex = pageIndex
        if let geom = try? document.geometry(for: pageIndex) {
            self.pageGeometry = geom
            setFrameSize(NSSize(width: CGFloat(geom.width), height: CGFloat(geom.height)))
        }
        needsDisplay = true
    }
}
#endif
