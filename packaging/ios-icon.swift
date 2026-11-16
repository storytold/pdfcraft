import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("error: \(message)\n".utf8))
    exit(EXIT_FAILURE)
}

guard CommandLine.arguments.count == 3 else {
    fail("usage: swift packaging/ios-icon.swift <macOS 1024 PNG> <iOS PNG>")
}
let input = URL(fileURLWithPath: CommandLine.arguments[1])
let output = URL(fileURLWithPath: CommandLine.arguments[2])
guard let source = CGImageSourceCreateWithURL(input as CFURL, nil),
      let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
      image.width == 1024, image.height == 1024 else {
    fail("expected the 1024x1024 macOS icon at \(input.path)")
}

// packaging/icons.sh centres an 824px body on macOS. iOS supplies its own mask.
guard let body = image.cropping(to: CGRect(x: 100, y: 100, width: 824, height: 824)),
      let context = CGContext(data: nil, width: 1024, height: 1024,
                              bitsPerComponent: 8, bytesPerRow: 4096,
                              space: CGColorSpaceCreateDeviceRGB(),
                              bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else {
    fail("could not create the opaque iOS icon canvas")
}
let canvas = CGRect(x: 0, y: 0, width: 1024, height: 1024)
context.setFillColor(red: 183.0 / 255, green: 2.0 / 255, blue: 40.0 / 255, alpha: 1)
context.fill(canvas)
context.interpolationQuality = .high
context.draw(body, in: canvas)
guard let icon = context.makeImage(),
      let destination = CGImageDestinationCreateWithURL(
        output as CFURL, UTType.png.identifier as CFString, 1, nil) else {
    fail("could not write \(output.path)")
}
CGImageDestinationAddImage(destination, icon, nil)
guard CGImageDestinationFinalize(destination) else {
    fail("PNG encoding failed for \(output.path)")
}
print("iOS icon written to \(output.path)")
