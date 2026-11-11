//! PyO3 Native Module Binding for `pdfcraft-sdk` (Issue #872).
//!
//! # Architecture Reference
//! Module: sdks/python/src/lib.rs
//! Purpose:
//!   Provides high-performance, in-process C-ABI Python bindings directly linking
//!   to `pdfcraft-sdk` for serverless or local execution without HTTP overhead.

use pyo3::exceptions::{PyIOError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use pdfcraft_sdk::{DocumentResult, DocumentSource, LocalClient, MergeOptions, PdfCraftClient, RenderOptions, SdkError, SplitMode};

fn map_sdk_err(err: SdkError) -> PyErr {
    match err {
        SdkError::InvalidArgument(msg) => PyValueError::new_err(("INVALID_ARGUMENT", msg)),
        SdkError::QuotaExceeded(msg) => PyValueError::new_err(("QUOTA_EXCEEDED", msg)),
        SdkError::FileNotFound { path } => PyIOError::new_err(("NOT_FOUND", format!("File not found: {path}"))),
        SdkError::Io(e) => PyIOError::new_err(("IO_ERROR", e.to_string())),
        other => PyValueError::new_err(("ENGINE_ERROR", other.to_string())),
    }
}

/// In-process native engine driver exposed to Python.
#[pyclass]
struct NativeLocalClient {
    client: LocalClient,
}

#[pymethods]
impl NativeLocalClient {
    #[new]
    fn new() -> Self {
        Self { client: LocalClient::default() }
    }

    /// Merges a list of file paths or raw byte buffers.
    #[pyo3(signature = (sources, output_filename=None, pages=None, passwords=None))]
    fn merge<'py>(
        &self,
        py: Python<'py>,
        sources: Vec<Bound<'py, PyAny>>,
        output_filename: Option<String>,
        pages: Option<Vec<Option<String>>>,
        passwords: Option<Vec<Option<String>>>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let mut rust_sources = Vec::with_capacity(sources.len());

        for item in sources {
            if let Ok(bytes) = item.downcast::<PyBytes>() {
                rust_sources.push(DocumentSource::from_bytes(bytes.as_bytes().to_vec(), None));
            } else if let Ok(s) = item.extract::<String>() {
                rust_sources.push(DocumentSource::from_path(s));
            } else {
                return Err(PyValueError::new_err("Each source must be either a file path string or bytes buffer"));
            }
        }

        let options = MergeOptions { output_filename, pages, passwords };

        let result = py.allow_threads(|| self.client.merge(&rust_sources, options)).map_err(map_sdk_err)?;

        let dict = PyDict::new(py);
        match result {
            DocumentResult::Memory { data, mime_type } => {
                dict.set_item("type", "memory")?;
                dict.set_item("data", PyBytes::new(py, &data))?;
                dict.set_item("mime_type", mime_type)?;
            }
            DocumentResult::File { path, mime_type, size_bytes } => {
                dict.set_item("type", "file")?;
                dict.set_item("path", path.to_string_lossy().to_string())?;
                dict.set_item("mime_type", mime_type)?;
                dict.set_item("size_bytes", size_bytes)?;
            }
        }

        Ok(dict)
    }

    /// Splits a PDF document into multiple files.
    #[pyo3(signature = (source, every_n_pages=None, before_pages=None, at_bookmarks=None, max_file_size_mb=None))]
    fn split<'py>(
        &self,
        py: Python<'py>,
        source: Bound<'py, PyAny>,
        every_n_pages: Option<usize>,
        before_pages: Option<Vec<usize>>,
        at_bookmarks: Option<bool>,
        max_file_size_mb: Option<f64>,
    ) -> PyResult<Vec<String>> {
        let selected = [every_n_pages.is_some(), before_pages.is_some(), at_bookmarks == Some(true), max_file_size_mb.is_some()]
            .into_iter()
            .filter(|&selected| selected)
            .count();
        if selected > 1 {
            return Err(PyValueError::new_err("Specify only one split mode"));
        }

        let rust_source = if let Ok(bytes) = source.downcast::<PyBytes>() {
            DocumentSource::from_bytes(bytes.as_bytes().to_vec(), None)
        } else if let Ok(s) = source.extract::<String>() {
            DocumentSource::from_path(s)
        } else {
            return Err(PyValueError::new_err("Source must be a file path string or bytes buffer"));
        };

        let mode = if let Some(n) = every_n_pages {
            SplitMode::EveryNPages(n)
        } else if let Some(pages) = before_pages {
            SplitMode::BeforePages(pages)
        } else if at_bookmarks.unwrap_or(false) {
            SplitMode::AtBookmarks
        } else if let Some(mb) = max_file_size_mb {
            SplitMode::MaxFileSizeMb(mb)
        } else {
            SplitMode::EveryNPages(1)
        };
        let res = py.allow_threads(|| self.client.split(&rust_source, mode)).map_err(map_sdk_err)?;

        Ok(res.files.into_iter().map(|p| p.to_string_lossy().to_string()).collect())
    }

    /// Renders a single page to an image.
    #[pyo3(signature = (source, page, dpi=150.0, image_format=None))]
    fn render_page<'py>(
        &self,
        py: Python<'py>,
        source: Bound<'py, PyAny>,
        page: usize,
        dpi: f64,
        image_format: Option<&str>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let rust_source = if let Ok(bytes) = source.downcast::<PyBytes>() {
            DocumentSource::from_bytes(bytes.as_bytes().to_vec(), None)
        } else if let Ok(s) = source.extract::<String>() {
            DocumentSource::from_path(s)
        } else {
            return Err(PyValueError::new_err("Source must be a file path string or bytes buffer"));
        };

        let format = match image_format.unwrap_or("png").to_lowercase().as_str() {
            "png" => pdfcraft_sdk::ImageFormat::Png,
            "jpeg" | "jpg" => pdfcraft_sdk::ImageFormat::Jpeg,
            "webp" => pdfcraft_sdk::ImageFormat::Webp,
            "tiff" | "tif" => pdfcraft_sdk::ImageFormat::Tiff,
            other => return Err(PyValueError::new_err(format!("Unsupported image format: {other}"))),
        };

        let options = RenderOptions { dpi, format };

        let result = py.allow_threads(|| self.client.render_page(&rust_source, page, options)).map_err(map_sdk_err)?;

        let dict = PyDict::new(py);
        match result {
            DocumentResult::Memory { data, mime_type } => {
                dict.set_item("type", "memory")?;
                dict.set_item("data", PyBytes::new(py, &data))?;
                dict.set_item("mime_type", mime_type)?;
            }
            DocumentResult::File { path, mime_type, size_bytes } => {
                dict.set_item("type", "file")?;
                dict.set_item("path", path.to_string_lossy().to_string())?;
                dict.set_item("mime_type", mime_type)?;
                dict.set_item("size_bytes", size_bytes)?;
            }
        }

        Ok(dict)
    }

    /// Invokes any named pdfcraft-automation tool with JSON arguments string or dict.
    fn call_tool<'py>(&self, py: Python<'py>, name: &str, args_json: &str) -> PyResult<Bound<'py, PyAny>> {
        let parsed_args: serde_json::Value = serde_json::from_str(args_json).map_err(|e| PyValueError::new_err(format!("Invalid JSON args: {e}")))?;

        let content_items = py.allow_threads(|| self.client.call_tool(name, parsed_args)).map_err(map_sdk_err)?;

        let py_list = pyo3::types::PyList::empty(py);
        for item in content_items {
            let dict = PyDict::new(py);
            match item {
                pdfcraft_sdk::ToolContent::Json(v) => {
                    dict.set_item("type", "json")?;
                    dict.set_item("data", v.to_string())?;
                }
                pdfcraft_sdk::ToolContent::Png { data, width, height } => {
                    dict.set_item("type", "png")?;
                    dict.set_item("data", PyBytes::new(py, &data))?;
                    dict.set_item("width", width)?;
                    dict.set_item("height", height)?;
                }
            }
            py_list.append(dict)?;
        }

        Ok(py_list.into_any())
    }

    /// Executes a sequential batch of automation tools sharing a single engine instance.
    #[pyo3(signature = (steps_json))]
    fn execute_batch<'py>(&self, py: Python<'py>, steps_json: &str) -> PyResult<Bound<'py, PyAny>> {
        let steps: Vec<pdfcraft_sdk::ports::BatchStep> =
            serde_json::from_str(steps_json).map_err(|e| PyValueError::new_err(format!("Invalid batch JSON: {e}")))?;

        let report = py.allow_threads(|| self.client.execute_batch(&steps)).map_err(map_sdk_err)?;
        let json_str = serde_json::to_string(&report).map_err(|e| PyValueError::new_err(format!("Serialization error: {e}")))?;

        let py_json = py.import("json")?;
        let dict = py_json.call_method1("loads", (json_str,))?;
        Ok(dict)
    }
}

/// PyO3 Python Module definition.
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<NativeLocalClient>()?;
    Ok(())
}
