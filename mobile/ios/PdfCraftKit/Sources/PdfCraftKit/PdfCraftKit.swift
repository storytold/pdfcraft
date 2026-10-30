import Foundation
import CoreGraphics
#if canImport(UIKit)
import UIKit
#endif
#if canImport(AppKit)
import AppKit
#endif

// Re-export generated UniFFI types
public typealias PdfEngine = Engine
public typealias PdfDocInfo = FfiDocInfo
public typealias PdfPageGeometry = FfiPageGeometry
public typealias PdfRenderRequest = FfiRenderRequest
public typealias PdfRenderResult = FfiRenderResult
public typealias PdfTile = FfiTile

extension FfiRenderResult {
    /// Converts premultiplied RGBA8 raw bytes into a CoreGraphics CGImage.
    public func toCGImage() -> CGImage? {
        guard width > 0, height > 0, pixels.count == Int(width * height * 4) else {
            return nil
        }
        
        let colorSpace = CGColorSpaceCreateDeviceRGB()
        let bitmapInfo = CGBitmapInfo.byteOrder32Big.rawValue | CGImageAlphaInfo.premultipliedLast.rawValue
        
        guard let dataProvider = CGDataProvider(data: pixels as CFData) else {
            return nil
        }
        
        return CGImage(
            width: Int(width),
            height: Int(height),
            bitsPerComponent: 8,
            bitsPerPixel: 32,
            bytesPerRow: Int(width * 4),
            space: colorSpace,
            bitmapInfo: CGBitmapInfo(rawValue: bitmapInfo),
            provider: dataProvider,
            decode: nil,
            shouldInterpolate: true,
            intent: .defaultIntent
        )
    }

    #if canImport(UIKit)
    /// Converts premultiplied RGBA8 raw bytes into a native UIImage on iOS.
    public func toUIImage() -> UIImage? {
        guard let cgImage = toCGImage() else { return nil }
        return UIImage(cgImage: cgImage)
    }
    #endif

    #if canImport(AppKit)
    /// Converts premultiplied RGBA8 raw bytes into a native NSImage on macOS.
    public func toNSImage() -> NSImage? {
        guard let cgImage = toCGImage() else { return nil }
        return NSImage(cgImage: cgImage, size: NSSize(width: Int(width), height: Int(height)))
    }
    #endif
}
