//! Refuse pattern content that remains reachable after redaction (#3647).
//!
//! Pattern cells paint through a path, not `Do`, so clipping/removing the path does not remove
//! their bytes. Inspect the rewritten pages' resource graphs before committing the
//! working copy. This is deliberately conservative: an unused text/image-bearing pattern is
//! refused too. A plain vector pattern remains supported. The bounded read-only pass follows
//! the fail-closed approach proposed in PR #162.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::RedactError;

const MAX_DEPTH: usize = 64;
// Nesting depth counts owning levels, not every hop, so a chain of references would recurse
// without reaching it. Every walk step passes through `Guard::walk`, which bounds the frames.
const MAX_FRAMES: usize = 512;
const MAX_OBJECTS: usize = 16_384;
const MAX_STREAM_BYTES: usize = 1 << 20;
const MAX_TOTAL_BYTES: usize = 8 << 20;
const MAX_FILTERS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PatternProblem {
    #[error("a tiling pattern carries text or images that cannot be removed safely")]
    Content,
    #[error("pattern resources or content cannot be read completely")]
    Unreadable,
    #[error("pattern inspection exceeded its resource, depth or decoded-content budget")]
    Limit,
}

struct Guard {
    left: usize,
    bytes_left: usize,
    seen: HashSet<(ObjRef, bool)>,
    active: HashSet<(ObjRef, bool)>,
    frames: usize,
}

impl Guard {
    fn new() -> Self {
        Self { left: MAX_OBJECTS, bytes_left: MAX_TOTAL_BYTES, seen: HashSet::new(), active: HashSet::new(), frames: 0 }
    }

    fn charge(&mut self, depth: usize) -> Result<(), PatternProblem> {
        self.left = self.left.checked_sub(1).ok_or(PatternProblem::Limit)?;
        if depth > MAX_DEPTH {
            return Err(PatternProblem::Limit);
        }
        Ok(())
    }

    fn walk(&mut self, doc: &Document, object: &Object, inside: bool, depth: usize) -> Result<(), PatternProblem> {
        self.frames += 1;
        if self.frames > MAX_FRAMES {
            self.frames -= 1;
            return Err(PatternProblem::Limit);
        }
        let result = self.walk_body(doc, object, inside, depth);
        self.frames -= 1;
        result
    }

    fn walk_body(&mut self, doc: &Document, object: &Object, inside: bool, depth: usize) -> Result<(), PatternProblem> {
        self.charge(depth)?;
        match object {
            Object::Ref(reference) => {
                let key = (*reference, inside);
                if inside && self.active.contains(&key) {
                    return Err(PatternProblem::Unreadable);
                }
                if !self.seen.insert(key) {
                    return Ok(());
                }
                let value = doc.try_get(reference.num).map_err(|_| PatternProblem::Unreadable)?;
                self.active.insert(key);
                // Resolving a reference is not a nesting level: the budget is for how deep a
                // document nests, and the renderer follows 50 nested forms (#3645).
                let result = self.walk(doc, &value, inside, depth);
                self.active.remove(&key);
                result
            }
            Object::Array(values) => {
                for value in values {
                    self.walk(doc, value, inside, depth + 1)?;
                }
                Ok(())
            }
            Object::Dict(dict) => self.dictionary(doc, dict, inside, false, depth + 1),
            Object::Stream(stream) => {
                if inside {
                    let subtype = resolved(doc, stream.dict.get(b"Subtype"))?;
                    let pattern_type = resolved(doc, stream.dict.get(b"PatternType"))?;
                    if subtype.as_name() == Some(b"Image") {
                        return Err(PatternProblem::Content);
                    }
                    if subtype.as_name() == Some(b"Form") {
                        if resolved(doc, stream.dict.get(b"Subtype2"))? != Object::Null || resolved(doc, stream.dict.get(b"Ref"))? != Object::Null {
                            return Err(PatternProblem::Unreadable);
                        }
                        self.content(doc, stream)?;
                    } else if pattern_type.as_int() == Some(1) {
                        self.content(doc, stream)?;
                    }
                }
                self.dictionary(doc, &stream.dict, inside, true, depth + 1)
            }
            _ => Ok(()),
        }
    }

    fn dictionary(&mut self, doc: &Document, dict: &Dict, inside: bool, stream: bool, depth: usize) -> Result<(), PatternProblem> {
        // Stream dictionaries include appearance and soft-mask streams whose consumers
        // may infer Form semantics without /Subtype. Resource-map names, in contrast,
        // are arbitrary and must not be mistaken for dictionary field names.
        let subtype = resolved(doc, dict.get(b"Subtype"))?;
        let pattern_type = resolved(doc, dict.get(b"PatternType"))?;
        let owns_resources = stream || matches!(subtype.as_name(), Some(b"Form" | b"Type3")) || pattern_type.as_int() == Some(1);
        for (key, value) in dict.iter() {
            if owns_resources && key.as_slice() == b"Resources" {
                self.resources(doc, value, inside, depth)?;
            } else if inside || !owns_resources {
                self.walk(doc, value, inside, depth + 1)?;
            }
        }
        Ok(())
    }

    fn resources(&mut self, doc: &Document, object: &Object, inside: bool, depth: usize) -> Result<(), PatternProblem> {
        self.charge(depth)?;
        let object = resolved(doc, Some(object))?;
        // A Form's null Resources is equivalent to an absent entry: it uses the already
        // inspected caller resources. Tiling-pattern resources are required separately.
        if object == Object::Null {
            return Ok(());
        }
        let Some(resources) = object.as_dict() else { return Err(PatternProblem::Unreadable) };
        for (category, value) in resources.iter() {
            // Resource dictionaries may include large embedded fonts and colour tables. Only
            // structures that can own another pattern cell need recursive inspection.
            if !matches!(category.as_slice(), b"Pattern" | b"XObject" | b"Font" | b"Properties" | b"ExtGState") {
                continue;
            }
            if category.as_slice() == b"Font" {
                let fonts = resolved(doc, Some(value))?;
                let Some(fonts) = fonts.as_dict() else { return Err(PatternProblem::Unreadable) };
                for (_, font) in fonts.iter() {
                    self.charge(depth)?;
                    let font = resolved(doc, Some(font))?;
                    let Some(dict) = font.as_dict() else { return Err(PatternProblem::Unreadable) };
                    if resolved(doc, dict.get(b"Subtype"))?.as_name() == Some(b"Type3")
                        && let Some(resources) = dict.get(b"Resources")
                    {
                        self.resources(doc, resources, inside, depth + 1)?;
                    }
                }
                continue;
            }
            if matches!(category.as_slice(), b"Pattern" | b"XObject" | b"Properties" | b"ExtGState") && resolved(doc, Some(value))? == Object::Null {
                continue;
            }
            if category.as_slice() == b"Pattern" {
                let object = resolved(doc, Some(value))?;
                let Some(patterns) = object.as_dict() else { return Err(PatternProblem::Unreadable) };
                for (_, pattern) in patterns.iter() {
                    self.charge(depth)?;
                    let object = resolved(doc, Some(pattern))?;
                    if object == Object::Null {
                        continue;
                    }
                    let Some(dict) = object.as_dict() else { return Err(PatternProblem::Unreadable) };
                    let kind = resolved(doc, dict.get(b"PatternType"))?;
                    match (kind.as_int(), &object) {
                        (Some(1), Object::Stream(_)) => {
                            // Do not guess a fallback for a malformed cell, which different
                            // readers may resolve differently. Pattern resources are mandatory.
                            if resolved(doc, dict.get(b"Resources"))?.as_dict().is_none() {
                                return Err(PatternProblem::Unreadable);
                            }
                        }
                        (Some(2), Object::Dict(_)) => {}
                        _ => return Err(PatternProblem::Unreadable),
                    }
                    self.walk(doc, pattern, true, depth)?;
                }
            } else {
                if (inside && matches!(category.as_slice(), b"XObject" | b"Properties")) || category.as_slice() == b"ExtGState" {
                    let object = resolved(doc, Some(value))?;
                    let Some(entries) = object.as_dict() else { return Err(PatternProblem::Unreadable) };
                    for (_, entry) in entries.iter() {
                        self.charge(depth)?;
                        let object = resolved(doc, Some(entry))?;
                        if object == Object::Null {
                            continue;
                        }
                        if category.as_slice() == b"Properties" {
                            let Some(properties) = object.as_dict() else { return Err(PatternProblem::Unreadable) };
                            replacement_text(doc, properties)?;
                        } else if category.as_slice() == b"ExtGState" {
                            let Some(state) = object.as_dict() else { return Err(PatternProblem::Unreadable) };
                            match resolved(doc, state.get(b"SMask"))? {
                                Object::Null => {}
                                Object::Name(name) if name == b"None" => {}
                                Object::Dict(mask) => {
                                    let group = resolved(doc, mask.get(b"G"))?;
                                    let Object::Stream(group) = group else { return Err(PatternProblem::Unreadable) };
                                    readable_form(doc, &group)?;
                                    if let Some(value) = mask.get(b"G") {
                                        self.walk(doc, value, inside, depth + 1)?;
                                    }
                                }
                                _ => return Err(PatternProblem::Unreadable),
                            }
                        } else {
                            let Object::Stream(stream) = object else { return Err(PatternProblem::Unreadable) };
                            let subtype = resolved(doc, stream.dict.get(b"Subtype"))?;
                            if subtype.as_name() == Some(b"Image") {
                                return Err(PatternProblem::Content);
                            }
                            readable_form(doc, &stream)?;
                        }
                    }
                }
                if category.as_slice() == b"XObject" {
                    let entries = resolved(doc, Some(value))?;
                    let Some(entries) = entries.as_dict() else { return Err(PatternProblem::Unreadable) };
                    for (_, entry) in entries.iter() {
                        self.charge(depth)?;
                        let object = resolved(doc, Some(entry))?;
                        if let Object::Stream(stream) = object
                            && resolved(doc, stream.dict.get(b"Subtype"))?.as_name() == Some(b"Form")
                        {
                            self.walk(doc, entry, inside, depth)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn content(&mut self, doc: &Document, stream: &Stream) -> Result<(), PatternProblem> {
        // Strict decoding with a small bound, unlike the tolerant general reader. Normalize
        // indirect filter declarations before using Stream::filters, which accepts direct data.
        let normalized = normalized_stream(doc, stream)?;
        let chain = normalized.filters();
        let max = MAX_STREAM_BYTES.min(self.bytes_left);
        if chain.is_empty() && normalized.raw.len() > max {
            return Err(PatternProblem::Limit);
        }
        let data = pdfcraft_filters::decode(&chain, &normalized.raw, max).map_err(|error| match error {
            pdfcraft_filters::FilterError::LimitExceeded(_) => PatternProblem::Limit,
            _ => PatternProblem::Unreadable,
        })?;
        self.bytes_left = self.bytes_left.checked_sub(data.len()).ok_or(PatternProblem::Limit)?;
        let parsed = pdfcraft_content::parse(&data);
        if parsed.skipped != 0 {
            return Err(PatternProblem::Unreadable);
        }
        for op in parsed.ops {
            self.charge(0)?;
            if matches!(op.op.as_slice(), b"BDC" | b"DP") {
                for operand in &op.operands {
                    if let Some(properties) = resolved(doc, Some(operand))?.as_dict() {
                        replacement_text(doc, properties)?;
                    }
                }
            }
            match op.op.as_slice() {
                b"Tj" | b"TJ" | b"'" | b"\"" | b"BI" => return Err(PatternProblem::Content),
                // Unknown operators cannot establish that no hidden content was skipped.
                b"q" | b"Q" | b"cm" | b"w" | b"J" | b"j" | b"M" | b"d" | b"ri" | b"i" | b"gs" | b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re"
                | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" | b"W" | b"W*" | b"BT" | b"ET" | b"Tc" | b"Tw" | b"Tz"
                | b"TL" | b"Tf" | b"Tr" | b"Ts" | b"Td" | b"TD" | b"Tm" | b"T*" | b"d0" | b"d1" | b"CS" | b"cs" | b"SC" | b"SCN" | b"sc" | b"scn"
                | b"G" | b"g" | b"RG" | b"rg" | b"K" | b"k" | b"sh" | b"Do" | b"MP" | b"DP" | b"BMC" | b"BDC" | b"EMC" | b"BX" | b"EX" => {}
                _ => return Err(PatternProblem::Unreadable),
            }
        }
        Ok(())
    }
}

fn readable_form(doc: &Document, stream: &Stream) -> Result<(), PatternProblem> {
    // PS, Form/Subtype2 PS, reference XObjects and unknown/missing subtypes cannot be
    // proven free of recoverable pattern content by the PDF content parser.
    if resolved(doc, stream.dict.get(b"Subtype"))?.as_name() != Some(b"Form")
        || resolved(doc, stream.dict.get(b"Subtype2"))? != Object::Null
        || resolved(doc, stream.dict.get(b"Ref"))? != Object::Null
    {
        return Err(PatternProblem::Unreadable);
    }
    Ok(())
}

fn replacement_text(doc: &Document, properties: &Dict) -> Result<(), PatternProblem> {
    for key in [b"ActualText".as_slice(), b"Alt", b"E"] {
        if resolved(doc, properties.get(key))? != Object::Null {
            return Err(PatternProblem::Content);
        }
    }
    Ok(())
}

fn resolved(doc: &Document, value: Option<&Object>) -> Result<Object, PatternProblem> {
    let mut value = value.cloned().unwrap_or(Object::Null);
    for _ in 0..MAX_DEPTH {
        let Object::Ref(reference) = value else { return Ok(value) };
        value = (*doc.try_get(reference.num).map_err(|_| PatternProblem::Unreadable)?).clone();
    }
    Err(PatternProblem::Limit)
}

fn normalized_stream(doc: &Document, stream: &Stream) -> Result<Stream, PatternProblem> {
    if stream.dict.contains(b"F") || stream.dict.contains(b"FFilter") || stream.dict.contains(b"FDecodeParms") || stream.dict.contains(b"DP") {
        return Err(PatternProblem::Unreadable);
    }
    let mut normalized = stream.clone();
    let filter = resolved(doc, stream.dict.get(b"Filter"))?;
    let names = match &filter {
        Object::Null => Vec::new(),
        Object::Name(_) => vec![filter],
        Object::Array(values) if values.len() <= MAX_FILTERS => values.iter().map(|v| resolved(doc, Some(v))).collect::<Result<Vec<_>, _>>()?,
        _ => return Err(PatternProblem::Unreadable),
    };
    if names.iter().any(|name| name.as_name().is_none()) {
        return Err(PatternProblem::Unreadable);
    }
    let parms = resolved(doc, stream.dict.get(b"DecodeParms"))?;
    let parms = match parms {
        Object::Null => vec![Object::Null; names.len()],
        Object::Dict(_) if names.len() == 1 => vec![parms],
        Object::Array(values) if values.len() == names.len() => values,
        _ => return Err(PatternProblem::Unreadable),
    };
    let parms = parms
        .iter()
        .map(|value| {
            let value = resolved(doc, Some(value))?;
            let Object::Dict(mut dict) = value else {
                return if value == Object::Null { Ok(value) } else { Err(PatternProblem::Unreadable) };
            };
            for key in [b"Predictor".as_slice(), b"Colors", b"BitsPerComponent", b"Columns", b"EarlyChange"] {
                if let Some(value) = dict.get(key) {
                    let value = resolved(doc, Some(value))?;
                    if value.as_int().is_none() {
                        return Err(PatternProblem::Unreadable);
                    }
                    dict.set(key.to_vec(), value);
                }
            }
            if !matches!(dict.int(b"Predictor").unwrap_or(1), 1 | 2 | 10..=15)
                || dict.int(b"Colors").unwrap_or(1) <= 0
                || dict.int(b"Columns").unwrap_or(1) <= 0
                || !matches!(dict.int(b"BitsPerComponent").unwrap_or(8), 1 | 2 | 4 | 8 | 16)
                || !matches!(dict.int(b"EarlyChange").unwrap_or(1), 0 | 1)
            {
                return Err(PatternProblem::Unreadable);
            }
            Ok(Object::Dict(dict))
        })
        .collect::<Result<Vec<_>, _>>()?;
    normalized.dict.set(b"Filter".to_vec(), Object::Array(names));
    normalized.dict.set(b"DecodeParms".to_vec(), Object::Array(parms));
    Ok(normalized)
}

/// Validate the requested pages before `apply` commits its working copy.
pub(crate) fn check(doc: &Document, selected: &[usize]) -> Result<(), RedactError> {
    let pages = pdfcraft_model::pages(doc);
    for &index in selected {
        let Some(page) = pages.get(index) else { return Err(RedactError::Unreadable(index.saturating_add(1))) };
        let mut guard = Guard::new();
        let checked = (|| {
            if let Some(resources) = page.dict.get(b"Resources") {
                guard.resources(doc, resources, false, 0)?;
            }
            // Kept appearance streams may own their own pattern resources.
            for annotation in crate::annots_of(doc, &page.dict) {
                let object = resolved(doc, Some(&annotation))?;
                if let Some(appearance) = object.as_dict().and_then(|dict| dict.get(b"AP")) {
                    guard.walk(doc, appearance, false, 0)?;
                }
            }
            Ok(())
        })();
        checked.map_err(|reason| RedactError::Pattern { page: index.saturating_add(1), reason })?;
    }
    Ok(())
}
