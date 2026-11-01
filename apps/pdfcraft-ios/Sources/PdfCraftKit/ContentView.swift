import CoreGraphics
import PdfCraftFFI
import SwiftUI
import UniformTypeIdentifiers

/// The app's whole UI. A thin Xcode app target's `@main App` just shows this.
public struct PdfCraftRootView: View {
    @StateObject private var viewModel = PdfViewModel()
    @State private var isPickerPresented = false
    @State private var isFindBarVisible = false
    @State private var isOutlineVisible = false
    @State private var query = ""

    public init() {}

    public var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                if isFindBarVisible, isDocumentLoaded {
                    findBar
                }
                content
            }
            .navigationTitle(viewModel.title ?? "PdfCraft")
            .toolbar {
                if isDocumentLoaded {
                    ToolbarItemGroup(placement: .navigationBarTrailing) {
                        if !viewModel.outline.isEmpty {
                            Button(action: { isOutlineVisible = true }) { Image(systemName: "list.bullet") }
                        }
                        Button(action: { isFindBarVisible.toggle() }) { Image(systemName: "magnifyingglass") }
                    }
                }
            }
            .fileImporter(isPresented: $isPickerPresented, allowedContentTypes: [.pdf]) { result in
                if case .success(let url) = result {
                    loadDocument(at: url)
                }
            }
            .sheet(isPresented: $isOutlineVisible) {
                OutlineView(entries: viewModel.outline) { page in
                    viewModel.goToPage(page)
                    isOutlineVisible = false
                }
            }
        }
    }

    private var isDocumentLoaded: Bool {
        if case .loaded = viewModel.state { return true }
        return false
    }

    private var findBar: some View {
        HStack {
            TextField("Find in document", text: $query)
                .textFieldStyle(.roundedBorder)
                .onChange(of: query) { newValue in viewModel.search(newValue) }
            if !query.isEmpty {
                Text(viewModel.matches.isEmpty ? "0/0" : "\((viewModel.matchIndex ?? 0) + 1)/\(viewModel.matches.count)")
                    .foregroundStyle(.secondary)
                Button(action: viewModel.previousMatch) { Image(systemName: "chevron.up") }
                    .disabled(viewModel.matches.isEmpty)
                Button(action: viewModel.nextMatch) { Image(systemName: "chevron.down") }
                    .disabled(viewModel.matches.isEmpty)
            }
        }
        .padding(.horizontal)
        .padding(.vertical, 8)
    }

    @ViewBuilder
    private var content: some View {
        switch viewModel.state {
        case .empty:
            Button("Open a PDF") { isPickerPresented = true }
        case .loading:
            ProgressView()
        case .error(let message):
            VStack(spacing: 16) {
                Text(message)
                Button("Try another file") { isPickerPresented = true }
            }
            .padding()
        case .loaded:
            PageView(
                page: viewModel.page,
                pageCount: viewModel.pageCount,
                image: viewModel.currentPageImage,
                onPrev: { viewModel.goToPage(viewModel.page - 1) },
                onNext: { viewModel.goToPage(viewModel.page + 1) },
                onGoToPage: { viewModel.goToPage($0) }
            )
        }
    }

    private func loadDocument(at url: URL) {
        let accessed = url.startAccessingSecurityScopedResource()
        defer { if accessed { url.stopAccessingSecurityScopedResource() } }
        guard let data = try? Data(contentsOf: url) else { return }
        viewModel.open(data: data)
    }
}

/// The table of contents (`MobileDocument.outline()`), presented as a sheet.
private struct OutlineView: View {
    let entries: [OutlineEntry]
    let onSelect: (Int) -> Void

    var body: some View {
        NavigationStack {
            List(Array(entries.enumerated()), id: \.offset) { _, entry in
                Button(action: { onSelect(Int(entry.page)) }) {
                    Text(entry.title)
                        .padding(.leading, CGFloat(entry.depth) * 16)
                }
            }
            .navigationTitle("Contents")
        }
    }
}

/// The current page, full-bleed, with pinch-to-zoom, a prev/next bar and a jump slider. The
/// active search match (if any), already baked into `image` by `PdfViewModel`, pans/zooms with
/// the page for free.
private struct PageView: View {
    let page: Int
    let pageCount: Int
    let image: CGImage?
    let onPrev: () -> Void
    let onNext: () -> Void
    let onGoToPage: (Int) -> Void

    @State private var zoom: CGFloat = 1.0
    @GestureState private var pinchDelta: CGFloat = 1.0

    var body: some View {
        VStack {
            Group {
                if let image {
                    Image(decorative: image, scale: 1.0)
                        .resizable()
                        .scaledToFit()
                        .scaleEffect(zoom * pinchDelta)
                        .gesture(
                            MagnificationGesture()
                                .updating($pinchDelta) { value, state, _ in state = value }
                                .onEnded { value in zoom = min(max(zoom * value, 1.0), 5.0) }
                        )
                } else {
                    ProgressView()
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .onChange(of: page) { _ in zoom = 1.0 }

            HStack(spacing: 24) {
                Button(action: onPrev) { Image(systemName: "chevron.left") }
                    .disabled(page == 0)
                Text("\(page + 1) / \(pageCount)")
                Button(action: onNext) { Image(systemName: "chevron.right") }
                    .disabled(page == pageCount - 1)
            }
            .padding(.horizontal)

            if pageCount > 1 {
                Slider(
                    value: Binding(
                        get: { Double(page) },
                        set: { onGoToPage(Int($0.rounded())) }
                    ),
                    in: 0...Double(pageCount - 1),
                    step: 1
                )
                .padding([.horizontal, .bottom])
            }
        }
    }
}
