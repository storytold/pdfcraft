import { test } from "node:test";
import assert from "node:assert/strict";

// Import modules
import { PdfCraftLocalClient } from "../dist/local.js";
import { PdfCraftRestClient } from "../dist/rest.js";
import { InvalidArgumentError, PdfCraftError } from "../dist/errors.js";

test("LocalClient merge throws PdfCraftError when native addon is unavailable", async () => {
  const local = new PdfCraftLocalClient();
  const doc1 = new TextEncoder().encode("%PDF-1.4 sample 1");
  const doc2 = new TextEncoder().encode("%PDF-1.4 sample 2");

  await assert.rejects(async () => {
    await local.merge([doc1, doc2]);
  }, (err) => {
    assert.equal(err instanceof PdfCraftError, true);
    assert.equal(err.errorCode, "NATIVE_UNAVAILABLE");
    assert.equal(err.statusCode, 501);
    return true;
  });
});

test("LocalClient empty sources throws InvalidArgumentError", async () => {
  const local = new PdfCraftLocalClient();
  await assert.rejects(async () => {
    await local.merge([]);
  }, InvalidArgumentError);
});

test("LocalClient split throws PdfCraftError when native addon is unavailable", async () => {
  const local = new PdfCraftLocalClient();
  await assert.rejects(async () => {
    await local.split("doc.pdf");
  }, (err) => {
    assert.equal(err instanceof PdfCraftError, true);
    assert.equal(err.errorCode, "NATIVE_UNAVAILABLE");
    return true;
  });
});

test("LocalClient renderPage validates page >= 1 and checks native availability", async () => {
  const local = new PdfCraftLocalClient();
  await assert.rejects(async () => {
    await local.renderPage("doc.pdf", 0);
  }, InvalidArgumentError);

  await assert.rejects(async () => {
    await local.renderPage("doc.pdf", 1, { dpi: 300 });
  }, (err) => {
    assert.equal(err instanceof PdfCraftError, true);
    assert.equal(err.errorCode, "NATIVE_UNAVAILABLE");
    return true;
  });
});

test("RestClient maps RFC 7807 error details", async () => {
  const rest = new PdfCraftRestClient({ baseUrl: "http://127.0.0.1:9999" });

  assert.throws(() => {
    rest.handleErrorResponse(
      new TextEncoder().encode(
        JSON.stringify({ status: 400, detail: "Bad param", code: "BAD_REQUEST" })
      ),
      400
    );
  }, InvalidArgumentError);

  assert.throws(() => {
    rest.handleErrorResponse(
      new TextEncoder().encode(
        JSON.stringify({ status: 413, detail: "Limit exceeded", code: "QUOTA_EXCEEDED" })
      ),
      413
    );
  }, (err) => {
    assert.equal(err.statusCode, 413);
    assert.equal(err.errorCode, "QUOTA_EXCEEDED");
    return true;
  });
});
