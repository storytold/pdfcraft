//! `geometry FILE`: pages' line segments as one JSON document, from the automation tool
//! `page_geometry`, so the command and the tool give the same answer.
use serde_json::{Map, Value, json};

use super::commands::page_list;
use super::options::{GEOMETRY_OPTIONS, needs_value, positional, validate_options};
use super::{CliError, stdout_line};

/// The limit options, and the `page_geometry` argument each sets.
const LIMITS: [(&str, &str); 4] = [
    ("--max-segments", "max_segments"),
    ("--max-targets", "max_targets"),
    ("--max-bytes", "max_bytes"),
    ("--max-total-segments", "max_total_segments"),
];

/// The value-taking option given last with no value after it. `flag` reads that as the option
/// being absent, so `geometry FILE --coords` ran with the default. The walk is the one
/// `validate_options` makes, so `--password --coords` is still a password of `--coords`.
fn missing_value(args: &[String]) -> Option<&'static str> {
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        match GEOMETRY_OPTIONS.iter().find(|spec| spec.name == arg) {
            Some(spec) if spec.takes_value && i + 1 == args.len() => return Some(spec.name),
            Some(spec) if spec.takes_value => i += 2,
            _ => i += 1,
        }
    }
    None
}

/// The value of each value-taking option, read in the same single walk as `missing_value`, so
/// an option's value is never read again as an option: `--password --coords` is a password of
/// `--coords`, and `--coords` there sets nothing. A repeated option keeps its first value, as
/// `flag` did.
fn option_values(args: &[String]) -> Vec<(&'static str, &str)> {
    let mut values: Vec<(&'static str, &str)> = Vec::new();
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        match GEOMETRY_OPTIONS.iter().find(|spec| spec.name == arg) {
            Some(spec) if spec.takes_value => {
                if let Some(value) = args.get(i + 1)
                    && !values.iter().any(|(name, _)| *name == spec.name)
                {
                    values.push((spec.name, value));
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    values
}

/// One option's value from `option_values`.
fn value_of<'a>(values: &[(&'static str, &'a str)], option: &str) -> Option<&'a str> {
    values.iter().find(|(name, _)| *name == option).map(|(_, value)| *value)
}

/// The one input file, once every option given has its value: both are checked before the
/// document is opened, so a mistyped command prints nothing on stdout.
fn input_file(args: &[String]) -> Result<&str, String> {
    if let Some(option) = missing_value(args) {
        return Err(needs_value("geometry", option, None));
    }
    match positional(args, GEOMETRY_OPTIONS)[..] {
        [] => Err("geometry: missing file".into()),
        [path] => Ok(path),
        [_, extra, ..] => Err(format!("geometry: unexpected argument {extra:?} (one input file)")),
    }
}

/// The `page_geometry` arguments the command line asks for (without the document).
fn tool_args(values: &[(&'static str, &str)]) -> Result<Map<String, Value>, String> {
    let mut tool = Map::new();
    if let Some(pages) = value_of(values, "--pages") {
        let pages = page_list(pages).map_err(|e| format!("geometry: {e}"))?;
        tool.insert("pages".into(), json!(pages.iter().map(|p| p + 1).collect::<Vec<_>>()));
    }
    if let Some(coords) = value_of(values, "--coords") {
        tool.insert("coords".into(), json!(coords));
    }
    for (option, key) in LIMITS {
        if let Some(v) = value_of(values, option) {
            let n: u64 = v.parse().map_err(|_| format!("geometry: bad {option} {v:?}: expected a whole number"))?;
            tool.insert(key.into(), json!(n));
        }
    }
    Ok(tool)
}

/// The JSON a tool returned.
fn json_of(content: Vec<pdfcraft_automation::Content>) -> Option<Value> {
    content.into_iter().find_map(|c| match c {
        pdfcraft_automation::Content::Json(v) => Some(v),
        pdfcraft_automation::Content::Png { .. } => None,
    })
}

pub(crate) fn geometry(args: &[String]) -> Result<(), CliError> {
    validate_options("geometry", args, GEOMETRY_OPTIONS)?;
    let path = input_file(args)?;
    let values = option_values(args);
    let mut tool = tool_args(&values)?;
    let mut auto = pdfcraft_automation::Automation::new();
    let mut open = json!({ "path": path });
    if let Some(password) = value_of(&values, "--password") {
        open["password"] = json!(password);
    }
    let opened = auto.call("doc_open", &open).map_err(|e| format!("geometry: {e}"))?;
    let doc = json_of(opened).and_then(|v| v["doc"].as_u64()).ok_or("geometry: the document did not open")?;
    tool.insert("doc".into(), json!(doc));
    let result = auto.call("page_geometry", &Value::Object(tool)).map_err(|e| format!("geometry: {e}"))?;
    let mut result = json_of(result).ok_or("geometry: no result")?;
    // `as_object_mut`, not `result["file"] =`, which panics on anything but an object.
    if let Some(object) = result.as_object_mut() {
        object.insert("file".into(), json!(path));
    }
    // One line: a heavy sheet lists millions of segments. `Value`'s `Display` writes the same
    // compact JSON as `to_string` straight to stdout, without a second copy of it in memory.
    stdout_line(format_args!("{result}"))
}
