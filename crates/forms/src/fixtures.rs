//! Synthetic forms for tests in this crate and the ones above it (no real data).

/// An AcroForm (no XFA) whose widgets run a JavaScript on each annotation trigger of ISO
/// 32000-2 Table 197 (E, X, D, U, Fo, Bl), adding `<field> <event.name>` to the `log` field
/// (separated by `|`): the shape of the form another viewer's order was measured on. Text fields
/// `a` and `b` also log their commit keystroke (`a Keystroke commit`) and `Validate`; `btn` is a
/// push button (Mouse Up in `/A`) and `cb` a check box; `log` is read-only. Each script starts
/// with a comment naming the key it is under (`/* Fo */`, `/* A */`).
pub fn trigger_form() -> Vec<u8> {
    let mut objs: Vec<String> = vec![String::new(); 3]; // 1 catalog, 2 pages, 3 page
    let lit = |s: &str| format!("({})", s.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)"));
    let log = |what: &str| format!(r#"var l = this.getField("log"); l.value = (l.value == "" ? "" : l.value + "|") + {what};"#);
    let add = |objs: &mut Vec<String>, body: String| {
        objs.push(body);
        objs.len()
    };
    let js = |objs: &mut Vec<String>, code: &str| add(objs, format!("<< /Type /Action /S /JavaScript /JS {} >>", lit(code)));
    let aa = |objs: &mut Vec<String>, name: &str, triggers: &[&str], typed: bool| {
        // Each script names the key it is under, in a comment.
        let mut parts: Vec<String> =
            triggers.iter().map(|t| format!("/{t} {} 0 R", js(objs, &format!("/* {t} */ {}", log(&format!(r#""{name} " + event.name"#)))))).collect();
        if typed {
            let commit = format!("if (event.willCommit) {{ {} }}", log(&format!(r#""{name} Keystroke commit""#)));
            parts.push(format!("/K {} 0 R", js(objs, &commit)));
            parts.push(format!("/V {} 0 R", js(objs, &log(&format!(r#""{name} Validate""#)))));
        }
        format!("<< {} >>", parts.join(" "))
    };
    let all = ["E", "X", "D", "U", "Fo", "Bl"];
    let mut fields = Vec::new();
    for (name, rect) in [("a", "72 640 272 664"), ("b", "320 640 520 664")] {
        let a = aa(&mut objs, name, &all, true);
        let body =
            format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /Rect [{rect}] /F 4 /P 3 0 R /DA (/Helv 12 Tf 0 g) /V () /AA {a} >>");
        fields.push(add(&mut objs, body));
    }
    let up = js(&mut objs, &format!("/* A */ {}", log(r#""btn " + event.name"#)));
    let a = aa(&mut objs, "btn", &["E", "X", "D", "Fo", "Bl"], false);
    let body = format!(
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (btn) /Rect [72 580 192 604] /F 4 /P 3 0 R /DA (/Helv 12 Tf 0 g) /MK << /CA (Button) >> /A {up} 0 R /AA {a} >>"
    );
    fields.push(add(&mut objs, body));
    let stream =
        |content: &str| format!("<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length {} >>\nstream\n{content}\nendstream", content.len());
    let on = add(&mut objs, stream("0 0 0 rg 4 4 12 12 re f"));
    let off = add(&mut objs, stream(""));
    let a = aa(&mut objs, "cb", &all, false);
    let body = format!(
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (cb) /Rect [320 582 340 602] /F 4 /P 3 0 R /V /Off /AS /Off /AP << /N << /Yes {on} 0 R /Off {off} 0 R >> >> /AA {a} >>"
    );
    fields.push(add(&mut objs, body));
    let body =
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /Ff 4097 /Rect [72 300 540 520] /F 4 /P 3 0 R /DA (/Helv 9 Tf 0 g) /V () >>".to_string();
    fields.push(add(&mut objs, body));
    let helv = add(&mut objs, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into());
    let refs: Vec<String> = fields.iter().map(|f| format!("{f} 0 R")).collect();
    let refs = refs.join(" ");
    objs[0] =
        format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{refs}] /DA (/Helv 12 Tf 0 g) /DR << /Font << /Helv {helv} 0 R >> >> >> >>");
    objs[1] = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into();
    objs[2] = format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /Helv {helv} 0 R >> >> /Annots [{refs}] >>");
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}
