"""Verification test suite for PdfCraft Python SDK (Task 5.1)."""

import unittest
from pathlib import Path

from pdfcraft.errors import InvalidArgumentError, PdfCraftError
from pdfcraft.local import LocalClient
from pdfcraft.rest import RestClient
from pdfcraft.types import DocumentSource, MergeOptions, RenderOptions

MINIMAL_VALID_PDF = (
    b"%PDF-1.4\n"
    b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"
    b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n"
    b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n"
    b"trailer\n<< /Root 1 0 R >>\n%%EOF\n"
)


class TestPdfCraftPythonSdk(unittest.TestCase):
    def setUp(self):
        self.local = LocalClient()
        self.rest = RestClient(base_url="http://127.0.0.1:9999")

    def test_local_merge_in_memory(self):
        doc1 = MINIMAL_VALID_PDF
        doc2 = MINIMAL_VALID_PDF
        res = self.local.merge([doc1, doc2])
        self.assertTrue(res.is_memory)
        self.assertEqual(res.mime_type, "application/pdf")
        self.assertTrue(res.data.startswith(b"%PDF-"))
        self.assertGreater(len(res.data), 0)

    def test_local_empty_merge_raises_invalid_argument(self):
        with self.assertRaises(InvalidArgumentError):
            self.local.merge([])

    def test_local_split(self):
        res = self.local.split(MINIMAL_VALID_PDF, every_n_pages=1)
        self.assertGreaterEqual(len(res.files), 1)
        self.assertTrue(all(isinstance(f, Path) for f in res.files))
        self.assertTrue(all(f.exists() for f in res.files))

    def test_local_render_page_dpi(self):
        res = self.local.render_page(MINIMAL_VALID_PDF, 1, RenderOptions(dpi=150.0))
        self.assertTrue(res.is_memory)
        self.assertEqual(res.mime_type, "image/png")
        # Real PNG starts with 8-byte PNG signature: \x89PNG\r\n\x1a\n
        self.assertTrue(res.data.startswith(b"\x89PNG\r\n\x1a\n"))

    def test_local_render_page_zero_rejected(self):
        with self.assertRaises(InvalidArgumentError):
            self.local.render_page(MINIMAL_VALID_PDF, 0)

    def test_local_call_tool_native_execution(self):
        tools = self.local.call_tool("command_list", {})
        self.assertGreater(len(tools), 0)
        self.assertEqual(tools[0]["type"], "json")

    def test_rest_problem_details_mapping(self):
        # Verify 400 Bad Request mapping
        with self.assertRaises(InvalidArgumentError):
            self.rest._handle_error_response(b'{"status": 400, "detail": "Invalid page", "code": "BAD_REQUEST"}', 400)

        # Verify 413 Quota Exceeded mapping
        with self.assertRaises(PdfCraftError) as ctx:
            self.rest._handle_error_response(b'{"status": 413, "detail": "Limit reached", "code": "QUOTA_EXCEEDED"}', 413)
        self.assertEqual(ctx.exception.status_code, 413)

    def test_local_execute_batch(self):
        report = self.local.execute_batch([
            {"tool": "command_list", "args": {}},
        ])
        self.assertTrue(report.get("ok"))
        self.assertEqual(len(report.get("steps", [])), 1)
        self.assertEqual(report["steps"][0]["status"], "ok")

    def test_local_split_modes(self):
        # Test individual modes
        res1 = self.local.split(MINIMAL_VALID_PDF, every_n_pages=1)
        self.assertGreaterEqual(len(res1.files), 1)

        res2 = self.local.split(MINIMAL_VALID_PDF, before_pages=[1])
        self.assertGreaterEqual(len(res2.files), 1)

        # at_bookmarks=True on document without bookmarks correctly fails with informative engine error
        with self.assertRaises(ValueError) as ctx:
            self.local.split(MINIMAL_VALID_PDF, at_bookmarks=True)
        self.assertIn("no top-level bookmarks", str(ctx.exception))

        res4 = self.local.split(MINIMAL_VALID_PDF, max_file_size_mb=10.0)
        self.assertGreaterEqual(len(res4.files), 1)

        # Explicit at_bookmarks=False does not conflict
        res5 = self.local.split(MINIMAL_VALID_PDF, every_n_pages=1, at_bookmarks=False)
        self.assertGreaterEqual(len(res5.files), 1)

        # Test all 6 conflicting pairs
        # 1. every_n_pages + before_pages
        with self.assertRaises(ValueError):
            self.local.split(MINIMAL_VALID_PDF, every_n_pages=1, before_pages=[1])

        # 2. every_n_pages + at_bookmarks
        with self.assertRaises(ValueError):
            self.local.split(MINIMAL_VALID_PDF, every_n_pages=1, at_bookmarks=True)

        # 3. every_n_pages + max_file_size_mb
        with self.assertRaises(ValueError):
            self.local.split(MINIMAL_VALID_PDF, every_n_pages=1, max_file_size_mb=5.0)

        # 4. before_pages + at_bookmarks
        with self.assertRaises(ValueError):
            self.local.split(MINIMAL_VALID_PDF, before_pages=[1], at_bookmarks=True)

        # 5. before_pages + max_file_size_mb
        with self.assertRaises(ValueError):
            self.local.split(MINIMAL_VALID_PDF, before_pages=[1], max_file_size_mb=5.0)

        # 6. at_bookmarks + max_file_size_mb
        with self.assertRaises(ValueError):
            self.local.split(MINIMAL_VALID_PDF, at_bookmarks=True, max_file_size_mb=5.0)

    def test_local_error_mapping_to_pdfcraft_error_hierarchy(self):
        # Local client errors must be catchable as PdfCraftError and have error_code
        with self.assertRaises(PdfCraftError) as ctx:
            self.local.split(MINIMAL_VALID_PDF, at_bookmarks=True)
        self.assertEqual(ctx.exception.error_code, "ENGINE_ERROR")
        self.assertIsInstance(ctx.exception, ValueError)

        with self.assertRaises(InvalidArgumentError) as ctx2:
            self.local.merge([])
        self.assertEqual(ctx2.exception.error_code, "INVALID_ARGUMENT")
        self.assertIsInstance(ctx2.exception, PdfCraftError)
        self.assertIsInstance(ctx2.exception, ValueError)


if __name__ == "__main__":
    unittest.main()
