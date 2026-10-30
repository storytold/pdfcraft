import SwiftUI

/// Command palette view providing search and rapid access across all tools in the automation catalogue.
public struct PdfCommandPaletteView: View {
    public let document: PdfDocument?
    public let engine: PdfEngine
    public let onSelectTool: ((FfiToolInfo) -> Void)?

    @State private var allTools: [FfiToolInfo] = []
    @State private var searchText: String = ""
    @State private var selectedTool: FfiToolInfo?

    public init(
        document: PdfDocument? = nil,
        engine: PdfEngine = PdfEngine(),
        onSelectTool: ((FfiToolInfo) -> Void)? = nil
    ) {
        self.document = document
        self.engine = engine
        self.onSelectTool = onSelectTool
    }

    private var filteredTools: [FfiToolInfo] {
        if searchText.trimmingCharacters(in: .whitespaces).isEmpty {
            return allTools
        }
        let query = searchText.lowercased()
        return allTools.filter {
            $0.title.lowercased().contains(query)
                || $0.name.lowercased().contains(query)
                || $0.description.lowercased().contains(query)
        }
    }

    public var body: some View {
        NavigationView {
            List {
                ForEach(filteredTools, id: \.name) { tool in
                    Button(action: { handleToolTapped(tool) }) {
                        VStack(alignment: .leading, spacing: 4) {
                            HStack {
                                Text(tool.title)
                                    .font(.headline)
                                    .foregroundColor(.primary)

                                Spacer()

                                badge(for: tool)
                            }

                            Text(tool.description)
                                .font(.caption)
                                .foregroundColor(.secondary)
                                .lineLimit(2)

                            HStack(spacing: 6) {
                                Text(tool.name)
                                    .font(.system(.caption2, design: .monospaced))
                                    .foregroundColor(.secondary)

                                if tool.readOnly {
                                    Text("Read-only")
                                        .font(.caption2)
                                        .padding(.horizontal, 4)
                                        .background(Color.blue.opacity(0.15))
                                        .foregroundColor(.blue)
                                        .cornerRadius(4)
                                }

                                if tool.destructive {
                                    Text("Destructive")
                                        .font(.caption2)
                                        .padding(.horizontal, 4)
                                        .background(Color.red.opacity(0.15))
                                        .foregroundColor(.red)
                                        .cornerRadius(4)
                                }
                            }
                        }
                        .padding(.vertical, 4)
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel("\(tool.title)")
                    .accessibilityHint("\(tool.description)")
                }
            }
            .searchable(text: $searchText, prompt: "Search commands and tools...")
            .navigationTitle("Command Palette")
            .onAppear {
                self.allTools = listTools()
            }
            .sheet(item: $selectedTool) { tool in
                NavigationView {
                    PdfToolFormView(tool: tool, document: document, engine: engine) { _ in
                        selectedTool = nil
                    }
                    .navigationTitle(tool.title)
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button("Close") { selectedTool = nil }
                        }
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func badge(for tool: FfiToolInfo) -> some View {
        switch tool.classification {
        case .schemaDriven:
            Text("Tool")
                .font(.caption2)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(Color.accentColor.opacity(0.15))
                .foregroundColor(.accentColor)
                .cornerRadius(4)
        case .bespokeScreen:
            Text("Action")
                .font(.caption2)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(Color.green.opacity(0.15))
                .foregroundColor(.green)
                .cornerRadius(4)
        case .notOnMobile:
            Text("Desktop")
                .font(.caption2)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(Color.secondary.opacity(0.15))
                .foregroundColor(.secondary)
                .cornerRadius(4)
        }
    }

    private func handleToolTapped(_ tool: FfiToolInfo) {
        if let callback = onSelectTool {
            callback(tool)
        } else if tool.classification == .schemaDriven {
            selectedTool = tool
        }
    }
}

extension FfiToolInfo: @retroactive Identifiable {
    public var id: String { name }
}
