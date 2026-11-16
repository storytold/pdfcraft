import XCTest
import UIKit
@testable import PdfCraftMobilePOC

@MainActor
final class DocumentBridgeTests: XCTestCase {
    func testPackagedAppHasUsableIconsForBothDeviceFamilies() throws {
        // Bundle's runtime lookup filters device-qualified keys/resources on iPhone.
        let url = try XCTUnwrap(Bundle.main.url(forResource: "Info", withExtension: "plist"))
        let info = try XCTUnwrap(PropertyListSerialization.propertyList(
            from: Data(contentsOf: url), format: nil) as? [String: Any])
        let resources = try FileManager.default.contentsOfDirectory(
            at: Bundle.main.bundleURL, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "png" }
        for key in ["CFBundleIcons", "CFBundleIcons~ipad"] {
            let icons = try XCTUnwrap(info[key] as? [String: Any], key)
            let primary = try XCTUnwrap(icons["CFBundlePrimaryIcon"] as? [String: Any])
            let files = try XCTUnwrap(primary["CFBundleIconFiles"] as? [String])
            XCTAssertFalse(files.isEmpty, key)
            for name in files {
                let resource = try XCTUnwrap(resources.first {
                    $0.lastPathComponent.hasPrefix(name)
                }, "Missing packaged icon: \(name)")
                let image = try XCTUnwrap(UIImage(contentsOfFile: resource.path))
                XCTAssertGreaterThan(image.size.width, 0)
                XCTAssertGreaterThan(image.size.height, 0)
            }
        }
    }

    func testStageSyntheticFilesFixture() throws {
        let documents = try FileManager.default.url(for: .documentDirectory, in: .userDomainMask,
                                                   appropriateFor: nil, create: true)
        let directory = documents.appendingPathComponent("PdfCraft UI Workflow Test", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let previousExport = directory.appendingPathComponent("copy-2.pdf")
        if FileManager.default.fileExists(atPath: previousExport.path) {
            try FileManager.default.removeItem(at: previousExport)
        }
        try Self.fixture().write(to: directory.appendingPathComponent("PdfCraft iOS UI Fixture.pdf"), options: .atomic)
        try Self.fixture(withText: true).write(to: directory.appendingPathComponent("PdfCraft iOS Text Fixture.pdf"), options: .atomic)
        try Self.fixture(withText: true, pageCount: 2)
            .write(to: directory.appendingPathComponent("PdfCraft iOS Multipage Fixture.pdf"), options: .atomic)
        try Data("not a PDF".utf8)
            .write(to: directory.appendingPathComponent("PdfCraft iOS Invalid Fixture.pdf"), options: .atomic)
    }

    func testRealRustRoundTripAndCleanup() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let source = directory.appendingPathComponent("source.pdf")
        let original = Self.fixture()
        try original.write(to: source)
        let bridge = DocumentBridge()
        let opened = try await bridge.open(source)
        XCTAssertEqual(opened.pageCount, 1)
        let initial = try await bridge.render(page: 1, edge: 600)
        XCTAssertFalse(initial.data.isEmpty)
        let emptyText = try await bridge.pageText(page: 1)
        XCTAssertEqual(emptyText.text, "")
        XCTAssertFalse(emptyText.truncated)
        let rotated = try await bridge.rotate(page: 1)
        XCTAssertEqual(rotated.pages.first?.rotation, 90)
        XCTAssertTrue(rotated.dirty)
        let changed = try await bridge.render(page: 1, edge: 600)
        XCTAssertNotEqual(initial.data, changed.data)
        let saved = try await bridge.saveCopy()
        XCTAssertEqual(try Data(contentsOf: source), original)

        let reopened = DocumentBridge()
        let metadata = try await reopened.open(saved)
        XCTAssertEqual(metadata.pages.first?.rotation, 90)
        let renderedAgain = try await reopened.render(page: 1, edge: 600)
        XCTAssertEqual(renderedAgain.data, changed.data)
        try await reopened.close(discard: false)
        try await bridge.close(discard: false)
        XCTAssertFalse(FileManager.default.fileExists(atPath: saved.path))
    }

    func testMalformedPDFReturnsAnErrorAndSessionCanRecover() async throws {
        let source = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).appendingPathExtension("pdf")
        defer { try? FileManager.default.removeItem(at: source) }
        try Data("not a PDF".utf8).write(to: source)
        let bridge = DocumentBridge()
        do {
            _ = try await bridge.open(source)
            XCTFail("Malformed input must not open")
        } catch {
            XCTAssertTrue(error is MobileFailure)
        }
        try Self.fixture().write(to: source)
        _ = try await bridge.open(source)
        _ = try await bridge.rotate(page: 1)
        do {
            try await bridge.close(discard: false)
            XCTFail("Closing must not silently discard unsaved work")
        } catch {
            XCTAssertTrue(error is MobileFailure)
        }
        try await bridge.close(discard: true)
    }

    func testEmbeddedTextCrossesRustBridgeAndFollowsDocumentLifecycle() async throws {
        let source = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).appendingPathExtension("pdf")
        defer { try? FileManager.default.removeItem(at: source) }
        try Self.fixture(withText: true).write(to: source)
        let bridge = DocumentBridge()
        _ = try await bridge.open(source)
        let output = try await bridge.pageText(page: 1)
        XCTAssertEqual(output.page, 1)
        XCTAssertEqual(output.text, "PdfCraft text reading\ncaf\u{e9}")
        XCTAssertFalse(output.truncated)
        do {
            _ = try await bridge.pageText(page: 0)
            XCTFail("Invalid page numbers must not return empty text")
        } catch {
            XCTAssertTrue(error is MobileFailure)
        }
        _ = try await bridge.rotate(page: 1)
        let rotated = try await bridge.pageText(page: 1)
        XCTAssertEqual(rotated.text, output.text)
        try await bridge.close(discard: true)
        do {
            _ = try await bridge.pageText(page: 1)
            XCTFail("Closed documents must not return stale text")
        } catch {
            XCTAssertTrue(error is MobileFailure)
        }
    }

    func testMultiPageRoundTripKeepsOtherPagesUnchanged() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let source = directory.appendingPathComponent("multipage.pdf")
        let original = Self.fixture(withText: true, pageCount: 2)
        try original.write(to: source)
        let bridge = DocumentBridge()
        let opened = try await bridge.open(source)
        XCTAssertEqual(opened.pageCount, 2)
        let first = try await bridge.render(page: 1, edge: 600)
        let second = try await bridge.render(page: 2, edge: 600)
        let metadata = try await bridge.rotate(page: 2)
        XCTAssertEqual(metadata.pages.map(\.rotation), [0, 90])
        let changed = try await bridge.render(page: 2, edge: 600)
        XCTAssertNotEqual(changed.data, second.data)
        let text = try await bridge.pageText(page: 2)
        XCTAssertEqual(text.text, "PdfCraft second page\nSecond page only")
        let saved = try await bridge.saveCopy()
        let reopened = DocumentBridge()
        let restored = try await reopened.open(saved)
        XCTAssertEqual(restored.pages.map(\.rotation), [0, 90])
        let firstAgain = try await reopened.render(page: 1, edge: 600)
        let secondAgain = try await reopened.render(page: 2, edge: 600)
        XCTAssertEqual(firstAgain.data, first.data)
        XCTAssertEqual(secondAgain.data, changed.data)
        XCTAssertEqual(try Data(contentsOf: source), original)
        try await reopened.close(discard: false)
        try await bridge.close(discard: false)
    }

    private static func fixture(withText: Bool = false, pageCount: Int = 1) -> Data {
        let fontID = 3 + pageCount * 2
        let resources = withText ? " /Resources << /Font << /F1 \(fontID) 0 R >> >>" : ""
        let kids = (0..<pageCount).map { "\(3 + $0 * 2) 0 R" }.joined(separator: " ")
        var objects = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [\(kids)] /Count \(pageCount) >>"
        ]
        for index in 0..<pageCount {
            var content = index == 0 ? "1 0 0 rg 10 20 60 40 re f" : "0 0 1 rg 80 20 60 40 re f"
            if withText {
                let lines = index == 0 ? ["PdfCraft text reading", "caf\\351"] : ["PdfCraft second page", "Second page only"]
                content += "\n0 g BT /F1 12 Tf 10 260 Td (\(lines[0])) Tj 0 -18 Td (\(lines[1])) Tj ET"
            }
            objects.append("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents \(4 + index * 2) 0 R\(resources) >>")
            objects.append("<< /Length \(content.utf8.count) >>\nstream\n\(content)\nendstream")
        }
        if withText {
            objects.append("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>")
        }
        var pdf = "%PDF-1.7\n"
        var offsets: [Int] = []
        for (index, object) in objects.enumerated() {
            offsets.append(pdf.utf8.count)
            pdf += "\(index + 1) 0 obj\n\(object)\nendobj\n"
        }
        let xref = pdf.utf8.count
        pdf += "xref\n0 \(objects.count + 1)\n0000000000 65535 f \n"
        for offset in offsets { pdf += String(format: "%010d 00000 n \n", offset) }
        pdf += "trailer\n<< /Size \(objects.count + 1) /Root 1 0 R >>\nstartxref\n\(xref)\n%%EOF\n"
        return Data(pdf.utf8)
    }
}
