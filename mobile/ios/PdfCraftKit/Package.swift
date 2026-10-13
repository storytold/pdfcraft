// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "PdfCraftKit",
    platforms: [
        .iOS(.v17),
        .macOS(.v14)
    ],
    products: [
        .library(
            name: "PdfCraftKit",
            targets: ["PdfCraftKit"]
        )
    ],
    targets: [
        .binaryTarget(
            name: "pdfcraft_ffiFFI",
            path: "Frameworks/pdfcraft_ffiFFI.xcframework"
        ),
        .target(
            name: "PdfCraftKit",
            dependencies: ["pdfcraft_ffiFFI"],
            path: "Sources/PdfCraftKit",
            resources: [.process("Resources")]
        ),
        .testTarget(
            name: "PdfCraftKitTests",
            dependencies: ["PdfCraftKit"],
            path: "Tests/PdfCraftKitTests"
        )
    ]
)
