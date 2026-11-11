//! Preserving Reader usage rights (`/Perms /UR3` and `/UR`) on dynamic XFA forms when saving
//! data-only changes: updating only the `datasets` (and `form`, when script overrides exist)
//! packets in the original shell without PdfCraft's layout revision (#4309).

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

use crate::model::{Access, Node, Presence, Subform, Template, Value};
use crate::packets::{Encoding, decode, encode};
use crate::{
    DataNode, FormNode, MAX_XML_DEPTH, Overrides, XfaError, apply_overrides, data_of, form_tree, overrides, parse_datasets, template_of,
    xml_nesting_depth,
};

/// The warning reported when laying out a dynamic XFA form voids its usage rights and the save
/// cannot preserve the original shell.
pub const USAGE_RIGHTS_REMOVED: &str = crate::pdf::USAGE_RIGHTS_REMOVED;

const MAX_REF_HOPS: usize = 16;
const MAX_PACKET_BYTES: usize = 64 << 20;
const MAX_PACKET_WRITE: usize = 48 << 20;
const MAX_NODES: u32 = 500_000;
const XFA_DATA_NS: &str = "http://www.xfa.org/schema/xfa-data/1.0/";
const XFA_FORM_NS: &str = "http://www.xfa.org/schema/xfa-form/2.8/";
const XDP_OPEN: &str = "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\">";
const XDP_CLOSE: &str = "</xdp:xdp>";
const WRAP_OPEN: &str = "<pdfcraft-wrap xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\" xmlns:xdp=\"http://ns.adobe.com/xdp/\">";
const WRAP_CLOSE: &str = "</pdfcraft-wrap>";
const EMPTY_DATASETS: &str = "<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data/></xfa:datasets>";

/// Follow indirect references up to [`MAX_REF_HOPS`] steps, rejecting cycles.
fn resolve_chain(doc: &Document, obj: &Object) -> Option<Object> {
    let Object::Ref(mut cur) = *obj else { return Some(obj.clone()) };
    let mut seen = HashSet::new();
    for _ in 0..MAX_REF_HOPS {
        if !seen.insert(cur) {
            return None;
        }
        let next = doc.get(cur);
        match &*next {
            Object::Ref(r) => cur = *r,
            other => return Some(other.clone()),
        }
    }
    None
}

/// Whether `doc`'s catalog `/Perms` dictionary carries Reader usage rights (`/UR3` or `/UR`),
/// following indirect reference chains and ignoring `null` entries (§7.3.7).
pub fn has_usage_rights(doc: &Document) -> bool {
    let Some(root) = doc.root() else { return false };
    let catalog = doc.get(root);
    let Some(perms) = catalog.as_dict().and_then(|c| c.get(b"Perms")).and_then(|p| resolve_chain(doc, p)) else {
        return false;
    };
    let Some(perms) = perms.as_dict() else { return false };
    [b"UR3".as_slice(), b"UR".as_slice()]
        .into_iter()
        .any(|key| perms.get(key).and_then(|v| resolve_chain(doc, v)).is_some_and(|v| !matches!(v, Object::Null)))
}

/// Whether `doc` is a usage-rights XFA shell whose packets decode losslessly and can accept
/// updated `datasets` and `form` packets without rewriting the template stream.
pub fn can_preserve_usage_rights(doc: &Document) -> bool {
    if !has_usage_rights(doc) {
        return false;
    }
    let Ok(xfa) = xfa_object(doc) else { return false };
    match xfa {
        Object::Array(items) => {
            let Some(ti) = find_array_packet(doc, &items, "template") else { return false };
            if items.get(ti + 1).is_none_or(|o| decode_stream_exact(doc, o).is_err()) {
                return false;
            }
            for name in ["datasets", "form"] {
                if let Some(i) = find_array_packet(doc, &items, name)
                    && items.get(i + 1).is_none_or(|o| decode_stream_exact(doc, o).is_err())
                {
                    return false;
                }
            }
            let missing_packet = find_array_packet(doc, &items, "datasets").is_none() || find_array_packet(doc, &items, "form").is_none();
            !missing_packet || check_array_envelope_possible(doc, &items).is_ok()
        }
        Object::Stream(s) => s
            .decoded_within(MAX_PACKET_BYTES)
            .ok()
            .and_then(|b| decode(&b))
            .is_some_and(|(xdp, _)| xml_nesting_depth(&xdp) <= MAX_XML_DEPTH && roxmltree::Document::parse_with_options(&xdp, xml_opts()).is_ok()),
        _ => false,
    }
}

/// Whether `doc`'s scripted `presence`/`access` overrides can be written to an XFA `<form>` packet
/// and read back on reopen with identical live-form state.
pub fn overrides_roundtrip(doc: &Document) -> bool {
    let ov = overrides(doc);
    if ov.is_empty() {
        return true;
    }
    if ov.presence.keys().chain(ov.access.keys()).any(|k| k.contains("#pageSet")) {
        return false;
    }
    let Some(tpl_elem) = extract_template_element(doc).ok().flatten() else { return false };
    let Ok((raw_tpl, _)) = crate::parse(&tpl_elem) else { return false };
    let Ok(cur_tpl) = effective_template(doc) else { return false };
    let wanted = apply_overrides(&cur_tpl, &ov);
    let form_xml = build_form_packet(&raw_tpl.root, &wanted.root);
    let combined = format!("{XDP_OPEN}{tpl_elem}{form_xml}{XDP_CLOSE}");
    let Ok((reopened, _)) = crate::parse(&combined) else { return false };
    let data = effective_data(doc);
    let live_want = form_tree(&cur_tpl, data.as_ref(), &ov);
    let live_got = form_tree(&reopened, data.as_ref(), &Overrides::default());
    !live_want.truncated && !live_got.truncated && same_live_state(&live_want.root, &live_got.root, 0)
}

/// Copy the current XFA form data (`datasets`) and any script `presence`/`access` state (`form`
/// packet) from `source` into the unlaid-out `shell`, leaving `shell`'s template, placeholder
/// pages and `/Perms` untouched. Returns `Ok(true)` when `shell` changed, `Ok(false)` when
/// `shell` already held the same data and state.
pub fn sync_form_into_shell(shell: &mut Document, source: &Document) -> Result<bool, XfaError> {
    let ds_update = wanted_datasets(shell, source)?;
    let form_update = wanted_form_packet(shell, source)?;
    if ds_update.is_none() && form_update.is_none() {
        return Ok(false);
    }
    write_shell_packets(shell, ds_update.as_deref(), form_update.as_deref())?;
    Ok(true)
}

fn xml_opts<'a>() -> roxmltree::ParsingOptions<'a> {
    roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES, ..Default::default() }
}

fn parse_packet_xml(text: &str) -> Result<(), XfaError> {
    if xml_nesting_depth(text) > MAX_XML_DEPTH {
        return Err(XfaError::TooLarge("the XFA packet nests XML elements deeper than the reader accepts".into()));
    }
    if roxmltree::Document::parse_with_options(text, xml_opts()).is_ok() {
        return Ok(());
    }
    let wrapped = format!("{WRAP_OPEN}{}{WRAP_CLOSE}", strip_xml_decl(text));
    roxmltree::Document::parse_with_options(&wrapped, xml_opts()).map(|_| ()).map_err(|e| XfaError::Malformed(e.to_string()))
}

fn decode_stream_exact(doc: &Document, obj: &Object) -> Result<(String, Encoding), XfaError> {
    let resolved = resolve_chain(doc, obj).ok_or_else(|| XfaError::Malformed("cyclic indirect reference in XFA packet".into()))?;
    let Object::Stream(s) = resolved else {
        return Err(XfaError::Malformed("the XFA packet is not a stream".into()));
    };
    let bytes = s.decoded_within(MAX_PACKET_BYTES)?;
    decode(&bytes).ok_or_else(|| XfaError::Malformed("the XFA packet is neither UTF-8 nor UTF-16".into()))
}

fn find_array_packet(doc: &Document, items: &[Object], target: &str) -> Option<usize> {
    let mut i = 0;
    while i < items.len() {
        if let Some(name) = items.get(i).and_then(|o| resolve_chain(doc, o)).and_then(|o| o.as_string().map(|s| s.to_text())) {
            if name == target && i + 1 < items.len() {
                return Some(i);
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    None
}

fn xfa_object(doc: &Document) -> Result<Object, XfaError> {
    let root = doc.root().ok_or(XfaError::NotXfa)?;
    let catalog = doc.get(root);
    let acro = catalog.as_dict().and_then(|c| c.get(b"AcroForm")).and_then(|a| resolve_chain(doc, a)).ok_or(XfaError::NotXfa)?;
    let xfa = acro.as_dict().and_then(|a| a.get(b"XFA")).ok_or(XfaError::NotXfa)?;
    resolve_chain(doc, xfa).ok_or(XfaError::NotXfa)
}

fn extract_named_packet(doc: &Document, name: &str) -> Result<Option<String>, XfaError> {
    match xfa_object(doc)? {
        Object::Array(items) => match find_array_packet(doc, &items, name) {
            Some(i) => {
                let obj = items.get(i + 1).ok_or(XfaError::NotXfa)?;
                Ok(Some(decode_stream_exact(doc, obj)?.0))
            }
            None => Ok(None),
        },
        Object::Stream(s) => {
            let bytes = s.decoded_within(MAX_PACKET_BYTES)?;
            let (xdp, _) = decode(&bytes).ok_or_else(|| XfaError::Malformed("the XFA stream is neither UTF-8 nor UTF-16".into()))?;
            if xml_nesting_depth(&xdp) > MAX_XML_DEPTH {
                return Err(XfaError::TooLarge("the XFA stream nests XML elements deeper than the reader accepts".into()));
            }
            let parsed = roxmltree::Document::parse_with_options(&xdp, xml_opts()).map_err(|e| XfaError::Malformed(e.to_string()))?;
            let found = find_xdp_element(&parsed, name);
            Ok(found.and_then(|n| xdp.get(n.range())).map(str::to_owned))
        }
        _ => Err(XfaError::NotXfa),
    }
}

fn is_template_element(n: &roxmltree::Node<'_, '_>) -> bool {
    n.is_element()
        && n.tag_name().name() == "template"
        && (n.tag_name().namespace().is_some_and(|ns| ns.contains("xfa-template"))
            || n.children().any(|c| c.is_element() && c.tag_name().name() == "subform"))
}

fn extract_template_element(doc: &Document) -> Result<Option<String>, XfaError> {
    let Some(raw) = extract_named_packet(doc, "template")? else { return Ok(None) };
    if xml_nesting_depth(&raw) > MAX_XML_DEPTH {
        return Err(XfaError::TooLarge("the XFA template nests XML elements deeper than the reader accepts".into()));
    }
    let parsed = roxmltree::Document::parse_with_options(&raw, xml_opts()).map_err(|e| XfaError::Malformed(e.to_string()))?;
    let root = parsed.root_element();
    let tpl = if is_template_element(&root) { Some(root) } else { root.children().find(is_template_element) };
    Ok(tpl.and_then(|n| raw.get(n.range())).map(str::to_owned))
}

fn find_xdp_element<'a, 'input>(parsed: &'a roxmltree::Document<'input>, name: &str) -> Option<roxmltree::Node<'a, 'input>> {
    let root = parsed.root_element();
    if name == "template" {
        return if is_template_element(&root) { Some(root) } else { root.children().find(is_template_element) };
    }
    if root.tag_name().name() == name {
        return Some(root);
    }
    match name {
        "datasets" => root
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "datasets" && n.tag_name().namespace() == Some(XFA_DATA_NS))
            .or_else(|| root.children().find(|n| n.is_element() && n.tag_name().name() == "datasets")),
        "form" => root
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "form" && n.tag_name().namespace().is_some_and(|ns| ns.contains("xfa-form"))),
        other => root.children().find(|n| n.is_element() && n.tag_name().name() == other),
    }
}

fn strip_xml_decl(xml: &str) -> &str {
    let trimmed = xml.trim_start_matches('\u{feff}').trim_start();
    if let Some(rest) = trimmed.strip_prefix("<?xml")
        && rest.chars().next().is_some_and(|c| c.is_ascii_whitespace() || c == '?')
        && let Some(end) = rest.find("?>")
    {
        return rest.get(end + 2..).unwrap_or_default().trim_start();
    }
    trimmed
}

fn starts_with_xdp_root(xml: &str) -> bool {
    let mut s = strip_xml_decl(xml);
    loop {
        s = s.trim_start();
        if let Some(rest) = s.strip_prefix("<!--") {
            let Some(end) = rest.find("-->") else { return false };
            s = rest.get(end + 3..).unwrap_or_default();
        } else if let Some(rest) = s.strip_prefix("<?") {
            let Some(end) = rest.find("?>") else { return false };
            s = rest.get(end + 2..).unwrap_or_default();
        } else {
            break;
        }
    }
    s.starts_with("<xdp:xdp") || s.starts_with("<xdp ") || s.starts_with("<xdp>")
}

fn first_element_tag_end(xml: &str) -> Option<(usize, usize)> {
    let b = xml.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
        } else if b[i..].starts_with(b"<!--") {
            i = (i + 4..b.len()).find(|&p| b[p..].starts_with(b"-->")).map_or(b.len(), |p| p + 3);
        } else if b[i..].starts_with(b"<?") {
            i = (i + 2..b.len()).find(|&p| b[p..].starts_with(b"?>")).map_or(b.len(), |p| p + 2);
        } else if b[i..].starts_with(b"<!") {
            i = (i + 2..b.len()).find(|&p| b[p] == b'>').map_or(b.len(), |p| p + 1);
        } else {
            let start = i;
            i += 1;
            let mut quote = 0u8;
            while i < b.len() {
                let c = b[i];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    return Some((start, i));
                }
                i += 1;
            }
            return None;
        }
    }
    None
}

fn ensure_xfa_ns_on_datasets(xml: &str) -> String {
    if !xml.contains("xfa:") {
        return xml.to_owned();
    }
    let Some((start, end)) = first_element_tag_end(xml) else { return xml.to_owned() };
    if xml.get(start..=end).is_some_and(|tag| tag.contains("xmlns:xfa=")) {
        return xml.to_owned();
    }
    let at = if xml.get(start..end).is_some_and(|s| s.ends_with('/')) { end - 1 } else { end };
    if let (Some(head), Some(tail)) = (xml.get(..at), xml.get(at..)) {
        let mut out = String::with_capacity(xml.len() + 56);
        out.push_str(head);
        out.push_str(" xmlns:xfa=\"");
        out.push_str(XFA_DATA_NS);
        out.push('"');
        out.push_str(tail);
        return out;
    }
    xml.to_owned()
}

fn wanted_datasets(shell: &Document, source: &Document) -> Result<Option<String>, XfaError> {
    let shell_ds = extract_named_packet(shell, "datasets")?;
    let source_ds = extract_named_packet(source, "datasets")?;
    let want = match (&shell_ds, source_ds) {
        (None, None) => return Ok(None),
        (Some(_), None) => EMPTY_DATASETS.to_owned(),
        (_, Some(s)) if shell_ds.as_deref() == Some(s.as_str()) => return Ok(None),
        (_, Some(s)) => s,
    };
    if want.len() > MAX_PACKET_WRITE {
        return Err(XfaError::TooLarge("the XFA datasets packet exceeds the write limit".into()));
    }
    parse_packet_xml(&want)?;
    Ok(Some(want))
}

fn effective_template(doc: &Document) -> Result<Template, XfaError> {
    if let Ok(tpl) = template_of(doc) {
        return Ok(tpl);
    }
    let tpl_elem = extract_template_element(doc)?.ok_or(XfaError::NotXfa)?;
    let form_elem = extract_named_packet(doc, "form")?.unwrap_or_default();
    let combined = format!("{XDP_OPEN}{tpl_elem}{}{XDP_CLOSE}", strip_xml_decl(&form_elem));
    Ok(crate::parse(&combined)?.0)
}

fn effective_data(doc: &Document) -> Option<DataNode> {
    data_of(doc).or_else(|| {
        extract_named_packet(doc, "datasets").ok().flatten().and_then(|ds| parse_datasets(&ensure_xfa_ns_on_datasets(strip_xml_decl(&ds))))
    })
}

fn wanted_form_packet(shell: &Document, source: &Document) -> Result<Option<String>, XfaError> {
    let source_tpl = effective_template(source)?;
    let ov = overrides(source);
    let wanted_tpl = apply_overrides(&source_tpl, &ov);
    let shell_tpl = effective_template(shell)?;
    if same_template_state(&wanted_tpl.root, &shell_tpl.root, 0) {
        return Ok(None);
    }
    let raw_xml = extract_template_element(shell)?.ok_or(XfaError::NotXfa)?;
    let (raw_tpl, _) = crate::parse(&raw_xml)?;
    let form_xml = build_form_packet(&raw_tpl.root, &wanted_tpl.root);
    if form_xml.len() > MAX_PACKET_WRITE {
        return Err(XfaError::TooLarge("the XFA form packet exceeds the write limit".into()));
    }
    Ok(Some(form_xml))
}

fn same_live_state(a: &FormNode, b: &FormNode, depth: usize) -> bool {
    if a.som != b.som || a.presence != b.presence || a.access != b.access || a.children.len() != b.children.len() {
        return false;
    }
    if depth >= 64 {
        return a.children.is_empty();
    }
    a.children.iter().zip(&b.children).all(|(x, y)| same_live_state(x, y, depth + 1))
}

fn same_template_state(a: &Subform, b: &Subform, depth: usize) -> bool {
    if a.common.presence != b.common.presence || a.children.len() != b.children.len() {
        return false;
    }
    if depth >= 64 {
        return a.children.is_empty();
    }
    a.children.iter().zip(&b.children).all(|(x, y)| same_node_state(x, y, depth + 1))
}

fn same_node_state(a: &Node, b: &Node, depth: usize) -> bool {
    match (a, b) {
        (Node::Subform(sa), Node::Subform(sb)) => same_template_state(sa, sb, depth),
        (Node::Area(aa), Node::Area(ab)) => {
            aa.common.presence == ab.common.presence
                && aa.children.len() == ab.children.len()
                && aa.children.iter().zip(&ab.children).all(|(x, y)| same_node_state(x, y, depth + 1))
        }
        (Node::Field(fa), Node::Field(fb)) => fa.common.presence == fb.common.presence && fa.access == fb.access && fa.caption == fb.caption,
        (Node::Draw(da), Node::Draw(db)) => da.common.presence == db.common.presence,
        (Node::ExclGroup(ga), Node::ExclGroup(gb)) => {
            ga.common.presence == gb.common.presence
                && ga.fields.len() == gb.fields.len()
                && ga
                    .fields
                    .iter()
                    .zip(&gb.fields)
                    .all(|(fa, fb)| fa.common.presence == fb.common.presence && fa.access == fb.access && fa.caption == fb.caption)
        }
        _ => false,
    }
}

fn presence_str(p: Presence) -> &'static str {
    match p {
        Presence::Visible => "visible",
        Presence::Hidden => "hidden",
        Presence::Invisible => "invisible",
        Presence::Inactive => "inactive",
    }
}

fn access_str(a: Access) -> &'static str {
    match a {
        Access::Open => "open",
        Access::ReadOnly => "readOnly",
        Access::Protected => "protected",
        Access::NonInteractive => "nonInteractive",
    }
}

fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

fn build_form_packet(raw: &Subform, want: &Subform) -> String {
    let inner = build_form_children(&raw.children, &want.children, 0);
    let root_changed = raw.common.presence != want.common.presence;
    if !root_changed && inner.is_empty() {
        return format!("<form xmlns=\"{XFA_FORM_NS}\"/>");
    }
    let mut attrs = String::new();
    if let Some(name) = want.common.name.as_deref().filter(|n| !n.is_empty()) {
        attrs.push_str(&format!(" name=\"{}\"", escape_attr(name)));
    }
    if root_changed {
        attrs.push_str(&format!(" presence=\"{}\"", presence_str(want.common.presence)));
    }
    if inner.is_empty() {
        format!("<form xmlns=\"{XFA_FORM_NS}\"><subform{attrs}/></form>")
    } else {
        format!("<form xmlns=\"{XFA_FORM_NS}\"><subform{attrs}>{inner}</subform></form>")
    }
}

fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

fn node_tag(n: &Node) -> &'static str {
    match n {
        Node::Subform(s) => {
            if s.set {
                "subformSet"
            } else {
                "subform"
            }
        }
        Node::Area(_) => "area",
        Node::Field(_) => "field",
        Node::Draw(_) => "draw",
        Node::ExclGroup(_) => "exclGroup",
    }
}

fn diff_form_node(raw: &Node, want: &Node, depth: usize) -> Option<String> {
    if depth > 64 {
        return None;
    }
    let name = want.common().name.as_deref().filter(|n| !n.is_empty())?;
    let tag = node_tag(want);
    let mut attrs = format!(" name=\"{}\"", escape_attr(name));
    let (changed, inner) = match (raw, want) {
        (Node::Subform(rs), Node::Subform(ws)) => {
            let p = rs.common.presence != ws.common.presence;
            if p {
                attrs.push_str(&format!(" presence=\"{}\"", presence_str(ws.common.presence)));
            }
            (p, build_form_children(&rs.children, &ws.children, depth + 1))
        }
        (Node::Area(ra), Node::Area(wa)) => {
            let p = ra.common.presence != wa.common.presence;
            if p {
                attrs.push_str(&format!(" presence=\"{}\"", presence_str(wa.common.presence)));
            }
            (p, build_form_children(&ra.children, &wa.children, depth + 1))
        }
        (Node::Field(rf), Node::Field(wf)) => {
            let p = rf.common.presence != wf.common.presence;
            let a = rf.access != wf.access;
            if p {
                attrs.push_str(&format!(" presence=\"{}\"", presence_str(wf.common.presence)));
            }
            if a {
                attrs.push_str(&format!(" access=\"{}\"", access_str(wf.access)));
            }
            let cap_inner = if rf.caption != wf.caption
                && let Some(Value::Text(t)) = wf.caption.as_ref().map(|c| &c.value)
            {
                format!("<caption><value><text>{}</text></value></caption>", escape_text(t))
            } else {
                String::new()
            };
            (p || a, cap_inner)
        }
        (Node::Draw(rd), Node::Draw(wd)) => {
            let p = rd.common.presence != wd.common.presence;
            if p {
                attrs.push_str(&format!(" presence=\"{}\"", presence_str(wd.common.presence)));
            }
            (p, String::new())
        }
        (Node::ExclGroup(rg), Node::ExclGroup(wg)) => {
            let p = rg.common.presence != wg.common.presence;
            if p {
                attrs.push_str(&format!(" presence=\"{}\"", presence_str(wg.common.presence)));
            }
            (p, String::new())
        }
        _ => return None,
    };
    if !changed && inner.is_empty() {
        return None;
    }
    Some(if inner.is_empty() { format!("<{tag}{attrs}/>") } else { format!("<{tag}{attrs}>{inner}</{tag}>") })
}

fn build_form_children(raw_nodes: &[Node], want_nodes: &[Node], depth: usize) -> String {
    let diffs: Vec<Option<String>> = raw_nodes.iter().zip(want_nodes).map(|(r, w)| diff_form_node(r, w, depth)).collect();
    let mut last_changed: HashMap<&str, usize> = HashMap::new();
    for (i, (w, d)) in want_nodes.iter().zip(&diffs).enumerate() {
        if d.is_some()
            && let Some(name) = w.common().name.as_deref().filter(|n| !n.is_empty())
        {
            last_changed.insert(name, i);
        }
    }
    let mut out = String::new();
    for (i, (w, d)) in want_nodes.iter().zip(diffs).enumerate() {
        if let Some(xml) = d {
            out.push_str(&xml);
        } else if let Some(name) = w.common().name.as_deref().filter(|n| !n.is_empty())
            && last_changed.get(name).is_some_and(|&last| i < last)
        {
            out.push_str(&format!("<{} name=\"{}\"/>", node_tag(w), escape_attr(name)));
        }
    }
    out
}

fn write_shell_packets(shell: &mut Document, ds_xml: Option<&str>, form_xml: Option<&str>) -> Result<(), XfaError> {
    let root = shell.root().ok_or(XfaError::NotXfa)?;
    let catalog = shell.get(root);
    let acro_entry = catalog.as_dict().and_then(|c| c.get(b"AcroForm")).cloned().ok_or(XfaError::NotXfa)?;
    let acro = resolve_chain(shell, &acro_entry).and_then(|o| o.as_dict().cloned()).ok_or(XfaError::NotXfa)?;
    let xfa_entry = acro.get(b"XFA").cloned().ok_or(XfaError::NotXfa)?;
    let resolved_xfa = resolve_chain(shell, &xfa_entry).ok_or(XfaError::NotXfa)?;
    match resolved_xfa {
        Object::Array(items) => write_array_packets(shell, root, &acro_entry, acro, items, ds_xml, form_xml),
        Object::Stream(_) => write_xdp_stream(shell, root, &acro_entry, acro, &xfa_entry, ds_xml, form_xml),
        _ => Err(XfaError::NotXfa),
    }
}

fn count_refs_to(doc: &Document, target_num: u32) -> Option<usize> {
    const MAX_REF_SCAN: usize = 1 << 20;
    const MAX_DEPTH: u8 = 64;
    fn walk(o: &Object, target_num: u32, depth: u8, scanned: &mut usize, hits: &mut usize) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        match o {
            Object::Ref(r) => {
                *scanned += 1;
                if r.num == target_num {
                    *hits += 1;
                }
                *scanned <= MAX_REF_SCAN
            }
            Object::Dict(d) => d.iter().all(|(_, v)| walk(v, target_num, depth + 1, scanned, hits)),
            Object::Stream(s) => s.dict.iter().all(|(_, v)| walk(v, target_num, depth + 1, scanned, hits)),
            Object::Array(a) => a.iter().all(|v| walk(v, target_num, depth + 1, scanned, hits)),
            _ => true,
        }
    }
    let (mut scanned, mut hits) = (0usize, 0usize);
    for num in doc.object_numbers() {
        let obj = doc.try_get(num).ok()?;
        if !walk(&obj, target_num, 0, &mut scanned, &mut hits) {
            return None;
        }
    }
    if !doc.trailer().iter().all(|(_, v)| walk(v, target_num, 0, &mut scanned, &mut hits)) {
        return None;
    }
    Some(hits)
}

fn unshared_direct_stream(shell: &Document, items: &[Object], name_idx: usize) -> Option<ObjRef> {
    if !name_idx.is_multiple_of(2) || shell.resolve(items.get(name_idx)?).as_string().is_none() {
        return None;
    }
    let r = items.get(name_idx + 1)?.as_ref()?;
    if !matches!(&*shell.get(r), Object::Stream(_)) {
        return None;
    }
    (count_refs_to(shell, r.num) == Some(1)).then_some(r)
}

fn default_array_encoding(shell: &Document, items: &[Object]) -> Encoding {
    find_array_packet(shell, items, "template")
        .and_then(|i| items.get(i + 1))
        .and_then(|o| decode_stream_exact(shell, o).ok())
        .map_or(Encoding::Utf8 { bom: false }, |(_, enc)| enc)
}

fn update_or_insert_packet(shell: &mut Document, items: &mut Vec<Object>, name: &str, xml: &str, fallback_enc: Encoding) -> Result<bool, XfaError> {
    let clean = if name == "datasets" { ensure_xfa_ns_on_datasets(strip_xml_decl(xml)) } else { strip_xml_decl(xml).to_owned() };
    if let Some(i) = find_array_packet(shell, items, name) {
        let existing = items.get(i + 1).cloned().ok_or(XfaError::NotXfa)?;
        let (_, enc) = decode_stream_exact(shell, &existing)?;
        let stream = Object::Stream(Stream::flate(Dict::new(), &encode(&clean, enc)));
        if let Some(r) = unshared_direct_stream(shell, items, i) {
            shell.set(r, stream);
            return Ok(false);
        }
        let r = shell.add(stream);
        if let Some(slot) = items.get_mut(i + 1) {
            *slot = Object::Ref(r);
        }
        return Ok(true);
    }
    let stream = Object::Stream(Stream::flate(Dict::new(), &encode(&clean, fallback_enc)));
    let r = shell.add(stream);
    let pos = find_array_packet(shell, items, "postamble").unwrap_or(items.len());
    items.insert(pos, Object::Ref(r));
    items.insert(pos, Object::String(PdfString::text(name)));
    Ok(true)
}

fn check_array_envelope_possible(shell: &Document, items: &[Object]) -> Result<(), XfaError> {
    if find_array_packet(shell, items, "preamble").is_some() {
        return Ok(());
    }
    if let Some((text, _)) = items.iter().find_map(|o| decode_stream_exact(shell, o).ok())
        && !starts_with_xdp_root(&text)
        && text.trim_start_matches('\u{feff}').trim_start().starts_with("<?xml")
    {
        return Err(XfaError::Malformed("cannot insert an XFA packet before an XML declaration without a preamble packet".into()));
    }
    Ok(())
}

fn ensure_array_envelope(shell: &mut Document, items: &mut Vec<Object>, enc: Encoding) {
    let has_preamble = find_array_packet(shell, items, "preamble").is_some();
    let has_postamble = find_array_packet(shell, items, "postamble").is_some();
    if has_preamble && has_postamble {
        return;
    }
    let first_has_xdp = items.iter().find_map(|o| decode_stream_exact(shell, o).ok()).is_some_and(|(t, _)| starts_with_xdp_root(&t));
    if first_has_xdp {
        return;
    }
    if !has_preamble {
        let pre = shell.add(Object::Stream(Stream::flate(Dict::new(), &encode(XDP_OPEN, enc))));
        items.insert(0, Object::Ref(pre));
        items.insert(0, Object::String(PdfString::text("preamble")));
    }
    if !has_postamble {
        let post = shell.add(Object::Stream(Stream::flate(Dict::new(), &encode(XDP_CLOSE, enc))));
        items.push(Object::String(PdfString::text("postamble")));
        items.push(Object::Ref(post));
    }
}

fn write_array_packets(
    shell: &mut Document,
    root: ObjRef,
    acro_entry: &Object,
    mut acro: Dict,
    mut items: Vec<Object>,
    ds_xml: Option<&str>,
    form_xml: Option<&str>,
) -> Result<(), XfaError> {
    let will_insert = (ds_xml.is_some() && find_array_packet(shell, &items, "datasets").is_none())
        || (form_xml.is_some() && find_array_packet(shell, &items, "form").is_none());
    if will_insert {
        check_array_envelope_possible(shell, &items)?;
    }
    let fallback = default_array_encoding(shell, &items);
    let mut array_changed = false;
    if let Some(ds) = ds_xml {
        array_changed |= update_or_insert_packet(shell, &mut items, "datasets", ds, fallback)?;
    }
    if let Some(fm) = form_xml {
        array_changed |= update_or_insert_packet(shell, &mut items, "form", fm, fallback)?;
    }
    if array_changed {
        ensure_array_envelope(shell, &mut items, fallback);
        acro.set(b"XFA".to_vec(), Object::Array(items));
        write_acroform(shell, root, acro_entry, acro)?;
    }
    Ok(())
}

fn splice_into_xdp(xdp: &str, ds_xml: Option<&str>, form_xml: Option<&str>) -> Result<String, XfaError> {
    if xml_nesting_depth(xdp) > MAX_XML_DEPTH {
        return Err(XfaError::TooLarge("the XFA stream nests XML elements deeper than the reader accepts".into()));
    }
    let parsed = roxmltree::Document::parse_with_options(xdp, xml_opts()).map_err(|e| XfaError::Malformed(e.to_string()))?;
    let root = parsed.root_element();
    if root.tag_name().name() == "template" {
        let mut out = String::new();
        let r = root.range();
        if let Some(decl) = xdp.get(..r.start) {
            out.push_str(decl);
        }
        out.push_str(XDP_OPEN);
        out.push_str(xdp.get(r).ok_or_else(|| XfaError::Malformed("invalid template range".into()))?);
        if let Some(ds) = ds_xml {
            out.push_str(&ensure_xfa_ns_on_datasets(strip_xml_decl(ds)));
        }
        if let Some(fm) = form_xml {
            out.push_str(strip_xml_decl(fm));
        }
        out.push_str(XDP_CLOSE);
        return Ok(out);
    }
    let root_range = root.range();
    let close_at = root_range.start
        + xdp
            .get(root_range.clone())
            .and_then(|s| s.rfind("</"))
            .ok_or_else(|| XfaError::Malformed("the XDP root element is self-closing".into()))?;
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let xdp_declares_xfa = root.namespaces().any(|ns| ns.name() == Some("xfa") && ns.uri() == XFA_DATA_NS);
    if let Some(ds) = ds_xml {
        let ds_clean = strip_xml_decl(ds);
        let text = if xdp_declares_xfa { ds_clean.to_owned() } else { ensure_xfa_ns_on_datasets(ds_clean) };
        match find_xdp_element(&parsed, "datasets") {
            Some(n) => edits.push((n.range().start, n.range().end, text)),
            None => edits.push((close_at, close_at, text)),
        }
    }
    if let Some(fm) = form_xml {
        let fm_clean = strip_xml_decl(fm).to_owned();
        match find_xdp_element(&parsed, "form") {
            Some(n) => edits.push((n.range().start, n.range().end, fm_clean)),
            None => edits.push((close_at, close_at, fm_clean)),
        }
    }
    edits.sort_by_key(|(s, e, _)| (*s, *e));
    let mut out = xdp.to_owned();
    for (s, e, replacement) in edits.into_iter().rev() {
        if out.get(s..e).is_none() {
            return Err(XfaError::Malformed("overlapping XDP packet ranges".into()));
        }
        out.replace_range(s..e, &replacement);
    }
    Ok(out)
}

fn write_xdp_stream(
    shell: &mut Document,
    root: ObjRef,
    acro_entry: &Object,
    mut acro: Dict,
    xfa_entry: &Object,
    ds_xml: Option<&str>,
    form_xml: Option<&str>,
) -> Result<(), XfaError> {
    let (xdp, enc) = decode_stream_exact(shell, xfa_entry)?;
    let updated = splice_into_xdp(&xdp, ds_xml, form_xml)?;
    if updated.len() > MAX_PACKET_WRITE {
        return Err(XfaError::TooLarge("the XFA stream exceeds the write limit".into()));
    }
    roxmltree::Document::parse_with_options(&updated, xml_opts()).map_err(|e| XfaError::Malformed(e.to_string()))?;
    let r = shell.add(Object::Stream(Stream::flate(Dict::new(), &encode(&updated, enc))));
    acro.set(b"XFA".to_vec(), Object::Ref(r));
    write_acroform(shell, root, acro_entry, acro)
}

fn resolve_dict_ref(doc: &Document, entry: &Object) -> Option<ObjRef> {
    let Object::Ref(mut cur) = *entry else { return None };
    let mut seen = HashSet::new();
    for _ in 0..MAX_REF_HOPS {
        if !seen.insert(cur) {
            return None;
        }
        match &*doc.get(cur) {
            Object::Ref(next) => cur = *next,
            Object::Dict(_) => return Some(cur),
            _ => return None,
        }
    }
    None
}

fn write_acroform(shell: &mut Document, root: ObjRef, acro_entry: &Object, acro: Dict) -> Result<(), XfaError> {
    if let Some(ar) = resolve_dict_ref(shell, acro_entry) {
        shell.set(ar, Object::Dict(acro));
        return Ok(());
    }
    match acro_entry {
        Object::Ref(_) => {
            let new_ar = shell.add(Object::Dict(acro));
            shell.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Ref(new_ar)))?;
            Ok(())
        }
        _ => {
            shell.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(acro)))?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, write_incremental};

    use super::*;

    fn assemble(objs: Vec<Vec<u8>>) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    fn packet_stream(bytes: &[u8]) -> Vec<u8> {
        let mut s = format!("<< /Length {} >>\nstream\n", bytes.len()).into_bytes();
        s.extend_from_slice(bytes);
        s.extend_from_slice(b"\nendstream");
        s
    }

    #[test]
    fn detects_ur3_and_ur_through_chained_indirect_references_and_ignores_null() {
        // Direct /UR3, indirect /UR, and chained /Perms -> 5 0 R -> 6 0 R -> /UR 7 0 R -> 8 0 R.
        let xdp = crate::fixtures::template(1);
        let direct = Document::open(Arc::new(crate::fixtures::usage_rights_shell(&xdp, "", false))).unwrap();
        assert!(has_usage_rights(&direct));

        let chained = assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R /Perms 5 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
            packet_stream(b""),
            b"6 0 R".to_vec(),
            b"<< /UR 7 0 R >>".to_vec(),
            b"8 0 R".to_vec(),
            b"<< /Type /Sig >>".to_vec(),
        ]);
        assert!(has_usage_rights(&Document::open(Arc::new(chained)).unwrap()));

        // `/UR3 null` is an absent entry in PDF (§7.3.7), not usage rights.
        let null_ur = assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R /Perms << /UR3 null >> >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
            packet_stream(b""),
        ]);
        assert!(!has_usage_rights(&Document::open(Arc::new(null_ur)).unwrap()));

        // Cyclic `/Perms 5 0 R -> 6 0 R -> 5 0 R` terminates cleanly and returns false.
        let cyclic = assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R /Perms 5 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
            packet_stream(b""),
            b"6 0 R".to_vec(),
            b"5 0 R".to_vec(),
        ]);
        assert!(!has_usage_rights(&Document::open(Arc::new(cyclic)).unwrap()));
    }

    #[test]
    fn sync_updates_only_the_datasets_stream_of_a_packet_array_and_preserves_utf16_encoding() {
        let preamble = b"<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\">";
        let tpl = b"<template xmlns=\"http://www.xfa.org/schema/xfa-template/3.3/\"><subform name=\"form\" layout=\"tb\"><pageSet><pageArea name=\"p\"><contentArea w=\"7in\" h=\"9in\"/><medium short=\"8.5in\" long=\"11in\"/></pageArea></pageSet><field name=\"city\" w=\"2in\" h=\"0.3in\"><ui><textEdit/></ui></field></subform></template>";
        let ds_utf16be = crate::encode_packet(
            "<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data><form><city>Old</city></form></xfa:data></xfa:datasets>",
            crate::Encoding::Utf16Be { bom: true },
        );
        let postamble = b"</xdp:xdp>";
        let shell_bytes = assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R /NeedsRendering true /AcroForm 9 0 R /Perms << /UR3 10 0 R >> >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
            packet_stream(b"BT ET"),
            packet_stream(preamble),
            packet_stream(tpl),
            packet_stream(&ds_utf16be),
            packet_stream(postamble),
            b"<< /Fields [] /XFA [(preamble) 5 0 R (template) 6 0 R (datasets) 7 0 R (postamble) 8 0 R] >>".to_vec(),
            b"<< /Type /Sig >>".to_vec(),
        ]);

        let mut source = Document::open(Arc::new(shell_bytes.clone())).unwrap();
        crate::render_into(&mut source).unwrap();
        crate::write_data_value(&mut source, &crate::som_to_path("form[0].city[0]"), "Tokyo").unwrap();

        let mut shell = Document::open(Arc::new(shell_bytes.clone())).unwrap();
        assert!(sync_form_into_shell(&mut shell, &source).unwrap());
        let edited: Vec<u32> = (1..=20).filter(|&n| shell.is_edited(n)).collect();
        assert_eq!(edited, vec![7], "only the datasets stream object (7 0 R) is rewritten: {edited:?}");

        let saved = write_incremental(&shell, &SaveOptions::default()).unwrap();
        assert!(saved.starts_with(&shell_bytes));
        let reopened = Document::open(Arc::new(saved)).unwrap();
        let Object::Stream(s) = &*reopened.get(ObjRef::new(7, 0)) else { panic!("expected stream") };
        let (_, enc) = crate::decode_packet(&s.decoded().unwrap()).unwrap();
        assert_eq!(enc, crate::Encoding::Utf16Be { bom: true });
        let packets = crate::read_packets(&reopened).unwrap().unwrap();
        let data = crate::parse_datasets(&packets.xdp).unwrap();
        assert_eq!(data.text_at(&crate::som_to_path("form[0].city[0]")), Some("Tokyo"));

        // Syncing again with no further change leaves the shell untouched.
        let mut again = reopened;
        assert!(!sync_form_into_shell(&mut again, &source).unwrap());
        assert!(!again.is_modified());
    }

    #[test]
    fn sync_writes_form_packet_for_script_presence_and_access_overrides() {
        let xdp = crate::fixtures::scripted_template();
        let shell_bytes = crate::fixtures::usage_rights_shell(&xdp, "", false);
        let mut source = Document::open(Arc::new(shell_bytes.clone())).unwrap();
        crate::render_into(&mut source).unwrap();
        let mut ov = crate::Overrides::default();
        ov.presence.insert("form[0].page1[0].details[0]".into(), "visible".into());
        ov.access.insert("form[0].page1[0].qty[0]".into(), "readOnly".into());
        crate::set_overrides(&mut source, &ov).unwrap();
        assert!(overrides_roundtrip(&source));
        crate::write_data_value(&mut source, &crate::som_to_path("form[0].page1[0].details[0].note[0]"), "Shown").unwrap();

        let mut shell = Document::open(Arc::new(shell_bytes)).unwrap();
        assert!(sync_form_into_shell(&mut shell, &source).unwrap());
        let saved = write_incremental(&shell, &SaveOptions::default()).unwrap();
        let unlaid_reopened = Document::open(Arc::new(saved)).unwrap();
        assert!(has_usage_rights(&unlaid_reopened));
        let tpl = crate::template_of(&unlaid_reopened).unwrap();
        let mut reopened = unlaid_reopened.clone();
        crate::render_into(&mut reopened).unwrap();
        let live = crate::form_tree(&tpl, crate::data_of(&reopened).as_ref(), &crate::overrides(&reopened));
        let page1 = &live.root.children[0];
        let details = page1.children.iter().find(|c| c.name == "details").unwrap();
        assert_eq!(details.presence, "visible");
        let qty = page1.children.iter().find(|c| c.name == "qty").unwrap();
        assert_eq!(qty.access, "readOnly");

        // Filling a field on the reopened shell leaves its pre-existing `<form>` packet intact,
        // and restoring `details` + `qty` to their template defaults clears `<form>` back to empty.
        let mut second_source = reopened;
        crate::write_data_value(&mut second_source, &crate::som_to_path("form[0].page1[0].details[0].note[0]"), "Updated").unwrap();
        let mut second_shell = unlaid_reopened;
        assert!(sync_form_into_shell(&mut second_shell, &second_source).unwrap());
        let saved2 = Document::open(Arc::new(write_incremental(&second_shell, &SaveOptions::default()).unwrap())).unwrap();
        let mut reset_source = saved2.clone();
        crate::render_into(&mut reset_source).unwrap();
        let mut back = crate::Overrides::default();
        back.presence.insert("form[0].page1[0].details[0]".into(), "hidden".into());
        back.access.insert("form[0].page1[0].qty[0]".into(), "open".into());
        crate::set_overrides(&mut reset_source, &back).unwrap();
        assert!(overrides_roundtrip(&reset_source));
        let mut reset_shell = saved2;
        assert!(sync_form_into_shell(&mut reset_shell, &reset_source).unwrap());
        let reopened_reset = Document::open(Arc::new(write_incremental(&reset_shell, &SaveOptions::default()).unwrap())).unwrap();
        let tpl_reset = crate::template_of(&reopened_reset).unwrap();
        let live_reset = crate::form_tree(&tpl_reset, crate::data_of(&reopened_reset).as_ref(), &crate::Overrides::default());
        assert_eq!(live_reset.root.children[0].children.iter().find(|c| c.name == "details").unwrap().presence, "hidden");
        assert_eq!(live_reset.root.children[0].children.iter().find(|c| c.name == "qty").unwrap().access, "open");

        // Per-instance override on `table[0].row[1]` (where `row[0]` stays visible) cannot be
        // represented in XFA's template-level `<form>` packet, so `overrides_roundtrip` refuses it.
        crate::add_data_instances(&mut reset_source, &crate::som_to_path("form[0].page1[0].table[0]"), "row", 2).unwrap();
        let mut row1_ov = crate::Overrides::default();
        row1_ov.presence.insert("form[0].page1[0].table[0].row[1]".into(), "hidden".into());
        crate::set_overrides(&mut reset_source, &row1_ov).unwrap();
        assert!(!overrides_roundtrip(&reset_source));
        let _ = Dict::new();
    }

    #[test]
    fn sync_allocates_new_streams_when_packets_are_missing_shared_or_named_indirectly() {
        let tpl = b"<template xmlns=\"http://www.xfa.org/schema/xfa-template/3.3/\"><subform name=\"form\" layout=\"tb\"><pageSet><pageArea name=\"p\"><contentArea w=\"7in\" h=\"9in\"/><medium short=\"8.5in\" long=\"11in\"/></pageArea></pageSet><field name=\"city\" w=\"2in\" h=\"0.3in\"><ui><textEdit/></ui></field></subform></template>";

        // 1. Envelope-less `[(template) 5 0 R]` inserts `datasets` plus `preamble`/`postamble` without touching 5 0 R.
        let bare_array = assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R /NeedsRendering true /AcroForm 6 0 R /Perms << /UR3 7 0 R >> >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
            packet_stream(b"BT ET"),
            packet_stream(tpl),
            b"<< /Fields [] /XFA [(template) 5 0 R] >>".to_vec(),
            b"<< /Type /Sig >>".to_vec(),
        ]);
        let mut src = Document::open(Arc::new(bare_array.clone())).unwrap();
        crate::render_into(&mut src).unwrap();
        crate::write_data_value(&mut src, &crate::som_to_path("form[0].city[0]"), "Rome").unwrap();
        let mut sh = Document::open(Arc::new(bare_array)).unwrap();
        assert!(sync_form_into_shell(&mut sh, &src).unwrap());
        assert!(!sh.is_edited(5), "template stream 5 0 R is untouched");
        let saved = Document::open(Arc::new(write_incremental(&sh, &SaveOptions::default()).unwrap())).unwrap();
        let xdp = crate::read_packets(&saved).unwrap().unwrap().xdp;
        assert_eq!(crate::parse_datasets(&xdp).unwrap().text_at(&crate::som_to_path("form[0].city[0]")), Some("Rome"));

        // 2. Shared stream (`(datasets) 4 0 R` also used as page `/Contents 4 0 R`) and indirect packet name `8 0 R` = `(datasets)`:
        //    allocates a new stream instead of overwriting `4 0 R`.
        let ds_bytes =
            b"<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data><form><city>Old</city></form></xfa:data></xfa:datasets>";
        let shared_and_indirect = assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R /NeedsRendering true /AcroForm 9 0 R /Perms << /UR3 10 0 R >> >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
            packet_stream(ds_bytes),
            packet_stream(b"<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\">"),
            packet_stream(tpl),
            packet_stream(b"</xdp:xdp>"),
            b"(datasets)".to_vec(),
            b"<< /Fields [] /XFA [(preamble) 5 0 R (template) 6 0 R 8 0 R 4 0 R (postamble) 7 0 R] >>".to_vec(),
            b"<< /Type /Sig >>".to_vec(),
        ]);
        let mut sh2 = Document::open(Arc::new(shared_and_indirect)).unwrap();
        assert!(sync_form_into_shell(&mut sh2, &src).unwrap());
        assert!(!sh2.is_edited(4), "shared page /Contents stream 4 0 R must not be overwritten in place");
        let saved2 = Document::open(Arc::new(write_incremental(&sh2, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(
            crate::parse_datasets(&crate::read_packets(&saved2).unwrap().unwrap().xdp).unwrap().text_at(&crate::som_to_path("form[0].city[0]")),
            Some("Rome")
        );

        // 3. Over-deep XML packet (> MAX_XML_DEPTH) fails closed with `XfaError::TooLarge` without panicking.
        let mut deep_ds = String::from("<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data>");
        for _ in 0..=MAX_XML_DEPTH {
            deep_ds.push_str("<n>");
        }
        for _ in 0..=MAX_XML_DEPTH {
            deep_ds.push_str("</n>");
        }
        deep_ds.push_str("</xfa:data></xfa:datasets>");
        let deep_pdf = assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
            packet_stream(b""),
            b"<< /Fields [] /XFA [(template) 6 0 R (datasets) 7 0 R] >>".to_vec(),
            packet_stream(tpl),
            packet_stream(deep_ds.as_bytes()),
        ]);
        let deep_src = Document::open(Arc::new(deep_pdf)).unwrap();
        let err = sync_form_into_shell(&mut sh2, &deep_src).unwrap_err();
        assert!(matches!(err, XfaError::TooLarge(_)), "expected TooLarge, got {err:?}");
    }

    #[test]
    fn handles_xml_prolog_comments_chained_acroform_and_unwrappable_shells() {
        let tpl = b"<template xmlns=\"http://www.xfa.org/schema/xfa-template/3.3/\"><subform name=\"form\" layout=\"tb\"><pageSet><pageArea name=\"p\"><contentArea w=\"7in\" h=\"9in\"/><medium short=\"8.5in\" long=\"11in\"/></pageArea></pageSet><field name=\"city\" w=\"2in\" h=\"0.3in\"><ui><textEdit/></ui></field></subform></template>";

        // 1. Bare packet array whose template starts with `<?xml` and lacks `preamble`/`datasets`
        //    is rejected upfront by `can_preserve_usage_rights` so the engine falls back cleanly.
        let mut prolog_tpl = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n".to_vec();
        prolog_tpl.extend_from_slice(tpl);
        let unwrappable = assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R /NeedsRendering true /AcroForm 6 0 R /Perms << /UR3 7 0 R >> >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
            packet_stream(b""),
            packet_stream(&prolog_tpl),
            b"<< /Fields [] /XFA [(template) 5 0 R] >>".to_vec(),
            b"<< /Type /Sig >>".to_vec(),
        ]);
        let unwrappable_doc = Document::open(Arc::new(unwrappable)).unwrap();
        assert!(has_usage_rights(&unwrappable_doc));
        assert!(!can_preserve_usage_rights(&unwrappable_doc));

        // 2. Datasets packet with `<?xml ...?>`, `<!-- comment > here -->` and missing `xmlns:xfa`
        //    splices cleanly into a single-stream XDP whose `/AcroForm` is chained `6 0 R -> 7 0 R`,
        //    updating `7 0 R` without touching Catalog `1 0 R`.
        let xdp_shell = assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R /NeedsRendering true /AcroForm 6 0 R /Perms << /UR3 8 0 R >> >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
            packet_stream(b""),
            packet_stream(tpl),
            b"7 0 R".to_vec(),
            b"<< /Fields [] /XFA 5 0 R >>".to_vec(),
            b"<< /Type /Sig >>".to_vec(),
        ]);
        let tricky_ds =
            b"<?xml version=\"1.0\"?>\n<!-- a > b --><xfa:datasets tag=\"x>y\"><xfa:data><form><city>Oslo</city></form></xfa:data></xfa:datasets>";
        let src_pdf = assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
            packet_stream(b""),
            b"<< /Fields [] /XFA [(template) 6 0 R (datasets) 7 0 R] >>".to_vec(),
            packet_stream(tpl),
            packet_stream(tricky_ds),
        ]);
        let mut shell = Document::open(Arc::new(xdp_shell)).unwrap();
        assert!(can_preserve_usage_rights(&shell));
        let source = Document::open(Arc::new(src_pdf)).unwrap();
        assert!(sync_form_into_shell(&mut shell, &source).unwrap());
        assert!(!shell.is_edited(1), "chained AcroForm 6 0 R -> 7 0 R updates 7 0 R without touching Catalog 1 0 R");
        assert!(shell.is_edited(7));
        let saved = Document::open(Arc::new(write_incremental(&shell, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(
            crate::parse_datasets(&crate::read_packets(&saved).unwrap().unwrap().xdp).unwrap().text_at(&crate::som_to_path("form[0].city[0]")),
            Some("Oslo")
        );

        // 3. Same prolog/comment/missing-ns datasets packet synced into an `/XFA` packet-array shell
        //    strips the mid-stream XML declaration so concatenated packets remain well-formed XML.
        let arr_xdp = format!("{XDP_OPEN}{}{XDP_CLOSE}", std::str::from_utf8(tpl).unwrap());
        let mut arr_shell = Document::open(Arc::new(crate::fixtures::usage_rights_shell(&arr_xdp, "", true))).unwrap();
        assert!(sync_form_into_shell(&mut arr_shell, &source).unwrap());
        let arr_saved = Document::open(Arc::new(write_incremental(&arr_shell, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(
            crate::parse_datasets(&crate::read_packets(&arr_saved).unwrap().unwrap().xdp).unwrap().text_at(&crate::som_to_path("form[0].city[0]")),
            Some("Oslo")
        );
    }
}
