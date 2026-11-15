import XCTest
@testable import PdfCraftMobilePOC

@MainActor
final class DocumentBridgeTests: XCTestCase {
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

    private static func fixture() -> Data {
        let content = "1 0 0 rg 10 20 60 40 re f"
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents 4 0 R >>",
            "<< /Length \(content.utf8.count) >>\nstream\n\(content)\nendstream"
        ]
        var pdf = "%PDF-1.7\n"
        var offsets: [Int] = []
        for (index, object) in objects.enumerated() {
            offsets.append(pdf.utf8.count)
            pdf += "\(index + 1) 0 obj\n\(object)\nendobj\n"
        }
        let xref = pdf.utf8.count
        pdf += "xref\n0 5\n0000000000 65535 f \n"
        for offset in offsets { pdf += String(format: "%010d 00000 n \n", offset) }
        pdf += "trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n\(xref)\n%%EOF\n"
        return Data(pdf.utf8)
    }
}
