use super::marks::{
    Background, Context, HeaderFooter, MarkKind, Watermark, add_picture_resource, bates_start, expand, matrix_number, n, picture, rgb, shrink_matrix,
};
use super::place::{
    END, Isolation, add_page_resources, add_resources, begin, check, check_on_top, contents, free_font_name, isolation, page_fonts, page_list, place,
    place_inside_wrapper, stream_tag, text_op, undrawable, wrapper_role,
};
use super::{Document, EditError, Object, added, helvetica_width};

/// Add a header and footer to `pages` (0-based). With `replace`, existing headers and footers on
/// those pages are removed first (Acrobat's Replace Existing).
pub fn add_header_footer(doc: &mut Document, pages: &[usize], hf: &HeaderFooter, replace: bool, cx: &Context) -> Result<(), EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    if hf.text.iter().all(|t| t.trim().is_empty()) {
        return Err(EditError::Invalid("type the header or footer text first".into()));
    }
    if let Some(c) = hf.first_undrawable() {
        return Err(undrawable(c));
    }
    if !(hf.font_size.is_finite() && hf.font_size > 0.0 && hf.font_size <= 200.0 && hf.margins.iter().all(|m| m.is_finite() && *m >= 0.0)) {
        return Err(EditError::Invalid("invalid font size or margins".into()));
    }
    // Page numbers count up from start_number, so the last selected page needs the most room.
    // Refuse the whole run before drawing anything: headers applied to some pages and then a
    // failed page number would leave the document half changed.
    let last_number = u64::from(hf.start_number)
        .checked_add(pages.len().saturating_sub(1) as u64)
        .ok_or_else(|| EditError::Invalid("the header page numbers are too large".into()))?;
    if last_number > u64::from(u32::MAX) {
        return Err(EditError::Invalid(format!(
            "the header start number {} leaves no room for {} pages (page numbers stop at {})",
            hf.start_number,
            pages.len(),
            u32::MAX
        )));
    }
    // The same goes for Bates numbers: the last selected page must still fit in a u64.
    let bates0 = hf.text.iter().find_map(|t| bates_start(t)).unwrap_or(1);
    if bates0.checked_add(pages.len().saturating_sub(1) as u64).is_none() {
        return Err(EditError::Invalid(format!(
            "the Bates start number {bates0} leaves no room for {} pages (Bates numbers stop at {})",
            pages.len(),
            u64::MAX
        )));
    }
    // Shrinking needs room on every page: refuse before anything is drawn or replaced.
    let shrinks = pages
        .iter()
        .map(|&i| hf.shrink.then(|| shrink_matrix(hf, all[i].display_size(doc), all[i].view_matrix(doc))).transpose())
        .collect::<Result<Vec<_>, _>>()?;
    for &i in pages {
        check_on_top(doc, &all[i])?;
    }
    if replace {
        remove_marks(doc, pages, MarkKind::HeaderFooter)?;
    }
    let all = page_list(doc);
    let count = all.len();
    for (k, &i) in pages.iter().enumerate() {
        let page = &all[i];
        let (w, h) = page.display_size(doc);
        let number =
            u32::try_from(u64::from(hf.start_number) + k as u64).map_err(|_| EditError::Invalid("the header page numbers are too large".into()))?;
        let bates = bates0.checked_add(k as u64).ok_or_else(|| EditError::Invalid("the Bates numbers are too large".into()))?;
        let size = hf.font_size;
        let [top, bottom, left, right] = hf.margins;
        // The page resource the face is added under: the usual name, unless the page already
        // draws another font by it, which must not change under the header.
        let preferred = match hf.font {
            added::Family::Helvetica => "PCHelv",
            added::Family::Times => "PCTimes",
            added::Family::Courier => "PCCour",
        };
        let base = hf.font.base_font(false, false);
        let resource = free_font_name(doc, &page_fonts(doc, page), preferred, base);
        let mut body: Vec<u8> = format!("BT\n/{resource} {} Tf\n{}\n", n(size), rgb(hf.color)).into_bytes();
        let mut underlines = String::new();
        for (slot, template) in hf.text.iter().enumerate() {
            if template.trim().is_empty() {
                continue;
            }
            let lines: Vec<String> = expand(template, number, count, bates, cx).lines().map(str::to_owned).collect();
            let header = slot < 3;
            let line_h = size * 1.2;
            for (li, line) in lines.iter().enumerate() {
                // Centred and right-aligned text is placed by its width in the chosen face.
                let tw = hf.font.width(line, size, false, false);
                let x = match slot % 3 {
                    0 => left,
                    1 => (w - tw) / 2.0,
                    _ => w - right - tw,
                };
                // Headers hang below the top margin; footers sit on the bottom margin.
                let y = if header { h - top - size * 0.8 - li as f64 * line_h } else { bottom + (lines.len() - 1 - li) as f64 * line_h };
                body.extend(text_op(x, y, line));
                if hf.underline {
                    underlines.push_str(&format!("{} {} {} {} re f\n", n(x), n(y - size * 0.15), n(tw), n((size * 0.06).max(0.4))));
                }
            }
        }
        body.extend_from_slice(b"ET\n");
        if !underlines.is_empty() {
            body.extend(format!("{}\n{underlines}", rgb(hf.color)).bytes());
        }
        let mut content = begin(MarkKind::HeaderFooter, "Header", page.view_matrix(doc)).into_bytes();
        content.extend(body);
        content.extend_from_slice(END.as_bytes());
        // A name already there is that very font (see `free_font_name`): left as it is.
        if !page_fonts(doc, page).contains(resource.as_bytes()) {
            add_page_resources(doc, page, None, &[(resource.into_bytes(), base)])?;
        }
        let page = &page_list(doc)[i];
        place(doc, page, MarkKind::HeaderFooter, content, false)?;
        if let Some(Some(m)) = shrinks.get(k) {
            let cm = format!("{} cm\n", m.map(matrix_number).join(" "));
            let page = &page_list(doc)[i];
            place_inside_wrapper(doc, page, MarkKind::HeaderFooter.tag(), cm.into_bytes())?;
        }
    }
    Ok(())
}

/// Add a text watermark to `pages`.
pub fn add_watermark(doc: &mut Document, pages: &[usize], wm: &Watermark, replace: bool) -> Result<(), EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    let text = wm.text.trim();
    if text.is_empty() && wm.source.is_none() {
        return Err(EditError::Invalid("type the watermark text first".into()));
    }
    if let Some(c) = wm.first_undrawable() {
        return Err(undrawable(c));
    }
    if !(wm.opacity.is_finite()
        && wm.rotation.is_finite()
        && wm.font_size.is_finite()
        && wm.font_size >= 0.0
        && wm.offset.iter().all(|o| o.is_finite()))
    {
        return Err(EditError::Invalid("invalid watermark settings".into()));
    }
    if !wm.behind {
        for &i in pages {
            check_on_top(doc, &all[i])?;
        }
    }
    if replace {
        remove_marks(doc, pages, MarkKind::Watermark)?;
    }
    let opacity = wm.opacity.clamp(0.0, 1.0);
    let lines: Vec<&str> = text.lines().collect();
    for &i in pages {
        let page = page_list(doc)[i].clone();
        let (w, h) = page.display_size(doc);
        if let Some(src) = &wm.source {
            let mut content = begin(MarkKind::Watermark, "Watermark", page.view_matrix(doc));
            content.push_str(&format!("/PCGS{} gs\n", (opacity * 100.0).round() as i64));
            content.push_str(&picture(src, (w, h), wm.scale, wm.rotation, wm.offset));
            content.push_str(END);
            add_resources(doc, &page, Some(opacity), None)?;
            add_picture_resource(doc, &page_list(doc)[i].clone(), src)?;
            let page = &page_list(doc)[i];
            place(doc, page, MarkKind::Watermark, content.into_bytes(), wm.behind)?;
            continue;
        }
        let widest = lines.iter().map(|l| helvetica_width(l, 1.0)).fold(0.0, f64::max).max(0.01);
        let size = if wm.font_size > 0.0 { wm.font_size } else { ((w * w + h * h).sqrt() * 0.5 / widest).clamp(6.0, 300.0) };
        let (s, c) = wm.rotation.to_radians().sin_cos();
        let (cx, cy) = (w / 2.0 + wm.offset[0], h / 2.0 + wm.offset[1]);
        let mut content = begin(MarkKind::Watermark, "Watermark", page.view_matrix(doc)).into_bytes();
        content.extend(
            format!(
                "/PCGS{} gs\n{} {} {} {} {} {} cm\nBT\n/PCHelv {} Tf\n{}\n",
                (opacity * 100.0).round() as i64,
                n(c),
                n(s),
                n(-s),
                n(c),
                n(cx),
                n(cy),
                n(size),
                rgb(wm.color)
            )
            .bytes(),
        );
        let line_h = size * 1.15;
        let total = line_h * (lines.len() as f64 - 1.0);
        for (li, line) in lines.iter().enumerate() {
            let tw = helvetica_width(line, size);
            content.extend(text_op(-tw / 2.0, total / 2.0 - li as f64 * line_h - size * 0.35, line));
        }
        content.extend_from_slice(b"ET\n");
        content.extend_from_slice(END.as_bytes());
        add_resources(doc, &page, Some(opacity), None)?;
        let page = &page_list(doc)[i];
        place(doc, page, MarkKind::Watermark, content, wm.behind)?;
    }
    Ok(())
}

/// Add a solid-colour background behind the content of `pages`.
pub fn add_background(doc: &mut Document, pages: &[usize], bg: &Background, replace: bool) -> Result<(), EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    if replace {
        remove_marks(doc, pages, MarkKind::Background)?;
    }
    let opacity = if bg.opacity.is_finite() { bg.opacity.clamp(0.0, 1.0) } else { 1.0 };
    for &i in pages {
        let page = page_list(doc)[i].clone();
        let (w, h) = page.display_size(doc);
        let mut content = begin(MarkKind::Background, "Background", page.view_matrix(doc));
        match &bg.source {
            Some(src) => {
                content.push_str(&format!("/PCGS{} gs\n", (opacity * 100.0).round() as i64));
                content.push_str(&picture(src, (w, h), bg.scale, 0.0, [0.0; 2]));
            }
            None => content.push_str(&format!("/PCGS{} gs\n{}\n0 0 {} {} re f\n", (opacity * 100.0).round() as i64, rgb(bg.color), n(w), n(h))),
        }
        content.push_str(END);
        add_resources(doc, &page, Some(opacity), None)?;
        if let Some(src) = &bg.source {
            add_picture_resource(doc, &page_list(doc)[i].clone(), src)?;
        }
        let page = &page_list(doc)[i];
        place(doc, page, MarkKind::Background, content.into_bytes(), true)?;
    }
    Ok(())
}

/// Remove marks of `kind` from `pages`; returns how many were removed. Wrappers left with
/// nothing after them go too, restoring the original content list; a wrapper that content
/// after it still relies on (another tool's appended stream) stays.
pub fn remove_marks(doc: &mut Document, pages: &[usize], kind: MarkKind) -> Result<usize, EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    let mut removed = 0;
    for &i in pages {
        let page = &all[i];
        let list = contents(doc, page)?;
        let before = list.len();
        let mut kept: Vec<Object> = list.into_iter().filter(|o| stream_tag(doc, o).as_deref() != Some(kind.tag())).collect();
        removed += before - kept.len();
        // Strip only wrapper pairs whose close ends the list (after stripping the pairs after
        // it): nothing comes after them to need the state they restore. Pairs come from the
        // operators; for content that can't be read that precisely, from the wrapper streams
        // alone, so updating a mark there doesn't nest another wrapper each time.
        if before != kept.len() {
            let wrappers = match isolation(doc, &kept) {
                Isolation::Clean { wrappers, .. } => wrappers,
                Isolation::Unknown { .. } | Isolation::TooDeep => {
                    let mut opens = Vec::new();
                    let mut pairs = Vec::new();
                    for (i, o) in kept.iter().enumerate() {
                        match wrapper_role(doc, o) {
                            Some(true) => opens.push(i),
                            Some(false) => pairs.extend(opens.pop().map(|open| (open, i))),
                            None => {}
                        }
                    }
                    pairs
                }
            };
            let pairs: std::collections::HashMap<usize, usize> = wrappers.into_iter().map(|(open, close)| (close, open)).collect();
            let mut discarded = std::collections::HashSet::new();
            let mut end = kept.len();
            while let Some(close) = end.checked_sub(1) {
                if discarded.contains(&close) {
                    end = close;
                } else if let Some(&open) = pairs.get(&close) {
                    discarded.insert(open);
                    discarded.insert(close);
                    end = close;
                } else {
                    break;
                }
            }
            kept = kept.into_iter().enumerate().filter(|(i, _)| !discarded.contains(i)).map(|(_, o)| o).collect();
        }
        if before != kept.len() {
            doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(kept)))?;
        }
    }
    Ok(removed)
}

/// Which kinds of marks the document has (for enabling Update/Remove).
pub fn marks_present(doc: &Document) -> Vec<MarkKind> {
    added::scan_markers(doc).marks
}
