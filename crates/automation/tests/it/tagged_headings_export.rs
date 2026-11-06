//! #3659: real Office exports retain the heading levels of a tagged PDF.
use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};
use std::io::Read;

/// Run an automation tool and require a successful JSON result for this fixture.
fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    a.call(tool, &args)
        .unwrap()
        .into_iter()
        .find_map(|c| match c {
            Content::Json(v) => Some(v),
            _ => None,
        })
        .unwrap()
}

/// Build a synthetic PDF whose structure roles disagree with font-size heading guesses.
fn tagged_pdf() -> Vec<u8> {
    let mut content = String::new();
    let mut elements = Vec::new();
    let mut ids = Vec::new();
    for level in 1..=6 {
        let mcid = elements.len();
        let size = match level {
            1 => 16,
            2 => 14,
            3 => 13,
            _ => 12,
        };
        let y = 760 - mcid * 42;
        content.push_str(&format!("/Span <</MCID {mcid}>> BDC BT /F1 {size} Tf 50 {y} Td (Heading {level}) Tj ET EMC\n"));
        ids.push(format!("{} 0 R", 8 + elements.len()));
        elements.push(format!("<< /Type /StructElem /S /H{level} /P 7 0 R /Pg 3 0 R /K {mcid} >>"));
    }
    // A large P is body text, including when its text duplicates a heading.
    for (text, size) in [("Heading 1", 24), ("Body text repeated often enough to define the ordinary twelve point font size.", 12)] {
        let mcid = elements.len();
        let y = 760 - mcid * 42;
        content.push_str(&format!("/Span <</MCID {mcid}>> BDC BT /F1 {size} Tf 50 {y} Td ({text}) Tj ET EMC\n"));
        ids.push(format!("{} 0 R", 8 + elements.len()));
        elements.push(format!("<< /Type /StructElem /S /P /P 7 0 R /Pg 3 0 R /K {mcid} >>"));
    }
    let parent_tree = 8 + elements.len();
    let objects: Vec<String> = [
        "<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 6 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 842] /Contents 5 0 R /StructParents 0 /Resources << /Font << /F1 4 0 R >> >> >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
        format!("<< /Type /StructTreeRoot /K [7 0 R] /ParentTree {parent_tree} 0 R >>"),
        format!("<< /Type /StructElem /S /Document /P 6 0 R /K [{}] >>", ids.join(" ")),
    ]
    .into_iter()
    .chain(elements)
    .chain([format!("<< /Nums [0 [{}]] >>", ids.join(" "))])
    .collect();
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

/// Read one XML member from an exported Office ZIP package.
fn part(zip: &[u8], name: &str) -> String {
    let mut at = 0;
    while zip.get(at..at + 4) == Some(b"PK\x03\x04") {
        let size = u32::from_le_bytes(zip[at + 18..at + 22].try_into().unwrap()) as usize;
        let name_len = u16::from_le_bytes(zip[at + 26..at + 28].try_into().unwrap()) as usize;
        let extra = u16::from_le_bytes(zip[at + 28..at + 30].try_into().unwrap()) as usize;
        let data = at + 30 + name_len + extra;
        if &zip[at + 30..at + 30 + name_len] == name.as_bytes() {
            let mut text = String::new();
            flate2::read::DeflateDecoder::new(&zip[data..data + size]).read_to_string(&mut text).unwrap();
            return text;
        }
        at = data + size;
    }
    panic!("missing ZIP member {name}");
}

/// Export the standard tagged fixture through the requested automation tool.
fn export(ext: &str) -> Vec<u8> {
    export_pdf(tagged_pdf(), ext)
}

/// Open supplied PDF bytes and export them through the automation API.
fn export_pdf(input: Vec<u8>, ext: &str) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("headings.pdf"), &input).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut a, "doc_open", json!({"path": "headings.pdf"}))["doc"].as_u64().unwrap();
    let before = call(&mut a, "doc_info", json!({"doc": doc}));
    call(&mut a, "doc_export_office", json!({"doc":doc,"path":format!("headings.{ext}")}));
    assert_eq!(call(&mut a, "doc_info", json!({"doc":doc})), before, "export is read-only");
    assert_eq!(std::fs::read(dir.path().join("headings.pdf")).unwrap(), input);
    std::fs::read(dir.path().join(format!("headings.{ext}"))).unwrap()
}

/// Export explicit H1-H6 levels and paragraphs to HTML regardless of their font sizes.
#[test]
fn html_export_preserves_tagged_heading_levels() {
    let html = String::from_utf8(export("html")).unwrap();
    for level in 1..=6 {
        assert!(html.contains(&format!("<h{level}>Heading {level}</h{level}>")), "{html}");
    }
    assert!(html.contains("<p>Heading 1</p>"), "a large tagged P stays a paragraph: {html}");
}

/// Export explicit heading levels into Word paragraph styles.
#[test]
fn word_export_preserves_tagged_heading_levels() {
    let zip = export("docx");
    let xml = part(&zip, "word/document.xml");
    let styles = part(&zip, "word/styles.xml");
    for level in 1..=6 {
        let heading = format!("Heading {level}");
        let para = xml.split("<w:p>").find(|p| p.contains(&format!(">{heading}</w:t>"))).unwrap();
        assert!(para.contains(&format!("<w:pStyle w:val=\"Heading{level}\"/>")), "{para}");
        let style = styles.split("<w:style ").find(|s| s.contains(&format!("w:styleId=\"Heading{level}\""))).unwrap();
        assert!(style.contains(&format!("<w:outlineLvl w:val=\"{}\"/>", level - 1)), "{style}");
    }
    let copies: Vec<_> = xml.split("<w:p>").filter(|p| p.contains(">Heading 1</w:t>")).collect();
    assert_eq!(copies.len(), 2, "{xml}");
    assert!(!copies[1].contains("<w:pStyle"), "a large P does not become a heading: {}", copies[1]);
}

/// Apply a synthetic document mutation and serialize the resulting PDF.
fn modified_pdf(edit: impl FnOnce(&mut pdfcraft_cos::Document)) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(tagged_pdf())).unwrap();
    edit(&mut doc);
    pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap()
}
/// Read the original fixture's page content for controlled stream mutations.
fn source_content(doc: &pdfcraft_cos::Document) -> String {
    let stream = doc.get(pdfcraft_cos::ObjRef::new(5, 0));
    let pdfcraft_cos::Object::Stream(stream) = &*stream else { panic!("content stream") };
    String::from_utf8(stream.decoded().unwrap()).unwrap()
}
/// Replace the fixture page's content with one synthetic stream.
fn set_content(doc: &mut pdfcraft_cos::Document, content: &str) {
    doc.set(pdfcraft_cos::ObjRef::new(5, 0), pdfcraft_cos::Stream::flate(pdfcraft_cos::Dict::new(), content.as_bytes()));
}

/// Keep one aliased heading together through nested spans and split content streams.
#[test]
fn rolemapped_heading_keeps_nested_spans_across_content_streams_together() {
    use pdfcraft_cos::{Dict, ObjRef, Object, Stream};
    let pdf = modified_pdf(|doc| {
        let mut roles = Dict::new();
        roles.set("Chapter", Object::name("H4"));
        doc.update_dict(ObjRef::new(6, 0), |d| d.set("RoleMap", Object::Dict(roles))).unwrap();
        let mut child = Dict::new();
        child.set("Type", Object::name("StructElem"));
        child.set("S", Object::name("Span"));
        child.set("P", Object::Ref(ObjRef::new(8, 0)));
        child.set("K", Object::Int(8));
        let child = doc.add(Object::Dict(child));
        doc.update_dict(ObjRef::new(8, 0), |d| {
            d.set("S", Object::name("Chapter"));
            d.set("K", Object::Array(vec![Object::Int(0), Object::Ref(child)]));
        })
        .unwrap();
        let parents: Vec<Object> = (8..16).map(|i| Object::Ref(ObjRef::new(i, 0))).chain([Object::Ref(child)]).collect();
        doc.update_dict(ObjRef::new(16, 0), |d| d.set("Nums", Object::Array(vec![Object::Int(0), Object::Array(parents)]))).unwrap();
        let mut resource = doc.get(ObjRef::new(3, 0)).as_dict().unwrap().get(b"Resources").unwrap().as_dict().unwrap().clone();
        let mut properties = Dict::new();
        let mut mark = Dict::new();
        mark.set("MCID", Object::Int(0));
        properties.set("M0", Object::Dict(mark));
        resource.set("Properties", Object::Dict(properties));
        let content = source_content(doc);
        let (_, rest) = content.split_once('\n').unwrap();
        let content = format!("/Span /M0 BDC BT /F1 16 Tf 50 760 Td (Head) Tj /Span <</MCID 8>> BDC (ing 1) Tj EMC ET EMC\n{rest}");
        let split = content.find("MCID 8").unwrap() + 5;
        let first = doc.add(Object::Stream(Stream::flate(Dict::new(), &content.as_bytes()[..split])));
        let second = doc.add(Object::Stream(Stream::flate(Dict::new(), &content.as_bytes()[split..])));
        doc.update_dict(ObjRef::new(3, 0), |d| {
            d.set("Resources", Object::Dict(resource));
            d.set("Contents", Object::Array(vec![Object::Ref(first), Object::Ref(second)]));
        })
        .unwrap();
    });
    let html = String::from_utf8(export_pdf(pdf, "html")).unwrap();
    assert_eq!(html.matches("<h4>Heading 1</h4>").count(), 1, "{html}");
    assert!(html.contains("<p>Heading 1</p>"), "{html}");
    assert!(!html.contains(">Head</") && !html.contains(">ing 1</"), "{html}");
}

/// Keep form and page MCID roles distinct in both HTML and Word export.
#[test]
fn form_and_page_marks_with_the_same_mcid_keep_distinct_roles() {
    use pdfcraft_cos::{Dict, ObjRef, Object, Stream};
    let pdf = modified_pdf(|doc| {
        let content = source_content(doc);
        let (first, rest) = content.split_once('\n').unwrap();
        let mut resources = doc.get(ObjRef::new(3, 0)).as_dict().unwrap().get(b"Resources").unwrap().as_dict().unwrap().clone();
        let mut fd = Dict::new();
        fd.set("Type", Object::name("XObject"));
        fd.set("Subtype", Object::name("Form"));
        fd.set("BBox", Object::Array(vec![0, 0, 612, 842].into_iter().map(Object::Int).collect()));
        fd.set("Resources", Object::Dict(resources.clone()));
        fd.set("StructParents", Object::Int(1));
        let form = doc.add(Object::Stream(Stream::flate(fd, first.as_bytes())));
        let mut xobjects = Dict::new();
        xobjects.set("Fm", Object::Ref(form));
        resources.set("XObject", Object::Dict(xobjects));
        doc.update_dict(ObjRef::new(3, 0), |d| d.set("Resources", Object::Dict(resources))).unwrap();
        set_content(doc, &format!("/Fm Do\n{}", rest.replace("MCID 6", "MCID 0")));
        let mut mcr = Dict::new();
        mcr.set("Type", Object::name("MCR"));
        mcr.set("Pg", Object::Ref(ObjRef::new(3, 0)));
        mcr.set("Stm", Object::Ref(form));
        mcr.set("MCID", Object::Int(0));
        doc.update_dict(ObjRef::new(8, 0), |d| d.set("K", Object::Dict(mcr))).unwrap();
        doc.update_dict(ObjRef::new(14, 0), |d| d.set("K", Object::Int(0))).unwrap();
        let parents: Vec<Object> =
            [14, 9, 10, 11, 12, 13, 0, 15].into_iter().map(|n| if n == 0 { Object::Null } else { Object::Ref(ObjRef::new(n, 0)) }).collect();
        doc.update_dict(ObjRef::new(16, 0), |d| {
            d.set(
                "Nums",
                Object::Array(vec![Object::Int(0), Object::Array(parents), Object::Int(1), Object::Array(vec![Object::Ref(ObjRef::new(8, 0))])]),
            )
        })
        .unwrap();
    });
    for ext in ["html", "docx"] {
        let out = export_pdf(pdf.clone(), ext);
        if ext == "html" {
            let html = String::from_utf8(out).unwrap();
            assert!(html.contains("<h1>Heading 1</h1>") && html.contains("<p>Heading 1</p>"), "{html}");
        } else {
            let xml = part(&out, "word/document.xml");
            let copies: Vec<_> = xml.split("<w:p>").filter(|p| p.contains(">Heading 1</w:t>")).collect();
            assert_eq!(copies.len(), 2, "{xml}");
            assert!(copies[0].contains("w:val=\"Heading1\"") && !copies[1].contains("<w:pStyle"), "{xml}");
        }
    }
}

/// Retain size-based heading inference for untagged content and unnumbered H roles.
#[test]
fn untagged_and_unnumbered_headings_keep_the_size_heuristic() {
    use pdfcraft_cos::{ObjRef, Object};
    let untagged = modified_pdf(|doc| {
        doc.update_dict(ObjRef::new(1, 0), |d| {
            d.remove(b"StructTreeRoot");
            d.remove(b"MarkInfo");
        })
        .unwrap();
    });
    let html = String::from_utf8(export_pdf(untagged, "html")).unwrap();
    assert!(html.contains("<h2>Heading 1</h2>") && html.contains("<h1>Heading 1</h1>"), "{html}");
    let generic = modified_pdf(|doc| {
        doc.update_dict(ObjRef::new(8, 0), |d| d.set("S", Object::name("H"))).unwrap();
    });
    let html = String::from_utf8(export_pdf(generic, "html")).unwrap();
    assert!(html.contains("<h2>Heading 1</h2>") && html.contains("<p>Heading 1</p>"), "{html}");
}

/// Fall back on cyclic structure data while leaving input PDF bytes unchanged.
#[test]
fn broken_structure_cycles_fall_back_without_changing_the_input() {
    use pdfcraft_cos::{ObjRef, Object};
    let pdf = modified_pdf(|doc| {
        doc.update_dict(ObjRef::new(8, 0), |d| d.set("K", Object::Ref(ObjRef::new(8, 0)))).unwrap();
    });
    let html = String::from_utf8(export_pdf(pdf, "html")).unwrap();
    assert!(html.contains("<h2>Heading 1</h2>") && html.contains("<p>Heading 1</p>"), "{html}");
}

/// Build nested form invocations with an optional local heading or paragraph override.
fn inherited_form_pdf(depth: usize, own_level: Option<u8>, tail: bool) -> Vec<u8> {
    use pdfcraft_cos::{Dict, ObjRef, Object, Stream};
    assert!(depth > 0);
    modified_pdf(|doc| {
        let content = source_content(doc);
        let (first, rest) = content.split_once('\n').unwrap();
        let text = first.strip_prefix("/Span <</MCID 0>> BDC ").unwrap().strip_suffix(" EMC").unwrap();
        let resources = doc.get(ObjRef::new(3, 0)).as_dict().unwrap().get(b"Resources").unwrap().as_dict().unwrap().clone();
        let mut next = None;
        for level in 0..depth {
            let mut local = resources.clone();
            let mut body = if let Some(inner) = next {
                let mut objects = Dict::new();
                objects.set("N", Object::Ref(inner));
                local.set("XObject", Object::Dict(objects));
                "/N Do".to_owned()
            } else if own_level.is_some() {
                format!("/Span <</MCID 0>> BDC {text} EMC")
            } else {
                text.to_owned()
            };
            if level == 0 && tail {
                body.push_str(" BT /F1 12 Tf 50 740 Td (Restored heading) Tj ET");
            }
            let mut dict = Dict::new();
            dict.set("Type", Object::name("XObject"));
            dict.set("Subtype", Object::name("Form"));
            dict.set("BBox", Object::Array(vec![0, 0, 612, 842].into_iter().map(Object::Int).collect()));
            dict.set("Resources", Object::Dict(local));
            if level == 0 && own_level.is_some() {
                dict.set("StructParents", Object::Int(1));
            }
            let form = doc.add(Object::Stream(Stream::flate(dict, body.as_bytes())));
            if level == 0
                && let Some(heading) = own_level
            {
                let mut mcr = Dict::new();
                mcr.set("Type", Object::name("MCR"));
                mcr.set("Pg", Object::Ref(ObjRef::new(3, 0)));
                mcr.set("Stm", Object::Ref(form));
                mcr.set("MCID", Object::Int(0));
                let mut tag = Dict::new();
                tag.set("Type", Object::name("StructElem"));
                let role = if heading == 0 { "P".to_owned() } else { format!("H{heading}") };
                tag.set("S", Object::name(&role));
                tag.set("P", Object::Ref(ObjRef::new(8, 0)));
                tag.set("K", Object::Dict(mcr));
                let tag = doc.add(Object::Dict(tag));
                doc.update_dict(ObjRef::new(8, 0), |d| d.set("K", Object::Array(vec![Object::Int(0), Object::Ref(tag)]))).unwrap();
                doc.update_dict(ObjRef::new(16, 0), |d| {
                    let mut nums = d.get(b"Nums").unwrap().as_array().unwrap().to_vec();
                    nums.extend([Object::Int(1), Object::Array(vec![Object::Ref(tag)])]);
                    d.set("Nums", Object::Array(nums));
                })
                .unwrap();
            }
            next = Some(form);
        }
        let mut objects = Dict::new();
        objects.set("F", Object::Ref(next.unwrap()));
        let mut resources = resources;
        resources.set("XObject", Object::Dict(objects));
        doc.update_dict(ObjRef::new(3, 0), |d| d.set("Resources", Object::Dict(resources))).unwrap();
        set_content(doc, &format!("/Span <</MCID 0>> BDC /F Do EMC\n{rest}"));
    })
}

/// Check the expected form heading semantics in both HTML and Word output.
fn assert_form_export_level(output: &[u8], ext: &str, text: &str, level: u8) {
    if ext == "html" {
        let html = std::str::from_utf8(output).unwrap();
        let tag = if level == 0 { "p".to_owned() } else { format!("h{level}") };
        let paragraph = html.lines().find(|line| line.contains(&format!(">{text}</"))).unwrap();
        assert!(paragraph.contains(&format!("<{tag}>{text}</{tag}>")), "{paragraph}");
    } else {
        let xml = part(output, "word/document.xml");
        let paragraph = xml.split("<w:p>").find(|p| p.contains(&format!(">{text}</w:t>"))).unwrap();
        if level == 0 {
            assert!(!paragraph.contains("<w:pStyle"), "{paragraph}");
        } else {
            assert!(paragraph.contains(&format!("<w:pStyle w:val=\"Heading{level}\"/>")), "{paragraph}");
        }
    }
}

/// Inherit a page heading through form invocations without local marked content.
#[test]
fn form_export_inherits_a_page_heading_through_unmarked_nested_forms() {
    for depth in [1, 3] {
        for ext in ["html", "docx"] {
            let output = export_pdf(inherited_form_pdf(depth, None, false), ext);
            assert_form_export_level(&output, ext, "Heading 1", 1);
            assert_form_export_level(&output, ext, "Heading 2", 2);
        }
    }
}

/// Let a form's explicit heading or paragraph override the invoking page heading.
#[test]
fn form_export_own_heading_and_paragraph_override_the_page_heading() {
    for level in [0, 3] {
        for ext in ["html", "docx"] {
            let output = export_pdf(inherited_form_pdf(2, Some(level), false), ext);
            assert_form_export_level(&output, ext, "Heading 1", level);
        }
    }
}

/// Resume the inherited heading after a form closes its own paragraph scope.
#[test]
fn form_export_restores_the_parent_heading_after_its_own_paragraph() {
    for ext in ["html", "docx"] {
        let output = export_pdf(inherited_form_pdf(2, Some(0), true), ext);
        assert_form_export_level(&output, ext, "Heading 1", 0);
        assert_form_export_level(&output, ext, "Restored heading", 1);
    }
}

/// Invalid role-map values leave inherited headings and valid aliases usable in both exports.
#[test]
fn malformed_large_rolemap_values_keep_inherited_and_aliased_export_levels() {
    use pdfcraft_cos::{Dict, ObjRef, Object, PdfString};
    let pdf = modified_pdf(|doc| {
        let mut payload = Dict::new();
        payload.set("Ignored", Object::String(PdfString::literal(vec![b'x'; 256 * 1024])));
        let mut roles = Dict::new();
        roles.set("Broken", Object::Array(vec![Object::Dict(payload)]));
        roles.set("Chapter", Object::Ref(doc.add(Object::name("H4"))));
        let mut inherited = Dict::new();
        inherited.set("S", Object::name("Broken"));
        inherited.set("K", Object::Int(0));
        let inherited = doc.add(Object::Dict(inherited));
        doc.update_dict(ObjRef::new(8, 0), |d| d.set("K", Object::Array(vec![Object::Ref(inherited); 256]))).unwrap();
        doc.update_dict(ObjRef::new(9, 0), |d| d.set("S", Object::name("Chapter"))).unwrap();
        doc.update_dict(ObjRef::new(6, 0), |d| d.set("RoleMap", Object::Dict(roles))).unwrap();
    });
    for ext in ["html", "docx"] {
        let output = export_pdf(pdf.clone(), ext);
        assert_form_export_level(&output, ext, "Heading 1", 1);
        assert_form_export_level(&output, ext, "Heading 2", 4);
    }
}
