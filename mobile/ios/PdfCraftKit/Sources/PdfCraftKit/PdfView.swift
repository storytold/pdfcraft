import SwiftUI

#if canImport(UIKit)
import UIKit

/// SwiftUI wrapper for displaying and interacting with a PDF page using native gestures and tiled rendering.
public struct PdfView: UIViewRepresentable {
    public let document: PdfDocument
    public let pageIndex: Int

    public init(document: PdfDocument, pageIndex: Int = 0) {
        self.document = document
        self.pageIndex = pageIndex
    }

    public func makeUIView(context: Context) -> PdfScrollView {
        let scrollView = PdfScrollView(document: document, pageIndex: pageIndex)
        return scrollView
    }

    public func updateUIView(_ uiView: PdfScrollView, context: Context) {
        uiView.update(document: document, pageIndex: pageIndex)
    }
}
#elseif canImport(AppKit)
import AppKit

/// macOS AppKit SwiftUI wrapper for displaying a PDF page.
public struct PdfView: NSViewRepresentable {
    public let document: PdfDocument
    public let pageIndex: Int

    public init(document: PdfDocument, pageIndex: Int = 0) {
        self.document = document
        self.pageIndex = pageIndex
    }

    public func makeNSView(context: Context) -> PdfPageView {
        let pageView = PdfPageView(document: document, pageIndex: pageIndex)
        return pageView
    }

    public func updateNSView(_ nsView: PdfPageView, context: Context) {
        nsView.update(document: document, pageIndex: pageIndex)
    }
}
#endif
