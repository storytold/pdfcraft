//! Generic tool catalogue, schema parsing, and invocation bridging to pdfcraft-automation.

use pdfcraft_automation::Automation;
use serde_json::Value;
use std::collections::HashSet;

use crate::error::{FfiError, catch_panic};

/// Mobile classification for tools in the automation catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiToolClassification {
    /// Rendered automatically via a native schema-driven form.
    SchemaDriven,
    /// Handled via a bespoke mobile screen or direct canvas interaction.
    BespokeScreen,
    /// Explicitly unsupported or deferred on mobile with a documented reason.
    NotOnMobile,
}

/// An output file produced by an automation tool (e.g. rendered images, exported data/files).
#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiToolOutputFile {
    pub name: String,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

/// Result of invoking an automation tool.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiToolResult {
    pub success: bool,
    pub output_json: String,
    pub output_files: Vec<FfiToolOutputFile>,
    pub error: Option<String>,
}

/// Supported field types for mobile schema-driven form rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiSchemaFieldType {
    String,
    Integer,
    Number,
    Boolean,
    Enum,
    PageRange,
    Array,
    Object,
}

/// A parsed field definition from a tool's JSON Schema for schema-driven form rendering.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiSchemaField {
    pub name: String,
    pub title: String,
    pub description: String,
    pub field_type: FfiSchemaFieldType,
    pub required: bool,
    pub default_value: Option<String>,
    pub enum_options: Vec<String>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
}

/// Information about a tool in the automation catalogue.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiToolInfo {
    pub name: String,
    pub title: String,
    pub description: String,
    pub input_schema_json: String,
    pub read_only: bool,
    pub destructive: bool,
    pub classification: FfiToolClassification,
    pub mobile_reason: Option<String>,
}

/// Classifies a tool by name into BespokeScreen, NotOnMobile, or SchemaDriven.
pub fn classify_tool(name: &str) -> (FfiToolClassification, Option<String>) {
    match name {
        // Bespoke screen / direct UI flows
        "doc_open" | "doc_list" | "doc_close" | "page_render" => (FfiToolClassification::BespokeScreen, None),
        "page_rotate" | "page_delete" | "page_move" | "page_insert_blank" | "page_insert_file" | "page_extract" | "page_replace"
        | "page_duplicate" | "page_reorder" => (FfiToolClassification::BespokeScreen, None),
        "page_enhance_camera" | "scan_to_pdf" => (FfiToolClassification::BespokeScreen, None),
        "comment_add"
        | "comment_reply"
        | "comment_delete"
        | "comment_set_state"
        | "comment_image_preview"
        | "comment_list"
        | "comment_mark"
        | "ink_add"
        | "text_markup_add" => (FfiToolClassification::BespokeScreen, None),
        "form_fields" | "form_reset" | "sign_field" | "signature_add" | "form_fill" => (FfiToolClassification::BespokeScreen, None),
        "text_find" => (FfiToolClassification::BespokeScreen, None),

        // Not on mobile due to platform sandbox or desktop driver constraints
        "printers" | "printer_options" | "doc_print" | "doc_print_readiness" => (
            FfiToolClassification::NotOnMobile,
            Some("Mobile OS delegates printing to system print sheet (AirPrint / Android PrintFramework)".into()),
        ),
        "link_open" => {
            (FfiToolClassification::NotOnMobile, Some("Sandboxed mobile application cannot launch arbitrary external executable paths".into()))
        }

        // Heavy feature gates if compiled without the respective feature
        #[cfg(not(feature = "ocr"))]
        "ocr_recognize" | "ocr_recognize_files" | "ocr_status" => {
            (FfiToolClassification::NotOnMobile, Some("Tool disabled: 'ocr' feature not compiled into pdfcraft-ffi".into()))
        }
        #[cfg(not(feature = "sign"))]
        "sign_list" | "sign_id_create" | "sign_document" | "sign_trust" => {
            (FfiToolClassification::NotOnMobile, Some("Tool disabled: 'sign' feature not compiled into pdfcraft-ffi".into()))
        }
        #[cfg(not(feature = "export"))]
        "doc_export_images" | "doc_export_text" | "doc_export_all_images" | "doc_export_office" => {
            (FfiToolClassification::NotOnMobile, Some("Tool disabled: 'export' feature not compiled into pdfcraft-ffi".into()))
        }

        // All other catalogue tools render via schema-driven forms
        _ => (FfiToolClassification::SchemaDriven, None),
    }
}

/// Parses the properties of a JSON Schema object into a list of FfiSchemaFields.
pub fn parse_schema_fields(schema: &Value) -> Result<Vec<FfiSchemaField>, String> {
    let props = schema.get("properties").and_then(Value::as_object).ok_or_else(|| "schema has no properties object".to_string())?;

    let req_set: HashSet<&str> =
        schema.get("required").and_then(Value::as_array).map(|arr| arr.iter().filter_map(Value::as_str).collect()).unwrap_or_default();

    let mut fields = Vec::new();
    for (name, prop) in props {
        // Skip document id if automatically injected by document context
        let desc = prop.get("description").and_then(Value::as_str).unwrap_or("").to_string();
        let title = prop.get("title").and_then(Value::as_str).map(String::from).unwrap_or_else(|| {
            // Generate title from snake_case name
            name.replace('_', " ")
                .split_whitespace()
                .map(|w| {
                    let mut chars = w.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        });

        let required = req_set.contains(name.as_str());
        let default_val = prop.get("default").map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        });
        let min_val = prop.get("minimum").and_then(Value::as_f64);
        let max_val = prop.get("maximum").and_then(Value::as_f64);

        let (field_type, enum_opts) = if let Some(opts) = prop.get("enum").and_then(Value::as_array) {
            let options = opts.iter().filter_map(Value::as_str).map(String::from).collect();
            (FfiSchemaFieldType::Enum, options)
        } else if name == "pages"
            || name.ends_with("_pages")
            || desc.to_lowercase().contains("page range")
            || desc.to_lowercase().contains("pages to")
        {
            (FfiSchemaFieldType::PageRange, Vec::new())
        } else {
            let ty = prop.get("type").and_then(Value::as_str).unwrap_or("string");
            let fty = match ty {
                "string" => FfiSchemaFieldType::String,
                "integer" => FfiSchemaFieldType::Integer,
                "number" => FfiSchemaFieldType::Number,
                "boolean" => FfiSchemaFieldType::Boolean,
                "array" => FfiSchemaFieldType::Array,
                "object" => FfiSchemaFieldType::Object,
                _ => return Err(format!("unsupported field type: {ty} for {name}")),
            };
            (fty, Vec::new())
        };

        fields.push(FfiSchemaField {
            name: name.clone(),
            title,
            description: desc,
            field_type,
            required,
            default_value: default_val,
            enum_options: enum_opts,
            minimum: min_val,
            maximum: max_val,
        });
    }

    Ok(fields)
}

/// Parses a tool's JSON Schema string into structured fields for mobile rendering.
#[uniffi::export]
pub fn parse_tool_schema(input_schema_json: String) -> Result<Vec<FfiSchemaField>, FfiError> {
    let schema: Value =
        serde_json::from_str(&input_schema_json).map_err(|e| FfiError::InvalidArgs { reason: format!("invalid schema JSON: {e}") })?;
    parse_schema_fields(&schema).map_err(|e| FfiError::InvalidArgs { reason: e })
}

/// Returns the catalogue of all available engine automation tools.
#[uniffi::export]
pub fn list_tools() -> Vec<FfiToolInfo> {
    pdfcraft_automation::tools()
        .into_iter()
        .map(|t| {
            let (mut classification, mut mobile_reason) = classify_tool(t.name);
            // If marked as SchemaDriven, verify schema is parseable; if not, degrade to NotOnMobile
            if classification == FfiToolClassification::SchemaDriven {
                match parse_schema_fields(&t.input_schema) {
                    Ok(_) => {}
                    Err(e) => {
                        classification = FfiToolClassification::NotOnMobile;
                        mobile_reason = Some(format!("Unsupported schema structure: {e}"));
                    }
                }
            }
            FfiToolInfo {
                name: t.name.to_string(),
                title: t.title.to_string(),
                description: t.description.to_string(),
                input_schema_json: t.input_schema.to_string(),
                read_only: t.read_only,
                destructive: t.destructive,
                classification,
                mobile_reason,
            }
        })
        .collect()
}

/// Invokes a tool by name with arguments passed as JSON string.
#[uniffi::export]
pub fn invoke_tool(name: String, args_json: String) -> Result<FfiToolResult, FfiError> {
    catch_panic(|| {
        let (classification, reason) = classify_tool(&name);
        if classification == FfiToolClassification::NotOnMobile
            && let Some(r) = reason
            && r.contains("feature not compiled into pdfcraft-ffi")
        {
            return Err(FfiError::InvalidArgs { reason: format!("Cannot invoke '{name}': {r}") });
        }

        let args: Value = serde_json::from_str(&args_json).map_err(|e| FfiError::InvalidArgs { reason: format!("invalid JSON args: {e}") })?;

        let mut automation = Automation::new();
        match automation.call(&name, &args) {
            Ok(contents) => {
                let mut out_values = Vec::new();
                let mut output_files = Vec::new();
                for (idx, c) in contents.into_iter().enumerate() {
                    match c {
                        pdfcraft_automation::Content::Json(val) => out_values.push(val),
                        pdfcraft_automation::Content::Png { width, height, data } => {
                            let file_name = format!("output_{}.png", idx + 1);
                            output_files.push(FfiToolOutputFile { name: file_name.clone(), mime_type: "image/png".into(), bytes: data });
                            out_values.push(serde_json::json!({
                                "type": "png",
                                "name": file_name,
                                "width": width,
                                "height": height,
                            }));
                        }
                    }
                }
                let output_json = serde_json::to_string(&out_values).unwrap_or_else(|_| "[]".into());
                Ok(FfiToolResult { success: true, output_json, output_files, error: None })
            }
            Err(e) => Ok(FfiToolResult { success: false, output_json: "null".into(), output_files: Vec::new(), error: Some(e.to_string()) }),
        }
    })
}
