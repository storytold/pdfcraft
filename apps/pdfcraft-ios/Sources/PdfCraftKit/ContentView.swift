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
                onNext: { viewModel.goToPage(page + 1) },
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

/// The current page, full-bleed, with pinch-to-zoom, a prev/next bar and a jump slider.
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
