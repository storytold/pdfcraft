import Foundation
import CoreGraphics

/// Layout mode for presenting PDF pages.
public enum PdfLayoutMode: String, CaseIterable, Sendable {
    /// Single page presented at a time.
    case singlePage
    /// Continuous vertical scroll of pages.
    case continuousScroll
    /// Side-by-side two-page spread (ideal for tablets and landscape orientation).
    case twoPageSpread
}

/// Fitting mode for page sizing.
public enum PdfFitMode: Sendable {
    case fitWidth
    case fitPage
    case custom(scale: CGFloat)
}

/// Additional view rotation applied to displayed pages.
public enum PdfViewRotation: Int, CaseIterable, Sendable {
    case rotation0 = 0
    case rotation90 = 90
    case rotation180 = 180
    case rotation270 = 270

    public var degrees: CGFloat {
        CGFloat(rawValue)
    }

    public var radians: CGFloat {
        degrees * .pi / 180.0
    }

    public func rotatedClockwise() -> PdfViewRotation {
        switch self {
        case .rotation0: return .rotation90
        case .rotation90: return .rotation180
        case .rotation180: return .rotation270
        case .rotation270: return .rotation0
        }
    }

    public func rotatedCounterClockwise() -> PdfViewRotation {
        switch self {
        case .rotation0: return .rotation270
        case .rotation90: return .rotation0
        case .rotation180: return .rotation90
        case .rotation270: return .rotation180
        }
    }
}

/// Zoom and layout configuration model for mobile viewers.
public struct PdfViewportState: Sendable {
    public var layoutMode: PdfLayoutMode
    public var fitMode: PdfFitMode
    public var rotation: PdfViewRotation
    public var currentPage: Int
    public var zoomScale: CGFloat

    public static let minZoomScale: CGFloat = 0.25   // 25%
    public static let maxZoomScale: CGFloat = 16.0   // 1600%

    public init(
        layoutMode: PdfLayoutMode = .singlePage,
        fitMode: PdfFitMode = .fitPage,
        rotation: PdfViewRotation = .rotation0,
        currentPage: Int = 0,
        zoomScale: CGFloat = 1.0
    ) {
        self.layoutMode = layoutMode
        self.fitMode = fitMode
        self.rotation = rotation
        self.currentPage = currentPage
        self.zoomScale = zoomScale.clamped(to: Self.minZoomScale...Self.maxZoomScale)
    }

    /// Computes the effective size of a page given its natural geometry, view rotation, and scale.
    public func effectiveSize(for geometry: PdfPageGeometry, scale: CGFloat = 1.0) -> CGSize {
        let totalRotation = (Int(geometry.rotation) + rotation.rawValue) % 360
        let isTransposed = totalRotation == 90 || totalRotation == 270
        let w = isTransposed ? CGFloat(geometry.height) : CGFloat(geometry.width)
        let h = isTransposed ? CGFloat(geometry.width) : CGFloat(geometry.height)
        return CGSize(width: w * scale, height: h * scale)
    }

    /// Calculates scale to fit width in viewport.
    public func scaleToFitWidth(geometry: PdfPageGeometry, viewportWidth: CGFloat) -> CGFloat {
        guard viewportWidth > 0 else { return 1.0 }
        let naturalSize = effectiveSize(for: geometry, scale: 1.0)
        guard naturalSize.width > 0 else { return 1.0 }
        return (viewportWidth / naturalSize.width).clamped(to: Self.minZoomScale...Self.maxZoomScale)
    }

    /// Calculates scale to fit page in viewport.
    public func scaleToFitPage(geometry: PdfPageGeometry, viewportSize: CGSize) -> CGFloat {
        guard viewportSize.width > 0, viewportSize.height > 0 else { return 1.0 }
        let naturalSize = effectiveSize(for: geometry, scale: 1.0)
        guard naturalSize.width > 0, naturalSize.height > 0 else { return 1.0 }
        let scaleW = viewportSize.width / naturalSize.width
        let scaleH = viewportSize.height / naturalSize.height
        return min(scaleW, scaleH).clamped(to: Self.minZoomScale...Self.maxZoomScale)
    }
}

extension Comparable {
    func clamped(to limits: ClosedRange<Self>) -> Self {
        min(max(self, limits.lowerBound), limits.upperBound)
    }
}
