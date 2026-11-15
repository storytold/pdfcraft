import SwiftUI
import UniformTypeIdentifiers
import CoreTransferable

private struct Export: Identifiable, Transferable {
    let id = UUID()
    let url: URL

    static var transferRepresentation: some TransferRepresentation {
        // SwiftUI builds a FileWrapper before applying the export dialog's default filename.
        FileRepresentation(exportedContentType: .pdf) { SentTransferredFile($0.url) }
            .suggestedFileName { $0.url.lastPathComponent }
    }
}

struct ContentView: View {
    @Environment(\.horizontalSizeClass) private var sizeClass
    @State private var bridge = DocumentBridge()
    @State private var document: DocumentMetadata?
    @State private var selectedPage: UInt32 = 1
    @State private var revision = 0
    @State private var importing = false
    @State private var busy = false
    @State private var failure: String?
    @State private var confirmClose = false
    @State private var export: Export?
    @State private var share: Export?
    @State private var savedCopy: URL?

    var body: some View {
        NavigationStack {
            Group {
                if let document {
                    HStack(spacing: 0) {
                        if sizeClass == .regular {
                            List(document.pages) { page in
                                Button("Page \(page.number)") { selectedPage = page.number }
                                    .accessibilityAddTraits(selectedPage == page.number ? .isSelected : [])
                            }
                            .frame(idealWidth: 180, maxWidth: 220)
                        }
                        ScrollViewReader { reader in
                            ScrollView {
                                LazyVStack(spacing: 24) {
                                    if !document.warnings.isEmpty {
                                        Text("PdfCraft reported document warnings. Some content may not display completely.")
                                            .font(.callout).padding()
                                    }
                                    ForEach(document.pages) { page in
                                        PageView(page: page, bridge: bridge, revision: revision,
                                                 selected: selectedPage == page.number) { selectedPage = page.number }
                                            .id(page.number)
                                    }
                                }
                                .padding(.vertical)
                            }
                            .onChange(of: selectedPage) { _, page in reader.scrollTo(page, anchor: .top) }
                        }
                    }
                } else {
                    ContentUnavailableView {
                        Text("PdfCraft iOS Proof of Concept")
                    } description: {
                        Text("Open a PDF to view, rotate, save a copy, and share. Processing stays on this device. Experimental; not an official ArtCraft release.")
                    } actions: {
                        Button("Open PDF") { importing = true }
                            .buttonStyle(.borderedProminent)
                            .disabled(busy)
                            .fileImporter(isPresented: $importing, allowedContentTypes: [.pdf]) { result in
                                switch result {
                                case .success(let url): open(url)
                                case .failure(let error): failure = error.localizedDescription
                                }
                            }
                    }
                }
            }
            .navigationTitle(document == nil ? "PdfCraftMobilePOC" : "Page \(selectedPage) of \(document?.pageCount ?? 0)")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    if document != nil {
                        Button("Close") {
                            if document?.dirty == true { confirmClose = true } else { close(discard: false) }
                        }
                        .disabled(busy)
                    }
                }
                ToolbarItemGroup(placement: .bottomBar) {
                    if let document {
                        Button("Rotate") { rotate() }
                            .disabled(busy || !document.editable)
                            .accessibilityLabel("Rotate page \(selectedPage) clockwise")
                        Spacer()
                        Button("Save a Copy") { save() }.disabled(busy)
                        Button("Share") { shareDocument() }.disabled(busy)
                    }
                }
            }
            .overlay { if busy { ProgressView("Working locally").padding().background(.regularMaterial).clipShape(RoundedRectangle(cornerRadius: 12)) } }
            .fileExporter(isPresented: Binding(get: { export != nil }, set: { if !$0 { export = nil } }),
                          item: export, contentTypes: [.pdf], defaultFilename: export?.url.lastPathComponent) { result in
                switch result {
                case .success:
                    if let current = document {
                        document = DocumentMetadata(pageCount: current.pageCount, dirty: false, editable: current.editable,
                                                    pages: current.pages, warnings: current.warnings)
                    }
                case .failure(let error):
                    failure = error.localizedDescription
                }
                export = nil
            } onCancellation: {
                export = nil
            }
            .sheet(item: $share) { item in ShareSheet(url: item.url) }
            .alert("PdfCraft", isPresented: Binding(get: { failure != nil }, set: { if !$0 { failure = nil } })) {
                Button("OK") { failure = nil }
            } message: { Text(failure ?? "") }
            .confirmationDialog("Discard unsaved changes?", isPresented: $confirmClose, titleVisibility: .visible) {
                Button("Discard and Close", role: .destructive) { close(discard: true) }
                Button("Cancel", role: .cancel) {}
            }
        }
    }

    private func open(_ url: URL) {
        busy = true
        Task {
            do {
                document = try await bridge.open(url)
                selectedPage = 1
                revision += 1
            } catch { failure = error.localizedDescription }
            busy = false
        }
    }

    private func rotate() {
        busy = true
        Task {
            do {
                document = try await bridge.rotate(page: selectedPage)
                revision += 1
                savedCopy = nil
            } catch { failure = error.localizedDescription }
            busy = false
        }
    }

    private func save() {
        busy = true
        Task {
            do {
                let url = try await bridge.saveCopy()
                savedCopy = url
                export = Export(url: url)
            } catch { failure = error.localizedDescription }
            busy = false
        }
    }

    private func close(discard: Bool) {
        busy = true
        Task {
            do {
                try await bridge.close(discard: discard)
                document = nil
                savedCopy = nil
                revision += 1
            } catch {
                if (error as? MobileFailure)?.sessionEnded == true {
                    document = nil
                    savedCopy = nil
                    revision += 1
                }
                failure = error.localizedDescription
            }
            busy = false
        }
    }

    private func shareDocument() {
        busy = true
        Task {
            do {
                let url: URL
                if let savedCopy { url = savedCopy } else { url = try await bridge.saveCopy() }
                savedCopy = url
                share = Export(url: url)
            } catch { failure = error.localizedDescription }
            busy = false
        }
    }
}
