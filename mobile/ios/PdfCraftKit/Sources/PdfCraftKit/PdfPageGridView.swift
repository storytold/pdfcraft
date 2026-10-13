import SwiftUI

/// Page organization thumbnail grid view for reordering, rotating, deleting,
/// inserting, and extracting pages in a PDF document.
public struct PdfPageGridView: View {
    public let document: PdfDocument
    public let onPageSelected: ((Int) -> Void)?

    @State private var pageCount: Int = 0
    @State private var selectedPages: Set<Int> = []
    @State private var thumbnails: [Int: CGImage] = [:]
    @State private var isSelectionMode: Bool = false
    @State private var showDeleteConfirm: Bool = false
    @State private var errorMessage: String?

    private let columns = [
        GridItem(.adaptive(minimum: 110, maximum: 160), spacing: 16)
    ]

    public init(document: PdfDocument, onPageSelected: ((Int) -> Void)? = nil) {
        self.document = document
        self.onPageSelected = onPageSelected
    }

    public var body: some View {
        VStack(spacing: 0) {
            // Action bar
            HStack {
                Button(isSelectionMode ? "Done" : "Select") {
                    isSelectionMode.toggle()
                    if !isSelectionMode {
                        selectedPages.removeAll()
                    }
                }

                Spacer()

                if isSelectionMode && !selectedPages.isEmpty {
                    Button(action: rotateSelected) {
                        Image(systemName: "rotate.right")
                    }
                    .accessibilityLabel("Rotate selected pages 90 degrees clockwise")
                    .padding(.horizontal, 4)

                    Button(action: { showDeleteConfirm = true }) {
                        Image(systemName: "trash")
                            .foregroundColor(.red)
                    }
                    .accessibilityLabel("Delete selected pages")
                    .padding(.horizontal, 4)

                    Button(action: duplicateSelected) {
                        Image(systemName: "plus.square.on.square")
                    }
                    .accessibilityLabel("Duplicate selected pages")
                    .padding(.horizontal, 4)
                }

                Button(action: insertBlankPage) {
                    Image(systemName: "plus")
                }
                .accessibilityLabel("Insert blank page")
            }
            .padding()
            .background(Color.secondary.opacity(0.12))

            if let err = errorMessage {
                Text(err)
                    .font(.caption)
                    .foregroundColor(.red)
                    .padding(.horizontal)
            }

            // Grid scroll view
            ScrollView {
                LazyVGrid(columns: columns, spacing: 16) {
                    ForEach(0..<pageCount, id: \.self) { pageIndex in
                        pageCell(for: pageIndex)
                            .onTapGesture {
                                if isSelectionMode {
                                    toggleSelection(for: pageIndex)
                                } else {
                                    onPageSelected?(pageIndex)
                                }
                            }
                    }
                }
                .padding()
            }
        }
        .onAppear(perform: reloadPages)
        .confirmationDialog(
            "Delete \(selectedPages.count) page(s)?",
            isPresented: $showDeleteConfirm,
            titleVisibility: .visible
        ) {
            Button("Delete", role: .destructive, action: deleteSelected)
            Button("Cancel", role: .cancel) {}
        }
    }

    @ViewBuilder
    private func pageCell(for page: Int) -> some View {
        VStack(spacing: 6) {
            ZStack(alignment: .topTrailing) {
                if let thumb = thumbnails[page] {
                    #if canImport(UIKit)
                    Image(uiImage: UIImage(cgImage: thumb))
                        .resizable()
                        .scaledToFit()
                        .frame(minHeight: 140, maxHeight: 180)
                        .background(Color.white)
                        .cornerRadius(6)
                        .shadow(radius: 2)
                    #else
                    Image(decorative: thumb, scale: 1.0)
                        .resizable()
                        .scaledToFit()
                        .frame(minHeight: 140, maxHeight: 180)
                        .background(Color.white)
                        .cornerRadius(6)
                        .shadow(radius: 2)
                    #endif
                } else {
                    Rectangle()
                        .fill(Color(.tertiarySystemFill))
                        .frame(minHeight: 140, maxHeight: 180)
                        .cornerRadius(6)
                        .overlay(ProgressView())
                }

                if isSelectionMode {
                    Image(systemName: selectedPages.contains(page) ? "checkmark.circle.fill" : "circle")
                        .foregroundColor(selectedPages.contains(page) ? .accentColor : .secondary)
                        .padding(6)
                }
            }

            Text("\(page + 1)")
                .font(.caption)
                .foregroundColor(.secondary)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Page \(page + 1) of \(pageCount)")
        .accessibilityHint(isSelectionMode ? (selectedPages.contains(page) ? "Selected. Double-tap to deselect." : "Not selected. Double-tap to select.") : "Double-tap to open page \(page + 1)")
        .accessibilityAddTraits(selectedPages.contains(page) ? .isSelected : [])
        .onAppear {
            loadThumbnail(for: page)
        }
    }

    private func toggleSelection(for page: Int) {
        if selectedPages.contains(page) {
            selectedPages.remove(page)
        } else {
            selectedPages.insert(page)
        }
    }

    private func reloadPages() {
        self.pageCount = document.pageCount
        self.thumbnails.removeAll()
        for p in 0..<pageCount {
            loadThumbnail(for: p)
        }
    }

    private func loadThumbnail(for page: Int) {
        guard thumbnails[page] == nil else { return }
        DispatchQueue.global(qos: .userInitiated).async {
            if let thumb = try? document.renderThumbnail(page: page, maxDimension: 180) {
                DispatchQueue.main.async {
                    self.thumbnails[page] = thumb
                }
            }
        }
    }

    // MARK: - Page Operations

    private func rotateSelected() {
        let pages1Based = selectedPages.map { $0 + 1 }.sorted()
        do {
            try document.invokeTool(name: "page_rotate", args: [
                "pages": pages1Based,
                "degrees": 90
            ])
            reloadPages()
        } catch {
            self.errorMessage = error.localizedDescription
        }
    }

    private func deleteSelected() {
        let pages1Based = selectedPages.map { $0 + 1 }.sorted()
        do {
            try document.invokeTool(name: "page_delete", args: [
                "pages": pages1Based
            ])
            selectedPages.removeAll()
            reloadPages()
        } catch {
            self.errorMessage = error.localizedDescription
        }
    }

    private func duplicateSelected() {
        let pages1Based = selectedPages.map { $0 + 1 }.sorted()
        do {
            try document.invokeTool(name: "page_duplicate", args: [
                "pages": pages1Based
            ])
            reloadPages()
        } catch {
            self.errorMessage = error.localizedDescription
        }
    }

    private func insertBlankPage() {
        let insertPos = (selectedPages.max() ?? (pageCount - 1)) + 1 + 1
        do {
            try document.invokeTool(name: "page_insert_blank", args: [
                "at": insertPos
            ])
            reloadPages()
        } catch {
            self.errorMessage = error.localizedDescription
        }
    }
}
