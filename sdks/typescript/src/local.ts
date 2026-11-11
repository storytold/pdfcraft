/**
 * In-process Local SDK Client for PdfCraft (Issue #872).
 *
 * # Architecture Reference
 * Module: sdks/typescript/src/local.ts
 * Purpose:
 *   In-process document processing client linking to pdfcraft-engine via N-API addon,
 *   providing zero-network, zero-daemon PDF processing (PDFL alternative).
 */

declare const process: any;
import { InvalidArgumentError, PdfCraftError } from "./errors.js";
import { DocumentResult, DocumentSource, MergeOptions, RenderOptions, SplitMode, SplitResult } from "./types.js";

export class PdfCraftLocalClient {
  private nativeBinding: any = null;

  constructor() {
    try {
      // Attempt loading compiled N-API native module if present
      this.nativeBinding = (process as any)._linkedBinding?.("pdfcraft_napi");
    } catch {
      this.nativeBinding = null;
    }
  }

  private requireBinding(): any {
    if (!this.nativeBinding) {
      throw new PdfCraftError(
        "pdfcraft native addon is not available; local processing is unsupported in this runtime (build N-API addon or use PdfCraftRestClient)",
        501,
        "NATIVE_UNAVAILABLE"
      );
    }
    return this.nativeBinding;
  }

  /**
   * Merges multiple PDF documents into a single document in-process.
   */
  async merge(
    sources: (DocumentSource | string | Uint8Array)[],
    options?: MergeOptions
  ): Promise<DocumentResult> {
    if (!sources || sources.length === 0) {
      throw new InvalidArgumentError("Cannot merge zero files");
    }

    return this.requireBinding().merge(sources, options);
  }

  /**
   * Splits a PDF document into multiple documents.
   */
  async split(
    source: DocumentSource | string | Uint8Array,
    mode?: SplitMode
  ): Promise<SplitResult> {
    return this.requireBinding().split(source, mode);
  }

  /**
   * Renders a single page of a PDF document to an image at the specified physical DPI.
   */
  async renderPage(
    source: DocumentSource | string | Uint8Array,
    page: number,
    options?: RenderOptions
  ): Promise<DocumentResult> {
    if (page < 1) {
      throw new InvalidArgumentError("Page index must be >= 1 (1-based index)");
    }

    return this.requireBinding().renderPage(source, page, options);
  }
}
