# PdfCraft SDK API Reference Manual

> [!IMPORTANT]
> The full, canonical domain-by-domain API reference manual is located at:
> 
> 📖 **[Canonical Domain-by-Domain API Reference Manual](sdk/api-reference.md)**

---

## 📚 Domains Covered in the API Reference

| Domain | APDFL Parity | Summary & Operations | Direct Link |
| :--- | :--- | :--- | :--- |
| **Document Lifecycle** | `PDDoc` | Open, save, close, inspect metadata, page count | [Document Lifecycle](sdk/api-reference.md#1-document-lifecycle--metadata) |
| **Organize Pages** | `PDPage` | Insert, delete, rotate, extract, reorder pages | [Organize Pages](sdk/api-reference.md#2-page-manipulation--organization) |
| **Bookmarks & Outline** | `PDBookmark` | Hierarchical outline read/write, depth traversal, pagination | [Bookmarks](sdk/api-reference.md#3-bookmarks--outline) |
| **Annotations** | `PDAnnot` | Highlight, Underline, Stamp, Sticky Note creation & query | [Annotations](sdk/api-reference.md#4-annotations) |
| **AcroForms & Data** | `AcroForm` | Field value read/write, widget styling, form flattening, XFDF | [AcroForms](sdk/api-reference.md#5-acroforms--data-exchange) |
| **Security & Signatures** | `PDCrypt` / `PDSign` | Password encryption, PKCS#12 signing, certificate verification | [Security & Signatures](sdk/api-reference.md#6-security--digital-signatures) |
| **High-Fidelity Rendering** | `PDPageDraw` | Screen previews, physical print rasters, decoupled DPI | [Rendering](sdk/api-reference.md#7-page-rendering--previews) |
| **Generic Automation** | Extensible | Call any of the 123 automation tools via generic batch execution | [Automation Bridge](sdk/api-reference.md#8-generic-automation-bridge) |

---

## 🔗 Related Resources

- **[Canonical SDK Developer Guide](sdk/README.md)**: Full architecture details, enterprise deployment guidelines, and multi-language tutorials.
- **[Control Planes Blueprint](sdk/control-planes.md)**: Control plane topology and MCP vs SDK comparisons.
- **[Runnable CLI Code Examples](sdk/examples/)**: Standalone executable scripts for Rust, Python, and TypeScript.
- **[OpenAPI 3.1 Specification](../sdks/openapi.json)**: Machine-readable REST API contracts.
- **[Main Project README](../README.md#⚡-fork-overview-enterprise-sdk--rest-api)**: Executive overview and quick-start snippets.
