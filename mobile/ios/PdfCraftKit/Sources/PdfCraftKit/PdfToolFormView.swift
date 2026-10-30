import SwiftUI

/// Schema-driven native form renderer for tools in the automation catalogue.
/// Renders dynamic form controls based on the tool's JSON Schema subset and requires
/// user confirmation before running destructive operations.
public struct PdfToolFormView: View {
    public let tool: FfiToolInfo
    public let document: PdfDocument?
    public let engine: PdfEngine
    public let onComplete: ((FfiToolResult) -> Void)?

    @State private var fields: [FfiSchemaField] = []
    @State private var stringValues: [String: String] = [:]
    @State private var boolValues: [String: Bool] = [:]
    @State private var isRunning = false
    @State private var errorMessage: String?
    @State private var showConfirmation = false

    public init(
        tool: FfiToolInfo,
        document: PdfDocument? = nil,
        engine: PdfEngine = PdfEngine(),
        onComplete: ((FfiToolResult) -> Void)? = nil
    ) {
        self.tool = tool
        self.document = document
        self.engine = engine
        self.onComplete = onComplete
    }

    public var body: some View {
        Form {
            Section(header: Text("Description")) {
                Text(tool.description)
                    .font(.subheadline)
                    .foregroundColor(.secondary)

                if tool.destructive {
                    HStack {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .foregroundColor(.orange)
                        Text("This operation modifies or deletes content in the document.")
                            .font(.caption)
                            .foregroundColor(.orange)
                    }
                }
            }

            if !fields.isEmpty {
                Section(header: Text("Parameters")) {
                    ForEach(fields, id: \.name) { field in
                        if field.name != "doc" {
                            fieldRow(for: field)
                        }
                    }
                }
            }

            if let err = errorMessage {
                Section {
                    Text(err)
                        .font(.footnote)
                        .foregroundColor(.red)
                }
            }

            Section {
                Button(action: executeTapped) {
                    HStack {
                        Spacer()
                        if isRunning {
                            ProgressView()
                                .padding(.trailing, 8)
                        }
                        Text(tool.destructive ? "Confirm & Run" : "Run \(tool.title)")
                            .bold()
                            .foregroundColor(tool.destructive ? .red : .accentColor)
                        Spacer()
                    }
                }
                .disabled(isRunning)
            }
        }
        .onAppear(perform: loadSchema)
        .confirmationDialog(
            "Are you sure you want to run \(tool.title)?",
            isPresented: $showConfirmation,
            titleVisibility: .visible
        ) {
            Button("Run \(tool.title)", role: .destructive) {
                runTool()
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This action is destructive and will modify the document.")
        }
    }

    @ViewBuilder
    private func fieldRow(for field: FfiSchemaField) -> some View {
        switch field.fieldType {
        case .string:
            VStack(alignment: .leading, spacing: 4) {
                Text(field.title)
                    .font(.caption)
                    .foregroundColor(.secondary)
                TextField(field.title, text: binding(for: field.name, defaultText: field.defaultValue ?? ""))
                    .textFieldStyle(.roundedBorder)
            }

        case .integer, .number:
            VStack(alignment: .leading, spacing: 4) {
                Text(field.title)
                    .font(.caption)
                    .foregroundColor(.secondary)
                TextField(field.title, text: binding(for: field.name, defaultText: field.defaultValue ?? ""))
                    .textFieldStyle(.roundedBorder)
            }

        case .boolean:
            Toggle(field.title, isOn: boolBinding(for: field.name, defaultValue: field.defaultValue == "true"))

        case .enum:
            Picker(field.title, selection: binding(for: field.name, defaultText: field.enumOptions.first ?? "")) {
                ForEach(field.enumOptions, id: \.self) { opt in
                    Text(opt).tag(opt)
                }
            }

        case .pageRange:
            VStack(alignment: .leading, spacing: 4) {
                Text(field.title)
                    .font(.caption)
                    .foregroundColor(.secondary)
                TextField("e.g. 1-3, 5 (or leave empty for all)", text: binding(for: field.name, defaultText: ""))
                    .textFieldStyle(.roundedBorder)
            }

        case .array, .object:
            VStack(alignment: .leading, spacing: 4) {
                Text(field.title)
                    .font(.caption)
                    .foregroundColor(.secondary)
                TextField("Comma-separated values or JSON", text: binding(for: field.name, defaultText: ""))
                    .textFieldStyle(.roundedBorder)
            }
        }
    }

    private func binding(for key: String, defaultText: String) -> Binding<String> {
        Binding(
            get: { stringValues[key] ?? defaultText },
            set: { stringValues[key] = $0 }
        )
    }

    private func boolBinding(for key: String, defaultValue: Bool) -> Binding<Bool> {
        Binding(
            get: { boolValues[key] ?? defaultValue },
            set: { boolValues[key] = $0 }
        )
    }

    private func defaultPath(for toolName: String) -> String {
        let base = (document?.name as NSString?)?.deletingPathExtension ?? "document"
        let suffix: String
        switch toolName {
        case "doc_reduce": suffix = "_reduced.pdf"
        case "doc_optimize": suffix = "_optimized.pdf"
        case "doc_flatten": suffix = "_flattened.pdf"
        case "export_text": suffix = ".txt"
        default: suffix = "_\(toolName.replacingOccurrences(of: "doc_", with: "")).pdf"
        }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(base + suffix)
        return url.path
    }

    private func loadSchema() {
        if let parsed = try? parseToolSchema(inputSchemaJson: tool.inputSchemaJson) {
            self.fields = parsed
            for f in parsed {
                if let def = f.defaultValue {
                    if f.fieldType == .boolean {
                        boolValues[f.name] = (def == "true")
                    } else {
                        stringValues[f.name] = def
                    }
                } else if f.fieldType == .enum, let first = f.enumOptions.first {
                    stringValues[f.name] = first
                } else if f.name.lowercased().contains("path") {
                    stringValues[f.name] = defaultPath(for: tool.name)
                }
            }
        }
    }

    private func executeTapped() {
        if tool.destructive {
            showConfirmation = true
        } else {
            runTool()
        }
    }

    private func runTool() {
        isRunning = true
        errorMessage = nil

        var argsObj: [String: Any] = [:]
        if let doc = document {
            argsObj["doc"] = doc.docId
        }

        for field in fields {
            guard field.name != "doc" else { continue }
            switch field.fieldType {
            case .boolean:
                if let val = boolValues[field.name] {
                    argsObj[field.name] = val
                }
            case .integer:
                if let str = stringValues[field.name], let intVal = Int(str.trimmingCharacters(in: .whitespaces)) {
                    argsObj[field.name] = intVal
                }
            case .number:
                if let str = stringValues[field.name], let doubleVal = Double(str.trimmingCharacters(in: .whitespaces)) {
                    argsObj[field.name] = doubleVal
                }
            case .enum, .string, .pageRange:
                if let str = stringValues[field.name], !str.isEmpty {
                    argsObj[field.name] = str
                }
            case .array:
                if let str = stringValues[field.name], !str.isEmpty {
                    let items = str.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
                    argsObj[field.name] = items
                }
            case .object:
                if let str = stringValues[field.name], let data = str.data(using: .utf8),
                   let obj = try? JSONSerialization.jsonObject(with: data) {
                    argsObj[field.name] = obj
                }
            }
        }

        do {
            let data = try JSONSerialization.data(withJSONObject: argsObj, options: [])
            let jsonString = String(data: data, encoding: .utf8) ?? "{}"
            let result: FfiToolResult
            if let doc = document {
                result = try doc.invokeTool(name: tool.name, argsJson: jsonString)
            } else {
                result = try engine.invokeTool(name: tool.name, argsJson: jsonString)
            }
            isRunning = false
            if !result.success, let err = result.error {
                self.errorMessage = err
            } else {
                onComplete?(result)
            }
        } catch {
            isRunning = false
            self.errorMessage = error.localizedDescription
        }
    }
}
