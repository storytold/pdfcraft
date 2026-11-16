import Foundation

struct PageMetadata: Decodable, Identifiable, Sendable {
    let number: UInt32
    let width: Double
    let height: Double
    let rotation: Int
    var id: UInt32 { number }
}

struct DocumentMetadata: Decodable, Sendable {
    let pageCount: Int
    let dirty: Bool
    let editable: Bool
    let pages: [PageMetadata]
    let warnings: [String]

    enum CodingKeys: String, CodingKey {
        case pageCount = "page_count"
        case dirty, editable, pages, warnings
    }
}

struct MobileFailure: Decodable, LocalizedError, Sendable {
    let code: String
    let message: String
    var errorDescription: String? { message }
    var sessionEnded: Bool { code == "closed" || code == "worker" || code == "engine" }
}

struct RenderedPage: Sendable {
    let data: Data
    let hasWarnings: Bool
}

struct ExtractedPageText: Decodable, Sendable {
    let page: UInt32
    let text: String
    let truncated: Bool
}

private struct OutputPath: Decodable {
    let path: String
}

private struct OpenOutput: Decodable {
    let ok: Bool
    let value: String
}

private struct RenderOutput: Decodable {
    let path: String
    let warnings: [RenderWarning]
}

private struct RenderWarning: Decodable {}

/// Only this actor owns the generated, non-Sendable FFI object.
actor DocumentBridge {
    private var session: MobileSession?

    private func engine() throws -> MobileSession {
        if let session { return session }
        do {
            let created = try create_mobile_session()
            session = created
            return created
        } catch {
            throw translated(error)
        }
    }

    private func translated(_ error: Error) -> Error {
        guard let rust = error as? RustString else { return error }
        let text = rust.toString()
        do {
            return try JSONDecoder().decode(MobileFailure.self, from: Data(text.utf8))
        } catch {
            return MobileFailure(code: "bridge", message: "PdfCraft returned an unreadable error.")
        }
    }

    private func decode<T: Decodable>(_ type: T.Type, _ call: () throws -> RustString) throws -> T {
        do {
            return try JSONDecoder().decode(type, from: Data(call().toString().utf8))
        } catch {
            throw translated(error)
        }
    }

    func open(_ url: URL) throws -> DocumentMetadata {
        try Task.checkCancellation()
        let access = url.startAccessingSecurityScopedResource()
        defer { if access { url.stopAccessingSecurityScopedResource() } }
        var coordinationError: NSError?
        var result: Result<DocumentMetadata, Error>?
        NSFileCoordinator().coordinate(readingItemAt: url, options: [], error: &coordinationError) { coordinated in
            result = Result {
                let engine = try self.engine()
                let output = try self.decode(OpenOutput.self) { engine.open(coordinated.path) }
                let payload = Data(output.value.utf8)
                guard output.ok else {
                    throw try JSONDecoder().decode(MobileFailure.self, from: payload)
                }
                return try JSONDecoder().decode(DocumentMetadata.self, from: payload)
            }
        }
        if let coordinationError { throw coordinationError }
        guard let result else {
            throw MobileFailure(code: "access", message: "Files did not provide access to the selected PDF.")
        }
        return try result.get()
    }

    func render(page: UInt32, edge: UInt32) throws -> RenderedPage {
        try Task.checkCancellation()
        let engine = try engine()
        let output = try decode(RenderOutput.self) { try engine.render(page, edge) }
        let data = try Data(contentsOf: URL(fileURLWithPath: output.path))
        try Task.checkCancellation()
        return RenderedPage(data: data, hasWarnings: !output.warnings.isEmpty)
    }

    func rotate(page: UInt32) throws -> DocumentMetadata {
        let engine = try engine()
        return try decode(DocumentMetadata.self) { try engine.rotate(page) }
    }

    func pageText(page: UInt32) throws -> ExtractedPageText {
        try Task.checkCancellation()
        let engine = try engine()
        let output = try decode(ExtractedPageText.self) { try engine.page_text(page) }
        try Task.checkCancellation()
        return output
    }

    func saveCopy() throws -> URL {
        let engine = try engine()
        let output = try decode(OutputPath.self) { try engine.save_copy() }
        return URL(fileURLWithPath: output.path)
    }

    func close(discard: Bool) throws {
        guard let session else { return }
        do {
            _ = try session.close(discard)
            self.session = nil
        } catch {
            let failure = translated(error)
            if (failure as? MobileFailure)?.sessionEnded == true { self.session = nil }
            throw failure
        }
    }
}
