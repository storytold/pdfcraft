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
    @Environment(\.colorScheme) private var colorScheme
    @ScaledMetric(relativeTo: .body) private var pageSelectorHeight: CGFloat = 52
    @AppStorage("mobile.workspace.appearance") private var appearance = WorkspaceAppearance.system
    @AppStorage("mobile.workspace.pinnedTools") private var pinnedToolIDs = ""
    @State private var bridge = DocumentBridge()
    @State private var document: DocumentMetadata?
    @State private var documentName = ""
    @State private var selectedPage: UInt32 = 1
    @State private var revision = 0
    @State private var importing = false
    @State private var busy = false
    @State private var failure: String?
    @State private var confirmClose = false
    @State private var export: Export?
    @State private var share: Export?
    @State private var savedCopy: URL?
    @State private var sidebarVisible = true
    @State private var showingTools = false
    @State private var toolQuery = ""
    @State private var selectedTool: WorkspaceTool?
    @State private var pendingTool: WorkspaceTool?
    @State private var queuedTool: WorkspaceTool?
    @State private var textPage: PageMetadata?

    private var pinnedTools: Set<WorkspaceTool> {
        let ids = Set(pinnedToolIDs.split(separator: ",").map(String.init))
        return Set(WorkspaceTool.allCases.filter { ids.contains($0.rawValue) })
    }

    private var style: WorkspaceStyle { WorkspaceStyle(scheme: colorScheme) }

    var body: some View {
        NavigationStack {
            HStack(spacing: 0) {
                if sizeClass == .regular && sidebarVisible {
                    sidebar
                    Divider()
                }
                workspace
            }
            .background(style.background)
            .navigationTitle(document == nil ? "PdfCraft" : documentName)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItemGroup(placement: .topBarLeading) {
                    Button {
                        if sizeClass == .regular { sidebarVisible.toggle() } else { showingTools = true }
                    } label: {
                        Image(systemName: "sidebar.left")
                    }
                    .accessibilityLabel("All tools")
                    if document != nil {
                        Button("Close") {
                            if document?.dirty == true { confirmClose = true } else { close(discard: false) }
                        }
                        .disabled(busy)
                        .keyboardShortcut("w")
                    }
                }
                ToolbarItemGroup(placement: .topBarTrailing) {
                    if document == nil {
                        Button("Open") { beginOpen() }
                            .disabled(busy)
                            .keyboardShortcut("o")
                            .accessibilityIdentifier("toolbar-open")
                    }
                    Button { showingTools = true } label: {
                        Image(systemName: "magnifyingglass")
                    }
                    .accessibilityLabel("Find tools and commands")
                    .keyboardShortcut("k")
                    Menu {
                        Picker("Appearance", selection: $appearance) {
                            ForEach(WorkspaceAppearance.allCases) { option in
                                Text(option.title).tag(option)
                            }
                        }
                    } label: {
                        Image(systemName: "circle.lefthalf.filled")
                    }
                    .accessibilityLabel("Appearance")
                    .accessibilityValue(appearance.title)
                }
                ToolbarItemGroup(placement: .bottomBar) {
                    if let document {
                        Button("Rotate") { rotate() }
                            .disabled(busy || !document.editable)
                            .accessibilityLabel("Rotate page \(selectedPage) clockwise")
                        Spacer()
                        Button("Save a Copy") { save() }
                            .disabled(busy)
                            .keyboardShortcut("s", modifiers: [.command, .shift])
                        Button("Share") { shareDocument() }.disabled(busy)
                    }
                }
            }
            .toolbarBackground(style.panel, for: .navigationBar, .bottomBar)
            .toolbarBackground(.visible, for: .navigationBar, .bottomBar)
            .overlay { if busy { ProgressView("Working locally").padding().background(.regularMaterial).clipShape(RoundedRectangle(cornerRadius: 12)) } }
            .fileImporter(isPresented: $importing, allowedContentTypes: [.pdf]) { result in
                let tool = pendingTool
                pendingTool = nil
                switch result {
                case .success(let url): open(url, then: tool)
                case .failure(let error): failure = error.localizedDescription
                }
            }
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
            .sheet(item: $textPage) { page in
                PageTextView(page: page.number, bridge: bridge, revision: revision)
            }
            .sheet(isPresented: $showingTools, onDismiss: {
                if let tool = queuedTool {
                    queuedTool = nil
                    choose(tool)
                }
                toolQuery = ""
            }) {
                NavigationStack {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 16) {
                            WorkspaceToolList(query: toolQuery, pinned: pinnedTools, selected: selectedTool,
                                              select: { queuedTool = $0; showingTools = false }, togglePin: togglePin)
                            Text("Tools available in this mobile build. Choose a tool to open a PDF first when needed.")
                                .font(.footnote).foregroundStyle(.secondary)
                        }
                        .padding()
                        .disabled(busy)
                    }
                    .background(style.background)
                    .accessibilityIdentifier("tools-browser")
                    .navigationTitle("All tools")
                    .navigationBarTitleDisplayMode(.inline)
                    .searchable(text: $toolQuery, prompt: "Find tools and commands")
                    .toolbar {
                        ToolbarItem(placement: .confirmationAction) {
                            Button("Done") { showingTools = false }
                        }
                    }
                }
            }
            .alert("PdfCraft", isPresented: Binding(get: { failure != nil }, set: { if !$0 { failure = nil } })) {
                Button("OK") { failure = nil }
            } message: { Text(failure ?? "") }
            .confirmationDialog("Discard unsaved changes?", isPresented: $confirmClose, titleVisibility: .visible) {
                Button("Discard and Close", role: .destructive) { close(discard: true) }
                Button("Cancel", role: .cancel) {}
            }
        }
        .tint(style.accent)
        .preferredColorScheme(appearance.colorScheme)
    }

    private var sidebar: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 16) {
                Text("All tools").font(.headline).accessibilityAddTraits(.isHeader)
                WorkspaceToolList(query: "", pinned: pinnedTools, selected: selectedTool,
                                  select: choose, togglePin: togglePin)
                if let document {
                    Divider()
                    Text("Pages").font(.headline).accessibilityAddTraits(.isHeader)
                    ForEach(document.pages) { page in
                        Button("Page \(page.number)") { selectedPage = page.number }
                            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                            .accessibilityAddTraits(selectedPage == page.number ? .isSelected : [])
                    }
                }
            }
            .padding(16)
        }
        .frame(width: 240)
        .frame(maxHeight: .infinity)
        .background(style.panel)
        .disabled(busy)
        .accessibilityIdentifier("workspace-sidebar")
    }

    @ViewBuilder
    private var workspace: some View {
        if let document {
            VStack(spacing: 0) {
                HStack {
                    Text("Page \(selectedPage) of \(document.pageCount)").font(.headline)
                    Spacer()
                    if document.dirty { Text("Unsaved changes").font(.caption).foregroundStyle(.secondary) }
                }
                .padding()
                .background(style.panel)
                if selectedTool == .organize {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(WorkspaceTool.organize.title).font(.headline)
                        Text(WorkspaceTool.organize.detail).font(.callout).foregroundStyle(.secondary)
                        if sizeClass != .regular || !sidebarVisible {
                            ScrollView(.horizontal) {
                                LazyHStack {
                                    ForEach(document.pages) { page in
                                        Button("Page \(page.number)") { selectedPage = page.number }
                                            .buttonStyle(.bordered)
                                            .frame(minHeight: 44)
                                            .accessibilityAddTraits(selectedPage == page.number ? .isSelected : [])
                                    }
                                }
                            }
                            .frame(height: pageSelectorHeight)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding()
                    .background(style.panel)
                    .accessibilityIdentifier("organize-pages-panel")
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
            .disabled(busy)
        } else {
            WorkspaceHome(pinned: pinnedTools, busy: busy, open: { beginOpen() }, select: choose,
                          togglePin: togglePin, showTools: { showingTools = true })
        }
    }

    private func togglePin(_ tool: WorkspaceTool) {
        var pinned = pinnedTools
        if pinned.contains(tool) { pinned.remove(tool) } else { pinned.insert(tool) }
        pinnedToolIDs = WorkspaceTool.allCases.filter { pinned.contains($0) }.map(\.rawValue).joined(separator: ",")
    }

    private func beginOpen(for tool: WorkspaceTool? = nil) {
        pendingTool = tool
        importing = true
    }

    private func choose(_ tool: WorkspaceTool) {
        guard !busy else { return }
        guard let document else {
            beginOpen(for: tool)
            return
        }
        selectedTool = tool
        switch tool {
        case .organize:
            break
        case .readText:
            guard let page = document.pages.first(where: { $0.number == selectedPage }) else {
                failure = "Select a page before reading text."
                return
            }
            textPage = page
        case .saveCopy:
            save()
        case .share:
            shareDocument()
        }
    }

    private func open(_ url: URL, then tool: WorkspaceTool? = nil) {
        busy = true
        Task {
            do {
                document = try await bridge.open(url)
                documentName = url.deletingPathExtension().lastPathComponent
                selectedPage = 1
                selectedTool = nil
                revision += 1
                busy = false
                if let tool { choose(tool) }
            } catch {
                failure = error.localizedDescription
                busy = false
            }
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
                selectedTool = nil
                savedCopy = nil
                revision += 1
            } catch {
                if (error as? MobileFailure)?.sessionEnded == true {
                    document = nil
                    selectedTool = nil
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
