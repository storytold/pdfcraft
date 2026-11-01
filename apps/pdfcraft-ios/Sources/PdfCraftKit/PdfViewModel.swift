import CoreGraphics
import Foundation
import PdfCraftFFI

/// Milestone 1: open a PDF, view its pages, page through it. No editing yet — that's
/// `pdfcraft-engine`'s surface, out of scope for this crate until a later milestone (see
/// `crates/mobile-ffi/src/lib.rs`).
@MainActor
final class PdfViewModel: ObservableObject {
    enum State {
        case empty
        case loading
        case error(String)
        case loaded(document: MobileDocument, page: Int, pageCount: Int)
    }

    @Published private(set) var state: State = .empty
    @Published private(set) var currentPageImage: CGImage?

    func open(data: Data) {
        state = .loading
        currentPageImage = nil
        Task.detached(priority: .userInitiated) {
            do {
                let document = try MobileDocument.open(bytes: data, password: nil)
                let pageCount = Int(document.pageCount())
                await self.didLoad(document: document, page: 0, pageCount: pageCount)
            } catch {
                await self.fail(error)
            }
        }
    }

    func goToPage(_ page: Int) {
        guard case .loaded(let document, _, let pageCount) = state, page >= 0, page < pageCount else { return }
        didLoad(document: document, page: page, pageCount: pageCount)
    }

    private func didLoad(document: MobileDocument, page: Int, pageCount: Int) {
        state = .loaded(document: document, page: page, pageCount: pageCount)
        currentPageImage = nil
        Task.detached(priority: .userInitiated) {
            do {
                // The screen's native scale (2x/3x) times a 1.0 "fit" zoom; pinch-to-zoom is a
                // later milestone.
                let rendered = try document.renderPage(page: UInt32(page), scale: 2.0)
                let image = Self.cgImage(from: rendered)
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

    private nonisolated static func cgImage(from page: MobilePage) -> CGImage? {
        let data = Data(page.rgba)
        guard let provider = CGDataProvider(data: data as CFData) else { return nil }
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
}
