import SwiftUI

struct PageView: View {
    let page: PageMetadata
    let bridge: DocumentBridge
    let revision: Int
    let selected: Bool
    let select: () -> Void
    @State private var image: UIImage?
    @State private var failure: String?
    @State private var warning = false
    @State private var zoom: CGFloat = 1
    @GestureState private var magnification: CGFloat = 1

    var body: some View {
        VStack(spacing: 8) {
            HStack {
                Button("Page \(page.number)", action: select)
                    .accessibilityLabel("Select page \(page.number)")
                    .accessibilityAddTraits(selected ? .isSelected : [])
                if warning {
                    Text("Rendering warnings").font(.caption).foregroundStyle(.orange)
                }
                Spacer()
                Menu("Zoom") {
                    Button("Fit page") { zoom = 1 }
                    Button("Zoom in") { zoom = min(zoom + 0.5, 3) }
                    Button("Zoom out") { zoom = max(zoom - 0.5, 1) }
                }
            }
            Group {
                if let image {
                    GeometryReader { geometry in
                        ScrollView([.horizontal, .vertical]) {
                            Image(uiImage: image)
                                .resizable()
                                .scaledToFit()
                                .frame(width: geometry.size.width * min(max(zoom * magnification, 1), 3),
                                       height: geometry.size.height * min(max(zoom * magnification, 1), 3))
                                .accessibilityLabel("Rendered PDF page \(page.number). Text reading is not available in this proof of concept.")
                                .accessibilityIdentifier("rendered-page-\(page.number)")
                                .onTapGesture(perform: select)
                                .gesture(MagnifyGesture()
                                    .updating($magnification) { value, state, _ in state = value.magnification }
                                    .onEnded { zoom = min(max(zoom * $0.magnification, 1), 3) })
                        }
                    }
                } else if let failure {
                    ContentUnavailableView {
                        Text("Cannot render page")
                    } description: {
                        Text(failure)
                    }
                } else {
                    ProgressView("Rendering page \(page.number)")
                }
            }
            .aspectRatio(page.width / page.height, contentMode: .fit)
            .background(.white)
            .overlay(Rectangle().stroke(selected ? Color.accentColor : .secondary.opacity(0.3), lineWidth: selected ? 3 : 1))
        }
        .padding(.horizontal)
        .task(id: revision) {
            image = nil
            failure = nil
            do {
                let output = try await bridge.render(page: page.number, edge: 1600)
                try Task.checkCancellation()
                guard let decoded = UIImage(data: output.data) else {
                    throw MobileFailure(code: "image", message: "The rendered image could not be decoded.")
                }
                image = decoded
                warning = output.hasWarnings
            } catch is CancellationError {
                // Disappearing pages must not publish stale images or errors.
            } catch {
                failure = error.localizedDescription
            }
        }
        .onDisappear { image = nil }
    }
}
