import SwiftUI

// Keep this mobile capability list aligned with engine/catalog.rs and engine/commands.rs.
enum WorkspaceTool: String, CaseIterable, Identifiable {
    case organize = "page.organize"
    case readText = "mobile.read_text"
    case saveCopy = "mobile.save_copy"
    case share = "mobile.share"

    var id: String { rawValue }

    var title: String {
        switch self {
        case .organize: "Organize pages"
        case .readText: "Read Text"
        case .saveCopy: "Save a Copy"
        case .share: "Share"
        }
    }

    var detail: String {
        switch self {
        case .organize: "Select a page and rotate it clockwise."
        case .readText: "Read and copy a page's embedded text."
        case .saveCopy: "Save a separate PDF through Files."
        case .share: "Share a current PDF copy with another app."
        }
    }

    var symbol: String {
        switch self {
        case .organize: "square.grid.2x2"
        case .readText: "text.alignleft"
        case .saveCopy: "doc.on.doc"
        case .share: "square.and.arrow.up"
        }
    }

    var tint: Color {
        switch self {
        case .organize: Color(red: 0x2D / 255.0, green: 0x9D / 255.0, blue: 0x5B / 255.0)
        case .readText: .purple
        case .saveCopy: .blue
        case .share: .teal
        }
    }

    func matches(_ query: String) -> Bool {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        return query.isEmpty || "\(title) \(detail)".localizedStandardContains(query)
    }
}

enum WorkspaceAppearance: String, CaseIterable, Identifiable {
    case system, light, dark
    var id: String { rawValue }
    var title: String { rawValue.capitalized }
    var colorScheme: ColorScheme? {
        switch self {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }
}

// A small native counterpart of ui-egui/theme.rs; PDF pages themselves stay white.
struct WorkspaceStyle {
    let scheme: ColorScheme
    var background: Color { color(light: 0xF1F1F3, dark: 0x19191C) }
    var panel: Color { color(light: 0xFFFFFF, dark: 0x26262A) }
    var card: Color { color(light: 0xFFFFFF, dark: 0x2E2E33) }
    var border: Color { color(light: 0xDADADE, dark: 0x3C3C43) }
    var accent: Color { color(light: 0x1B63E0, dark: 0x4B8BF5) }

    private func color(light: UInt32, dark: UInt32) -> Color {
        let rgb = scheme == .dark ? dark : light
        return Color(red: Double((rgb >> 16) & 255) / 255,
                     green: Double((rgb >> 8) & 255) / 255,
                     blue: Double(rgb & 255) / 255)
    }
}

struct WorkspaceHome: View {
    let pinned: Set<WorkspaceTool>
    let busy: Bool
    let open: () -> Void
    let select: (WorkspaceTool) -> Void
    let togglePin: (WorkspaceTool) -> Void
    let showTools: () -> Void
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let style = WorkspaceStyle(scheme: scheme)
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Welcome to PdfCraft")
                        .font(.largeTitle.bold())
                        .accessibilityAddTraits(.isHeader)
                    Text("An open-source PDF workbench - local and private.")
                        .foregroundStyle(.secondary)
                }

                VStack(alignment: .leading, spacing: 16) {
                    Button(action: open) {
                        Label("Open file", systemImage: "folder")
                            .font(.headline)
                            .frame(minHeight: 44)
                            .padding(.horizontal)
                    }
                    .buttonStyle(.borderedProminent)
                    .accessibilityIdentifier("Open PDF")
                    Text("Choose a PDF in Files. If you choose a tool first, Files opens before that tool runs.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                    Text("Recommended tools")
                        .font(.headline)
                        .accessibilityAddTraits(.isHeader)
                    toolGrid(WorkspaceTool.allCases.filter { !pinned.contains($0) })
                }
                .padding(20)
                .background(style.panel, in: RoundedRectangle(cornerRadius: 12))

                VStack(alignment: .leading, spacing: 12) {
                    HStack {
                        Text("Pinned Tools")
                            .font(.headline)
                            .accessibilityAddTraits(.isHeader)
                        Spacer()
                        Button(action: showTools) {
                            Label("Pin a tool", systemImage: "pin")
                        }
                        .frame(minHeight: 44)
                    }
                    if pinned.isEmpty {
                        Text("Pin your favorite tools here for quick access.")
                            .foregroundStyle(.secondary)
                    } else {
                        toolGrid(WorkspaceTool.allCases.filter { pinned.contains($0) })
                    }
                }
                .accessibilityIdentifier("pinned-tools")

                VStack(alignment: .leading, spacing: 8) {
                    Text("Your files stay yours").font(.headline)
                    Text("Use Files to browse recent documents, local folders, and your chosen providers. Originals are never overwritten.")
                        .foregroundStyle(.secondary)
                    Text("Experimental iPhone and iPad build. Not an official ArtCraft release. Only tools supported by this mobile build are shown.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
            .padding(24)
            .frame(maxWidth: 1100, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .background(style.background)
        .disabled(busy)
    }

    private func toolGrid(_ tools: [WorkspaceTool]) -> some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 200), spacing: 12)], spacing: 12) {
            ForEach(tools) { tool in
                WorkspaceToolCard(tool: tool, pinned: pinned.contains(tool),
                                  select: { select(tool) }, togglePin: { togglePin(tool) })
            }
        }
    }
}

private struct WorkspaceToolCard: View {
    let tool: WorkspaceTool
    let pinned: Bool
    let select: () -> Void
    let togglePin: () -> Void
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let style = WorkspaceStyle(scheme: scheme)
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Image(systemName: tool.symbol)
                    .font(.title2)
                    .foregroundStyle(tool.tint)
                    .accessibilityHidden(true)
                Spacer()
                ToolPinButton(tool: tool, pinned: pinned, toggle: togglePin)
            }
            Button(action: select) {
                VStack(alignment: .leading, spacing: 8) {
                    Text(tool.title).font(.headline)
                    Text(tool.detail).font(.callout).foregroundStyle(.secondary)
                    Text("Use now").font(.callout.weight(.semibold)).foregroundStyle(style.accent)
                }
                .frame(maxWidth: .infinity, minHeight: 100, alignment: .topLeading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(tool.title)
            .accessibilityHint(tool.detail)
            .accessibilityIdentifier("home-tool-\(tool.rawValue)")
        }
        .padding(16)
        .background(style.card, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(style.border))
    }
}

struct WorkspaceToolList: View {
    let query: String
    let pinned: Set<WorkspaceTool>
    let selected: WorkspaceTool?
    let select: (WorkspaceTool) -> Void
    let togglePin: (WorkspaceTool) -> Void

    var body: some View {
        let tools = WorkspaceTool.allCases.filter { $0.matches(query) }
        VStack(alignment: .leading, spacing: 4) {
            if tools.isEmpty {
                Text("No tools match your search.")
                    .foregroundStyle(.secondary)
                    .padding()
            }
            ForEach(tools) { tool in
                HStack(spacing: 4) {
                    Button { select(tool) } label: {
                        Label(tool.title, systemImage: tool.symbol)
                            .foregroundStyle(selected == tool ? Color.accentColor : .primary)
                            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("tool-\(tool.rawValue)")
                    .accessibilityAddTraits(selected == tool ? .isSelected : [])
                    ToolPinButton(tool: tool, pinned: pinned.contains(tool)) { togglePin(tool) }
                }
            }
        }
    }
}

private struct ToolPinButton: View {
    let tool: WorkspaceTool
    let pinned: Bool
    let toggle: () -> Void

    var body: some View {
        Button(action: toggle) {
            Image(systemName: pinned ? "pin.fill" : "pin")
                .frame(minWidth: 44, minHeight: 44)
        }
        .buttonStyle(.plain)
        .foregroundStyle(pinned ? Color.accentColor : .secondary)
        .accessibilityLabel("\(pinned ? "Unpin" : "Pin") \(tool.title) \(pinned ? "from" : "to") Home")
    }
}
