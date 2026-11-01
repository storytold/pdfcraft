// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "PdfCraftKit",
    platforms: [.iOS(.v16)],
    products: [
        .library(name: "PdfCraftKit", targets: ["PdfCraftKit"])
    ],
    targets: [
        // Written by `cargo xtask mobile ios` (xtask/src/mobile.rs); git-ignored, not committed.
        .binaryTarget(name: "pdfcraft_mobile_ffiFFI", path: "Frameworks/pdfcraft_mobile_ffiFFI.xcframework"),
        // The pure-Swift UniFFI wrapper over the C FFI above, also written by that command.
        .target(name: "PdfCraftFFI", dependencies: ["pdfcraft_mobile_ffiFFI"], path: "Sources/PdfCraftFFI"),
        .target(name: "PdfCraftKit", dependencies: ["PdfCraftFFI"], path: "Sources/PdfCraftKit"),
    ]
)
