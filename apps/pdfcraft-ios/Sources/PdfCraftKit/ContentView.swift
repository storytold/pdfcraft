import CoreGraphics
import SwiftUI
import UniformTypeIdentifiers

/// The app's whole UI for Milestone 1. A thin Xcode app target's `@main App` just shows this.
public struct PdfCraftRootView: View {
    @StateObject private var viewModel = PdfViewModel()
    @State private var isPickerPresented = false

    public init() {}

    public var body: some View {
        NavigationStack {
            content
                .navigationTitle("PdfCraft")
                .fileImporter(isPresented: $isPickerPresented, allowedContentTypes: [.pdf]) { result in
                    if case .success(let url) = result {
                        loadDocument(at: url)
                    }
                }
        }
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
        case .loaded(_, let page, let pageCount):
            PageView(
                page: page,
                pageCount: pageCount,
                image: viewModel.currentPageImage,
                onPrev: { viewModel.goToPage(page - 1) },
                onNext: { viewModel.goToPage(page + 1) }
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

/// The current page, full-bleed, with a prev/next bar. No pinch-to-zoom yet (Milestone 1).
private struct PageView: View {
    let page: Int
    let pageCount: Int
    let image: CGImage?
    let onPrev: () -> Void
    let onNext: () -> Void

    var body: some View {
        VStack {
            Group {
                if let image {
                    Image(decorative: image, scale: 1.0)
                        .resizable()
                        .scaledToFit()
                } else {
                    ProgressView()
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            HStack(spacing: 24) {
                Button(action: onPrev) { Image(systemName: "chevron.left") }
                    .disabled(page == 0)
                Text("\(page + 1) / \(pageCount)")
                Button(action: onNext) { Image(systemName: "chevron.right") }
                    .disabled(page == pageCount - 1)
            }
            .padding()
        }
    }
}
