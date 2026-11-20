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
    @State private var actions = DocumentActionState()
    @State private var documentName = ""
    @State private var selectedPage: UInt32 = 1
    @State private var importing = false
    @State private var failure: String?
    @State private var confirmClose = false
    @State private var export: Export?
    @State private var share: Export?
    @State private var sidebarVisible = true
    @State private var showingTools = false
    @State private var toolQuery = ""
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
            .navigationTitle(actions.document == nil ? "PdfCraft" : documentName)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItemGroup(placement: .topBarLeading) {
                    Button {
                        if sizeClass == .regular { sidebarVisible.toggle() } else { showingTools = true }
                    } label: {
                        Image(systemName: "sidebar.left")
                    }
                    .accessibilityLabel("All tools")
                    if actions.document != nil {
                        Button("Close") {
                            if actions.document?.dirty == true { confirmClose = true } else { close(discard: false) }
                        }
                        .disabled(actions.busy)
                        .keyboardShortcut("w")
                    }
                }
                ToolbarItemGroup(placement: .topBarTrailing) {
                    if actions.document == nil {
                        Button("Open") { beginOpen() }
                            .disabled(actions.busy)
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
                    if let document = actions.document {
                        Button("Rotate") { rotate() }
                            .disabled(actions.busy || !document.editable)
                            .accessibilityLabel("Rotate page \(selectedPage) clockwise")
                        Spacer()
                        Button("Save a Copy") { save() }
                            .disabled(actions.busy)
                            .keyboardShortcut("s", modifiers: [.command, .shift])
                        Button("Share") { shareDocument() }.disabled(actions.busy)
                    }
                }
            }
            .toolbarBackground(style.panel, for: .navigationBar, .bottomBar)
            .toolbarBackground(.visible, for: .navigationBar, .bottomBar)
            .overlay { if actions.busy { ProgressView("Working locally").padding().background(.regularMaterial).clipShape(RoundedRectangle(cornerRadius: 12)) } }
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
                    if let current = actions.document {
                        actions.document = DocumentMetadata(pageCount: current.pageCount, dirty: false, editable: current.editable,
                                                            pages: current.pages, warnings: current.warnings)
                    }
                case .failure(let error):
                    failure = error.localizedDescription
                }
                export = nil
                finishStagedCopy()
            } onCancellation: {
                export = nil
                finishStagedCopy()
            }
            .sheet(item: $share, onDismiss: finishStagedCopy) { item in ShareSheet(url: item.url) }
            .sheet(item: $textPage) { page in
                PageTextView(page: page.number, bridge: bridge, revision: actions.revision, terminalFailure: handleFailure)
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
                            WorkspaceToolList(query: toolQuery, pinned: pinnedTools, selected: actions.selectedTool,
                                              select: { queuedTool = $0; showingTools = false }, togglePin: togglePin)
                            Text("Tools available in this mobile build. Choose a tool to open a PDF first when needed.")
                                .font(.footnote).foregroundStyle(.secondary)
                        }
                        .padding()
                        .disabled(actions.busy)
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
                if actions.stagedCopy != nil {
                    Button("Retry Cleanup") { failure = nil; finishStagedCopy() }
                        .disabled(actions.releasingCopy)
                    Button("Discard and Close", role: .destructive) { failure = nil; close(discard: true) }
                        .disabled(actions.releasingCopy)
                } else {
                    Button("OK") { failure = nil }
                }
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
                WorkspaceToolList(query: "", pinned: pinnedTools, selected: actions.selectedTool,
                                  select: choose, togglePin: togglePin)
                if let document = actions.document {
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
        .disabled(actions.busy)
        .accessibilityIdentifier("workspace-sidebar")
    }

    @ViewBuilder
    private var workspace: some View {
        if let document = actions.document {
            VStack(spacing: 0) {
                HStack {
                    Text("Page \(selectedPage) of \(document.pageCount)").font(.headline)
                    Spacer()
                    if document.dirty { Text("Unsaved changes").font(.caption).foregroundStyle(.secondary) }
                }
                .padding()
                .background(style.panel)
                if actions.selectedTool == .organize {
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
                                PageView(page: page, bridge: bridge, revision: actions.revision,
                                         selected: selectedPage == page.number, select: { selectedPage = page.number },
                                         terminalFailure: handleFailure)
                                    .id(page.number)
                            }
                        }
                        .padding(.vertical)
                    }
                    .onChange(of: selectedPage) { _, page in reader.scrollTo(page, anchor: .top) }
                }
            }
            .disabled(actions.busy)
        } else {
            WorkspaceHome(pinned: pinnedTools, busy: actions.busy, open: { beginOpen() }, select: choose,
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
        guard !actions.busy else { return }
        guard let document = actions.document else {
            beginOpen(for: tool)
            return
        }
        actions.selectedTool = tool
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
        actions.busy = true
        Task {
            do {
                actions.document = try await bridge.open(url)
                documentName = url.deletingPathExtension().lastPathComponent
                selectedPage = 1
                actions.selectedTool = nil
                actions.revision += 1
                actions.busy = false
                if let tool { choose(tool) }
            } catch {
                handleFailure(error)
            }
        }
    }

    private func rotate() {
        actions.busy = true
        Task {
            do {
                actions.document = try await bridge.rotate(page: selectedPage)
                actions.revision += 1
                actions.busy = false
            } catch { handleFailure(error) }
        }
    }

    private func save() {
        actions.busy = true
        Task {
            do {
                let url = try await bridge.saveCopy()
                actions.stagedCopy = url
                export = Export(url: url)
            } catch {
                handleFailure(error)
            }
        }
    }

    private func finishStagedCopy() {
        Task {
            do { try await actions.releaseCopy { try await bridge.releaseCopy($0) } }
            catch { handleFailure(error) }
        }
    }

    private func close(discard: Bool) {
        actions.busy = true
        Task {
            do {
                try await bridge.close(discard: discard)
                actions.clearDocument()
                actions.busy = false
            } catch {
                handleFailure(error)
            }
        }
    }

    private func shareDocument() {
        actions.busy = true
        Task {
            do {
                let url = try await bridge.saveCopy()
                actions.stagedCopy = url
                share = Export(url: url)
            } catch {
                handleFailure(error)
            }
        }
    }

    private func handleFailure(_ error: Error) {
        actions.failed(error)
        if actions.document == nil { textPage = nil }
        failure = error.localizedDescription
    }
}
