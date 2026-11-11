"""In-process Local SDK Client for PdfCraft (Issue #872).

# Architecture Reference
Module: sdks/python/pdfcraft/local.py
Purpose:
    Direct in-process execution linking to pdfcraft-engine via PyO3 native module,
    providing zero-network, zero-daemon PDF processing (PDFL alternative).
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import List, Optional, Union

from pdfcraft.errors import (
    ConflictError,
    InvalidArgumentError,
    NotFoundError,
    PdfCraftError,
    QuotaExceededError,
)
from pdfcraft.types import DocumentResult, DocumentSource, MergeOptions, RenderOptions, SplitResult

try:
    from pdfcraft._native import NativeLocalClient  # type: ignore
except ImportError:
    NativeLocalClient = None  # type: ignore


def _call(fn, *args, **kwargs):
    """Executes a native call, mapping native error tuples to structured PdfCraftErrors."""
    try:
        return fn(*args, **kwargs)
    except (ValueError, IOError) as e:
        if len(e.args) == 2 and isinstance(e.args[0], str):
            code, msg = e.args[0], e.args[1]
            if code == "INVALID_ARGUMENT":
                raise InvalidArgumentError(msg) from e
            elif code == "QUOTA_EXCEEDED":
                raise QuotaExceededError(msg) from e
            elif code == "NOT_FOUND":
                raise NotFoundError(msg) from e
            elif code == "CONFLICT":
                raise ConflictError(msg) from e
            raise PdfCraftError(msg, status_code=500, error_code=code) from e
        raise e


class LocalClient:
    """In-process document processing client linking directly to the Rust engine."""

    def __init__(self) -> None:
        """Initializes the LocalClient using PyO3 native bindings."""
        if NativeLocalClient is None:
            raise ImportError(
                "PdfCraft LocalClient requires the compiled pdfcraft._native extension. "
                "Build it with `cargo build -p pdfcraft-pyo3` or `maturin develop`."
            )
        self._native = NativeLocalClient()

    def merge(
        self,
        sources: List[Union[str, Path, bytes, DocumentSource]],
        options: Optional[MergeOptions] = None,
    ) -> DocumentResult:
        """Merges multiple PDF documents into a single document."""
        if not sources:
            raise InvalidArgumentError("Cannot merge zero files")

        parsed_sources: List[DocumentSource] = []
        for s in sources:
            if isinstance(s, DocumentSource):
                parsed_sources.append(s)
            elif isinstance(s, bytes):
                parsed_sources.append(DocumentSource.from_bytes(s))
            else:
                parsed_sources.append(DocumentSource.from_path(str(s)))

        opts = options or MergeOptions()
        native_items = []
        for src in parsed_sources:
            if src.data is not None:
                native_items.append(src.data)
            elif src.path is not None:
                native_items.append(str(src.path))
        res = _call(
            self._native.merge,
            native_items,
            opts.output_filename,
            getattr(opts, "pages", None),
            getattr(opts, "passwords", None),
        )
        if res.get("type") == "memory":
            return DocumentResult(data=res.get("data"), mime_type=res.get("mime_type", "application/pdf"))
        return DocumentResult(
            path=Path(res["path"]), mime_type=res.get("mime_type", "application/pdf"), size_bytes=res.get("size_bytes", 0)
        )

    def split(
        self,
        source: Union[str, Path, bytes, DocumentSource],
        every_n_pages: Optional[int] = None,
        before_pages: Optional[list] = None,
        at_bookmarks: Optional[bool] = None,
        max_file_size_mb: Optional[float] = None,
    ) -> SplitResult:
        """Splits a PDF document into multiple documents."""
        if isinstance(source, DocumentSource):
            src = source
        elif isinstance(source, bytes):
            src = DocumentSource.from_bytes(source)
        else:
            src = DocumentSource.from_path(str(source))

        arg = src.data if src.data is not None else str(src.path)
        paths = _call(self._native.split, arg, every_n_pages, before_pages, at_bookmarks, max_file_size_mb)
        return SplitResult(files=[Path(p) for p in paths])

    def render_page(
        self,
        source: Union[str, Path, bytes, DocumentSource],
        page: int,
        options: Optional[RenderOptions] = None,
    ) -> DocumentResult:
        """Renders a single page of a PDF document to an image."""
        if page < 1:
            raise InvalidArgumentError("Page index must be >= 1 (1-based index)")

        opts = options or RenderOptions()

        if isinstance(source, DocumentSource):
            src = source
        elif isinstance(source, bytes):
            src = DocumentSource.from_bytes(source)
        else:
            src = DocumentSource.from_path(str(source))

        arg = src.data if src.data is not None else str(src.path)
        fmt = getattr(opts, "image_format", None) or getattr(opts, "format", "png")
        res = _call(self._native.render_page, arg, page, opts.dpi, fmt)
        if res.get("type") == "memory":
            return DocumentResult(data=res.get("data"), mime_type=res.get("mime_type", "image/png"))
        return DocumentResult(
            path=Path(res["path"]), mime_type=res.get("mime_type", "image/png"), size_bytes=res.get("size_bytes", 0)
        )

    def call_tool(self, name: str, args: Union[dict, str]) -> List[dict]:
        """Invokes any of the 149 pdfcraft-automation tools headlessly."""
        if not name or not name.strip():
            raise InvalidArgumentError("Tool name cannot be empty")

        args_str = args if isinstance(args, str) else json.dumps(args)
        return _call(self._native.call_tool, name, args_str)

    def execute_batch(self, steps: Union[List[dict], str]) -> dict:
        """Executes a sequential batch of automation tools sharing a single engine instance."""
        steps_str = steps if isinstance(steps, str) else json.dumps(steps)
        return _call(self._native.execute_batch, steps_str)
