import CoreGraphics
import Foundation
import PdfCraftFFI

/// A reader: open a document, view its pages, page through it, find text, jump through the
/// outline. No editing yet — that's `pdfcraft-engine`'s surface, out of scope for this crate
/// until a later milestone (see `crates/mobile-ffi/src/lib.rs`).
@MainActor
final class PdfViewModel: ObservableObject {
    enum State {
        case empty
        case loading
        case error(String)
        case loaded(document: MobileDocument)
    }

    /// Device pixels per PDF point for both page rendering and search highlight rects — they
    /// must match, or a match's rects won't line up with the rendered page.
    private static let renderScale: Float = 2.0

    @Published private(set) var state: State = .empty
    @Published private(set) var page = 0
    @Published private(set) var pageCount = 0
    @Published private(set) var title: String?
    @Published private(set) var outline: [OutlineEntry] = []
    @Published private(set) var currentPageImage: CGImage?
    @Published private(set) var matches: [SearchMatch] = []
    @Published private(set) var matchIndex: Int?

    var activeMatch: SearchMatch? {
        guard let matchIndex, matches.indices.contains(matchIndex) else { return nil }
        return matches[matchIndex]
    }

    func open(data: Data) {
        state = .loading
        resetDerivedState()
        Task.detached(priority: .userInitiated) {
            do {
                let document = try MobileDocument.open(bytes: data, password: nil)
                let title = document.title()
                let outline = document.outline()
                let pageCount = Int(document.pageCount())
                await self.didOpen(document, title: title, outline: outline, pageCount: pageCount)
            } catch {
                await self.fail(error)
            }
        }
    }

    func goToPage(_ target: Int) {
        guard case .loaded(let document) = state, target >= 0, target < pageCount else { return }
        page = target
        render(document: document)
    }

    /// Empty `query` clears the current search instead of running one.
    func search(_ query: String) {
        guard case .loaded(let document) = state else { return }
        guard !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            matches = []
            matchIndex = nil
            return
        }
        Task.detached(priority: .userInitiated) {
            let found = document.search(query: query, caseSensitive: false, wholeWords: false, scale: Self.renderScale)
            await self.didSearch(found, document: document)
        }
    }

    func nextMatch() { stepMatch(1) }

    func previousMatch() { stepMatch(-1) }

    private func stepMatch(_ delta: Int) {
        guard case .loaded(let document) = state, !matches.isEmpty else { return }
        let count = matches.count
        let next = (((matchIndex ?? 0) + delta) % count + count) % count
        matchIndex = next
        page = Int(matches[next].page)
        render(document: document)
    }

    private func didOpen(_ document: MobileDocument, title: String?, outline: [OutlineEntry], pageCount: Int) {
        self.title = title
        self.outline = outline
        self.pageCount = pageCount
        page = 0
        state = .loaded(document: document)
        render(document: document)
    }

    private func didSearch(_ found: [SearchMatch], document: MobileDocument) {
        matches = found
        guard let first = found.first else {
            matchIndex = nil
            return
        }
        matchIndex = 0
        page = Int(first.page)
        render(document: document)
    }

    private func render(document: MobileDocument) {
        currentPageImage = nil
        let page = self.page
        let highlights = activeMatch?.page == UInt32(page) ? (activeMatch?.rects ?? []) : []
        Task.detached(priority: .userInitiated) {
            do {
                let rendered = try document.renderPage(page: UInt32(page), scale: Self.renderScale)
                let image = Self.cgImage(from: rendered, highlights: highlights)
                await self.didRender(image)
            } catch {
                await self.fail(error)
            }
        }
    }

    private func didRender(_ image: CGImage?) {
        currentPageImage = image
    }

    private func fail(_ error: Error) {
        state = .error(error.localizedDescription)
    }

    private func resetDerivedState() {
        currentPageImage = nil
        matches = []
        matchIndex = nil
        title = nil
        outline = []
        pageCount = 0
        page = 0
    }

    private nonisolated static func cgImage(from page: MobilePage, highlights: [Rect]) -> CGImage? {
        let bytes = highlights.isEmpty ? page.rgba : highlighted(page.rgba, width: Int(page.width), height: Int(page.height), rects: highlights)
        guard let provider = CGDataProvider(data: bytes as CFData) else { return nil }
        let bitmapInfo = CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue)
        return CGImage(
            width: Int(page.width),
            height: Int(page.height),
            bitsPerComponent: 8,
            bitsPerPixel: 32,
            bytesPerRow: Int(page.width) * 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: bitmapInfo,
            provider: provider,
            decode: nil,
            shouldInterpolate: true,
            intent: .defaultIntent
        )
    }

    /// Blends a translucent highlight directly into the premultiplied RGBA bytes, in the same
    /// top-left-origin, y-down pixel space `rects` already use — simpler and less error-prone
    /// than drawing into a `CGContext`, whose default bottom-left origin would need every rect
    /// flipped first.
    private nonisolated static func highlighted(_ rgba: Data, width: Int, height: Int, rects: [Rect]) -> Data {
        var bytes = rgba
        let alpha = 0.35
        let (highlightR, highlightG, highlightB) = (255.0, 213.0, 0.0)
        for rect in rects {
            let x0 = max(0, Int(rect.x0.rounded(.down)))
            let y0 = max(0, Int(rect.y0.rounded(.down)))
            let x1 = min(width, Int(rect.x1.rounded(.up)))
            let y1 = min(height, Int(rect.y1.rounded(.up)))
            guard x1 > x0, y1 > y0 else { continue }
            for y in y0..<y1 {
                let rowStart = y * width * 4
                for x in x0..<x1 {
                    let i = rowStart + x * 4
                    guard i + 3 < bytes.count else { continue }
                    bytes[i] = UInt8((Double(bytes[i]) * (1 - alpha) + highlightR * alpha).rounded())
                    bytes[i + 1] = UInt8((Double(bytes[i + 1]) * (1 - alpha) + highlightG * alpha).rounded())
                    bytes[i + 2] = UInt8((Double(bytes[i + 2]) * (1 - alpha) + highlightB * alpha).rounded())
                }
            }
        }
        return bytes
    }
}
