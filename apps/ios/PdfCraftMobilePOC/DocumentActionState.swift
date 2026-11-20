import Foundation
import Observation

@MainActor
@Observable
final class DocumentActionState {
    var document: DocumentMetadata?
    var selectedTool: WorkspaceTool?
    var revision = 0
    var busy = false
    var stagedCopy: URL?
    private(set) var releasingCopy = false

    func clearDocument() {
        document = nil
        selectedTool = nil
        stagedCopy = nil
        revision += 1
    }

    func failed(_ error: Error) {
        if (error as? MobileFailure)?.sessionEnded == true { clearDocument() }
        busy = stagedCopy != nil
    }

    func releaseCopy(using release: (URL) async throws -> Void) async throws {
        guard let copy = stagedCopy, !releasingCopy else { return }
        releasingCopy = true
        defer { releasingCopy = false }
        try await release(copy)
        stagedCopy = nil
        busy = false
    }
}
