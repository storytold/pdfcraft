import SwiftUI

/// A native SwiftUI interface for document redactions with audit verification
/// and an explicit irreversible-action confirmation dialog (MOB-6.2).
public struct PdfRedactionView: View {
    public let document: PdfDocument
    public let pageIndex: Int

    @State private var searchText: String = ""
    @State private var overlayText: String = "REDACTED"
    @State private var showConfirmApply: Bool = false
    @State private var auditFindings: [FfiRedactionFinding] = []
    @State private var auditStatusMessage: String?
    @State private var isAuditPassing: Bool = true
    @State private var errorMessage: String?
    @State private var isProcessing: Bool = false

    public init(document: PdfDocument, pageIndex: Int = 0) {
        self.document = document
        self.pageIndex = pageIndex
    }

    public var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                Label("Redact Content", systemImage: "eye.slash.fill")
                    .font(.headline)
                Spacer()
                Button("Run Audit") {
                    performAudit()
                }
                .font(.subheadline)
            }
            .padding()
            .background(Color.secondary.opacity(0.1))

            // Status / Error Banner
            if let err = errorMessage {
                Text(err)
                    .font(.caption)
                    .foregroundColor(.red)
                    .padding(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.red.opacity(0.1))
            } else if let status = auditStatusMessage {
                HStack {
                    Image(systemName: isAuditPassing ? "checkmark.shield.fill" : "exclamationmark.shield.fill")
                        .foregroundColor(isAuditPassing ? .green : .orange)
                    Text(status)
                        .font(.caption)
                        .foregroundColor(isAuditPassing ? .green : .orange)
                }
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background((isAuditPassing ? Color.green : Color.orange).opacity(0.1))
            }

            // Controls
            Form {
                Section(header: Text("Mark for Redaction")) {
                    TextField("Text to redact", text: $searchText)

                    TextField("Overlay label (optional)", text: $overlayText)

                    Button(action: markText) {
                        Label("Mark Matching Text", systemImage: "highlighter")
                    }
                    .disabled(searchText.trimmingCharacters(in: .whitespaces).isEmpty || isProcessing)
                }

                Section(header: Text("Audit & Review")) {
                    if auditFindings.isEmpty {
                        Text("No pending marks or faux redactions detected.")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    } else {
                        ForEach(auditFindings.indices, id: \.self) { i in
                            let finding = auditFindings[i]
                            VStack(alignment: .leading, spacing: 4) {
                                Text("Page \(finding.page) • \(finding.kind)")
                                    .font(.caption.bold())
                                if !finding.coveredText.isEmpty {
                                    Text("\"\(finding.coveredText)\"")
                                        .font(.caption)
                                        .foregroundColor(.red)
                                }
                            }
                            .padding(.vertical, 2)
                        }
                    }

                    Button(role: .destructive, action: clearMarks) {
                        Label("Clear Pending Marks", systemImage: "trash")
                    }
                    .disabled(isProcessing)
                }

                Section {
                    Button(action: { showConfirmApply = true }) {
                        HStack {
                            Spacer()
                            Label("Apply Redactions (Permanent)", systemImage: "lock.shield.fill")
                                .bold()
                                .foregroundColor(.white)
                            Spacer()
                        }
                    }
                    .listRowBackground(Color.red)
                    .disabled(isProcessing)
                }
            }
        }
        .onAppear {
            performAudit()
        }
        .confirmationDialog(
            "Permanent Redaction Warning",
            isPresented: $showConfirmApply,
            titleVisibility: .visible
        ) {
            Button("Permanently Apply Redactions", role: .destructive) {
                applyRedactions()
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Applying redactions permanently removes all covered text, images, and vector artwork from the file content. Once saved, this content cannot be recovered.")
        }
    }

    private func markText() {
        guard !searchText.trimmingCharacters(in: .whitespaces).isEmpty else { return }
        isProcessing = true
        errorMessage = nil
        do {
            let count = try document.markRedaction(text: searchText, overlayText: overlayText.isEmpty ? nil : overlayText)
            auditStatusMessage = "Marked \(count) occurrence(s) for redaction. Run audit or apply."
            isAuditPassing = false
            performAudit()
        } catch {
            errorMessage = error.localizedDescription
        }
        isProcessing = false
    }

    private func clearMarks() {
        isProcessing = true
        errorMessage = nil
        do {
            try document.clearRedactions()
            auditStatusMessage = "Pending redaction marks cleared."
            performAudit()
        } catch {
            errorMessage = error.localizedDescription
        }
        isProcessing = false
    }

    private func performAudit() {
        do {
            let findings = try document.auditRedactions()
            self.auditFindings = findings
            let unapplied = findings.filter { $0.kind == "unapplied-mark" }.count
            let covered = findings.filter { $0.kind == "covered-text" }.count
            if findings.isEmpty {
                self.isAuditPassing = true
                self.auditStatusMessage = "Audit Passed: Document contains 0 faux redactions or unapplied marks."
            } else {
                self.isAuditPassing = false
                self.auditStatusMessage = "Audit: \(unapplied) unapplied mark(s), \(covered) covered text item(s) detected."
            }
        } catch {
            self.errorMessage = error.localizedDescription
        }
    }

    private func applyRedactions() {
        isProcessing = true
        errorMessage = nil
        do {
            let res = try document.applyRedactions()
            self.auditFindings = res.auditFindings
            self.isAuditPassing = res.auditPassed
            if res.auditPassed {
                self.auditStatusMessage = "Redactions applied successfully. Security audit verified 0 unredacted text items."
            } else {
                self.auditStatusMessage = "Warning: Redactions applied but \(res.auditFindings.count) item(s) flagged by security audit."
            }
        } catch {
            self.errorMessage = error.localizedDescription
        }
        isProcessing = false
    }
}
