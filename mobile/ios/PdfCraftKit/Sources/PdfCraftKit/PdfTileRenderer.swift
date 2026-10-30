import Foundation
import CoreGraphics

/// Helper for coordinate transformations between page points and device pixel tiles.
public struct PdfTileRenderer {
    /// Computes the tile rect in device pixels for a given dirty rect in page points.
    public static func computeTile(
        pageGeometry: PdfPageGeometry,
        rect: CGRect,
        scale: CGFloat
    ) -> PdfTile? {
        guard scale > 0 else { return nil }

        let totalPixelWidth = ceil(Double(pageGeometry.width) * Double(scale))
        let totalPixelHeight = ceil(Double(pageGeometry.height) * Double(scale))

        let rawX = floor(Double(rect.origin.x) * Double(scale))
        let rawY = floor(Double(rect.origin.y) * Double(scale))
        let rawW = ceil(Double(rect.size.width) * Double(scale))
        let rawH = ceil(Double(rect.size.height) * Double(scale))

        guard rawX < totalPixelWidth, rawY < totalPixelHeight else { return nil }
        guard rawW > 0, rawH > 0 else { return nil }

        let clampedX = max(0, rawX)
        let clampedY = max(0, rawY)
        let clampedW = min(rawW, totalPixelWidth - clampedX)
        let clampedH = min(rawH, totalPixelHeight - clampedY)

        guard clampedW > 0, clampedH > 0 else { return nil }

        return PdfTile(
            x: UInt32(clampedX),
            y: UInt32(clampedY),
            width: UInt32(clampedW),
            height: UInt32(clampedH)
        )
    }
}
