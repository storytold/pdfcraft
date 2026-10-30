import SwiftUI
import CoreGraphics
import PdfCraftKit

public struct ContentView: View {
    @State private var document: PdfDocument?
    @State private var statusText: String = "Loading..."
    @State private var pageCount: Int = 0

    public init() {}

    public var body: some View {
        VStack(spacing: 12) {
            Text("PdfCraft Mobile Viewer")
                .font(.headline)

            Text(statusText)
                .font(.subheadline)
                .foregroundColor(.secondary)

            if let doc = document {
                PdfView(document: doc, pageIndex: 0)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color.gray.opacity(0.3), lineWidth: 1))
                    .shadow(radius: 2)
            } else {
                ProgressView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .padding()
        .task {
            loadDocument()
        }
    }

    private func loadDocument() {
        do {
            let pdfString = """
            %PDF-1.7
            1 0 obj
            << /Type /Catalog /Pages 2 0 R >>
            endobj
            2 0 obj
            << /Type /Pages /Kids [3 0 R] /Count 1 >>
            endobj
            3 0 obj
            << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>
            endobj
            xref
            0 4
            0000000000 65535 f 
            0000000009 00000 n 
            0000000058 00000 n 
            0000000115 00000 n 
            trailer
            << /Size 4 /Root 1 0 R >>
            startxref
            193
            %%EOF
            """
            guard let data = pdfString.data(using: .utf8) else {
                statusText = "Failed to encode PDF data"
                return
            }

            let doc = try PdfDocument(name: "viewer_smoke.pdf", data: data)
            self.document = doc
            self.pageCount = doc.pageCount
            statusText = "Document loaded (1 page) - pinch, pan, or double-tap to zoom"
        } catch {
            statusText = "Error: \(error)"
        }
    }
}
