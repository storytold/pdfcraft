use super::{
    AddedContent, Arc, Edit, EditError, FieldValue, ImageEdit, NewAnnotation, Shape, a11y, copy_image, decode_data_file, embed_image, fit_picture,
    image_content_type, is_xfa_data_file, js, mark_source, measure, open_source, strip_xfa, unrestricted, xfa, xfa_after_data_change,
};
use crate::SourceProblem;
use crate::sources::{comment_source, import_pdf_comments};

/// Whether the opening password allows an edit (§7.6.4.2, Table 22).
pub(crate) fn check_permission(edit: &Edit, p: &pdfcraft_cos::Permissions) -> Result<(), EditError> {
    match edit {
        Edit::RotatePages { .. }
        | Edit::DeletePages { .. }
        | Edit::MovePages { .. }
        | Edit::ReversePages { .. }
        | Edit::InsertBlankPage { .. }
        | Edit::DuplicatePages { .. }
        | Edit::ReplacePages { .. }
        | Edit::SetPageBox { .. }
        | Edit::SetTransition { .. }
        | Edit::InsertPagesFrom { .. }
        // "Assemble the document: insert, rotate or delete pages and create bookmarks" (Table 22).
        | Edit::AddBookmark { .. }
        | Edit::RenameBookmark { .. }
        | Edit::DeleteBookmark { .. }
        | Edit::MoveBookmark { .. }
        | Edit::SetBookmarkPage { .. }
        | Edit::SetBookmarkColor { .. }
        | Edit::SetBookmarkStyle { .. }
        | Edit::BookmarksFromStructure
        | Edit::NumberPages { .. } => {
            if p.assemble() {
                Ok(())
            } else {
                Err(EditError::NotPermitted("page changes"))
            }
        }
        // Creates bookmarks (assembly), and their titles are the document's text, copied out of it
        // (Table 22, bit 5), as an export of it would be.
        Edit::BookmarksFromTextStyles { .. } => {
            if !p.assemble() {
                Err(EditError::NotPermitted("page changes"))
            } else if !p.copy() {
                Err(EditError::NotPermitted("copying text and images"))
            } else {
                Ok(())
            }
        }
        Edit::AddMeasurement(_)
        | Edit::AddAnnotation(_)
        | Edit::AddCustomStamp { .. }
        | Edit::DeleteAnnotation { .. }
        | Edit::SetAnnotationContents { .. }
        | Edit::ReplyToAnnotation { .. }
        | Edit::SetAnnotationStatus { .. }
        | Edit::MarkAnnotation { .. }
        | Edit::ReplaceText { .. }
        | Edit::EraseInk { .. }
        | Edit::LockAnnotation { .. }
        | Edit::MoveAnnotation { .. }
        | Edit::ReshapeAnnotation { .. }
        | Edit::ResizeAnnotation { .. }
        | Edit::StyleAnnotation { .. }
        | Edit::FillAnnotation { .. }
        | Edit::FontSizeAnnotation { .. }
        | Edit::SetAnnotationInfo { .. }
        | Edit::SetMeasurementScale { .. } => {
            if p.annotate() {
                Ok(())
            } else {
                Err(EditError::NotPermitted("comments"))
            }
        }
        Edit::SetFieldValue { .. } | Edit::ResetForm { .. } | Edit::SetFieldImage { .. } | Edit::ApplyScriptChanges { .. } | Edit::XfaEvent { .. } => {
            if p.fill_forms() {
                Ok(())
            } else {
                Err(EditError::NotPermitted("filling in form fields"))
            }
        }
        Edit::Protect(_) | Edit::RemoveProtection => {
            if unrestricted(p) {
                Ok(())
            } else {
                Err(EditError::NotPermitted("changing security"))
            }
        }
        // Converting to PDF/A takes the encryption off (PDF/A forbids it), so on an encrypted
        // document it is a change of security (#3921).
        Edit::ConvertPdfA { .. } => {
            if unrestricted(p) {
                Ok(())
            } else {
                Err(EditError::NotPermitted("removing the encryption, which PDF/A forbids"))
            }
        }
        Edit::SetInfo { .. }
        | Edit::AddHeaderFooter { .. }
        | Edit::AddWatermark { .. }
        | Edit::AddBackground { .. }
        | Edit::RemoveMarks { .. }
        | Edit::AddField { .. }
        | Edit::ApplyRedactions { .. }
        | Edit::AddText { .. }
        | Edit::AddImage { .. }
        | Edit::UpdateContent { .. }
        | Edit::MoveObjects { .. }
        | Edit::DeleteContent { .. }
        | Edit::ReplaceImage { .. }
        | Edit::PasteContent { .. }
        | Edit::ClearRedactions
        | Edit::RemoveHidden { .. }
        | Edit::ImportData { .. }
        | Edit::AddLink { .. }
        | Edit::SetLink { .. }
        | Edit::DeleteLink { .. }
        | Edit::RemoveLinks { .. }
        | Edit::AddLinks { .. }
        | Edit::Sanitize
        | Edit::XfaToStatic
        | Edit::SetFieldProps { .. }
        | Edit::DeleteField { .. }
        | Edit::DeleteWidget { .. }
        | Edit::DuplicateField { .. }
        | Edit::SetTabOrder { .. }
        | Edit::MoveInTabOrder { .. }
        | Edit::SetCalculationOrder { .. }
        | Edit::SetInitialView(_)
        | Edit::AddAttachment { .. }
        | Edit::DeleteAttachment { .. }
        | Edit::AutoTagText
        | Edit::SetAltText { .. }
        | Edit::AutoTagFormFields
        | Edit::SetTagProperty { .. }
        | Edit::MoveTag { .. }
        | Edit::MarkDecorative { .. }
        | Edit::MarkTagArtifact { .. }
        | Edit::AddOcrText { .. }
        | Edit::SetDocumentScript { .. }
        | Edit::SetFieldScript { .. }
        | Edit::RemoveStandardId { .. }
        | Edit::EditTextLine { .. }
        | Edit::EditTextBlock { .. }
        | Edit::RegisterTextFont { .. }
        | Edit::EditPageImage { .. }
        | Edit::Flatten { .. }
        | Edit::FlattenFillSign => {
            if p.modify() {
                Ok(())
            } else {
                Err(EditError::NotPermitted("changes to the document"))
            }
        }
        Edit::Batch { edits, .. } => edits.iter().try_for_each(|e| check_permission(e, p)),
    }
}

/// Dates and unique ids stamped onto what an edit creates.
pub(crate) struct EditCtx {
    /// Field scripts run (`None`: JavaScript is off) and what they produced.
    pub(crate) js: Option<js::JsRunner>,
    /// A laid-out XFA form's template (`None`: not an XFA form, or JavaScript is off) and what
    /// its scripts produced.
    pub(crate) xfa: Option<Arc<pdfcraft_xfa::model::Template>>,
    pub(crate) xfa_out: js::JsOutput,
    /// The datasets stream this edit wrote (later writes in the edit replace it in place).
    pub(crate) xfa_datasets: Option<pdfcraft_cos::ObjRef>,
    /// A form script ran too long and was abandoned: the document's scripts go off.
    pub(crate) xfa_ran_away: bool,
    /// What the XFA scripts' time limits are multiplied by ([`Session::with_xfa_time_scale`]).
    pub(crate) xfa_time_scale: u32,
    /// The edit took the XFA out of the form: what went with it, for the document's warnings.
    pub(crate) xfa_removed: Vec<String>,
    /// Pages in the document, for `xfa.layout.pageCount()`.
    pub(crate) pages: usize,
    pub(crate) date: Option<String>,
    /// Today in local time, for date tokens.
    pub(crate) today: (i64, u32, u32),
    pub(crate) seed: u64,
    pub(crate) count: u64,
    /// Fields the edit renamed to keep imported names separate (old → new), for the repair
    /// log and the undo label.
    pub(crate) renamed_fields: Vec<(String, String)>,
    /// The overlay streams the edit's applied redactions added, which alone the renderer's check
    /// leaves out.
    pub(crate) redaction_overlays: Vec<pdfcraft_cos::ObjRef>,
}

impl EditCtx {
    pub(crate) fn new(now: Option<i64>, salt: u64) -> Self {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ salt;
        if let Some(t) = now {
            seed ^= (t as u64).rotate_left(17);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if now.is_some() {
            // Real clock: mix in sub-second time so ids from two sessions don't collide.
            seed ^= std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(0) << 32;
        }
        Self {
            js: None,
            xfa: None,
            xfa_out: Default::default(),
            xfa_datasets: None,
            xfa_ran_away: false,
            xfa_time_scale: 1,
            xfa_removed: Vec::new(),
            pages: 0,
            date: now.map(pdfcraft_cos::pdf_date),
            today: (1970, 1, 1),
            seed,
            count: 0,
            renamed_fields: Vec::new(),
            redaction_overlays: Vec::new(),
        }
    }

    /// 32 bytes of entropy for new encryption keys and salts. `RandomState` is seeded by the
    /// operating system, so no extra dependency is needed.
    fn entropy(&mut self) -> [u8; 32] {
        use std::hash::{BuildHasher, Hasher};
        let mut out = [0u8; 32];
        for (i, chunk) in out.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(self.seed ^ i as u64);
            h.write_u64(self.count);
            self.count += 1;
            chunk.copy_from_slice(&h.finish().to_le_bytes());
        }
        out
    }

    /// A fresh `/NM`: a random-looking UUID (version 4 layout) from a splitmix64 stream.
    fn meta(&mut self) -> pdfcraft_annot::Meta {
        let mut next = || {
            self.count += 1;
            let mut z = self.seed.wrapping_add(self.count.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        let (a, b) = (next(), next());
        let id = format!(
            "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
            a >> 32,
            (a >> 16) & 0xffff,
            a & 0xfff,
            0x8000 | (b >> 48) & 0x3fff,
            b & 0xffff_ffff_ffff
        );
        pdfcraft_annot::Meta { date: self.date.clone(), id }
    }
}

/// Perform an edit on a working copy (the caller discards it on error).
pub(crate) fn run_edit(doc: &mut pdfcraft_cos::Document, edit: &Edit, cx: &mut EditCtx) -> Result<(), EditError> {
    match edit {
        Edit::RotatePages { pages, degrees } => pdfcraft_organize::rotate_pages(doc, pages, *degrees)?,
        Edit::DeletePages { pages } => pdfcraft_organize::delete_pages(doc, pages)?,
        Edit::MovePages { pages, to } => pdfcraft_organize::move_pages(doc, pages, *to)?,
        Edit::ReversePages { pages } => pdfcraft_organize::reverse_pages(doc, pages)?,
        Edit::InsertBlankPage { at, width, height } => {
            pdfcraft_organize::insert_blank_page(doc, *at, *width, *height)?;
        }
        Edit::SetInfo { key, value } => pdfcraft_organize::set_info(doc, key, value)?,
        Edit::DuplicatePages { pages, at } => pdfcraft_organize::duplicate_pages(doc, pages, *at)?,
        Edit::ReplacePages { pages, name, bytes, src_pages } => {
            let src = open_source(name, bytes)?;
            pdfcraft_organize::replace_pages(doc, pages, &src, src_pages)?;
        }
        Edit::SetPageBox { pages, which, spec } => pdfcraft_organize::set_page_box(doc, pages, *which, *spec)?,
        Edit::SetTransition { pages, spec } => pdfcraft_organize::set_transition(doc, pages, spec)?,
        Edit::InsertPagesFrom { name, bytes, pages, at, field_conflicts } => {
            let src = open_source(name, bytes)?;
            let pages = match pages {
                Some(p) => p.clone(),
                None => (0..pdfcraft_organize::page_count(&src)?).collect(),
            };
            // The file's bookmarks come along under one named after it, as in Combine Files.
            let title = std::path::Path::new(name).file_stem().map_or_else(|| name.clone(), |s| s.to_string_lossy().into_owned());
            let outcome = pdfcraft_organize::insert_file_pages(doc, &src, &title, &pages, *at, *field_conflicts)?;
            cx.renamed_fields = outcome.renamed_fields;
        }
        Edit::AddBookmark { parent, index, title, page, left, top } => {
            pdfcraft_organize::add_bookmark_at(doc, parent, *index, title, *page, *left, *top)?;
        }
        Edit::RenameBookmark { path, title } => pdfcraft_organize::rename_bookmark(doc, path, title)?,
        Edit::DeleteBookmark { path } => pdfcraft_organize::delete_bookmark(doc, path)?,
        Edit::MoveBookmark { from, to_parent, index } => {
            pdfcraft_organize::move_bookmark(doc, from, to_parent, *index)?;
        }
        Edit::SetBookmarkPage { path, page } => pdfcraft_organize::set_bookmark_page(doc, path, *page)?,
        Edit::SetBookmarkColor { path, color } => pdfcraft_organize::set_bookmark_color(doc, path, *color)?,
        Edit::SetBookmarkStyle { path, bold, italic } => pdfcraft_organize::set_bookmark_style(doc, path, *bold, *italic)?,
        Edit::BookmarksFromStructure => {
            let entries: Vec<_> = pdfcraft_a11y::headings(doc)
                .into_iter()
                .map(|h| pdfcraft_organize::OutlineEntry { level: h.level, title: h.title, page: h.page, element: h.obj, top: None })
                .collect();
            pdfcraft_organize::add_bookmark_tree(doc, "Untitled", &entries)?;
        }
        Edit::BookmarksFromTextStyles { levels } => {
            if levels.is_empty() {
                return Err(EditError::Invalid("choose a text style for at least one bookmark level".into()));
            }
            // Each line goes to the first level whose style it is in: a style given again, or one
            // within the size tolerance of an earlier level's, would leave its level empty.
            for (i, a) in levels.iter().enumerate() {
                if let Some(j) = levels.iter().skip(i.saturating_add(1)).position(|b| a.matches(b)) {
                    let (first, second) = (i.saturating_add(1), i.saturating_add(j).saturating_add(2));
                    return Err(EditError::Invalid(format!(
                        "bookmark levels {first} and {second} have the same text style: choose a different one for each level"
                    )));
                }
            }
            let entries = crate::text_style_bookmarks::headings(doc, levels);
            if entries.is_empty() {
                return Err(EditError::Invalid("no text is set in the chosen styles".into()));
            }
            pdfcraft_organize::add_bookmark_tree(doc, "Untitled", &entries)?;
        }
        Edit::NumberPages { from, to, style, prefix, first } => pdfcraft_organize::number_pages(doc, *from, *to, *style, prefix, *first)?,
        Edit::AddMeasurement(m) => {
            measure::add(doc, m, &cx.meta())?;
        }
        Edit::SetMeasurementScale { page, bbox, name, scale } => measure::set_scale(doc, *page, *bbox, name, scale)?,
        Edit::AddAnnotation(a) => {
            pdfcraft_annot::add_annotation(doc, a, &cx.meta())?;
        }
        Edit::AddCustomStamp { page, rect, name, file, author } => {
            let src = mark_source(doc, file)?;
            let (sw, sh) = (src.size.0.max(1.0), src.size.1.max(1.0));
            let natural = rect[2] == rect[0] && rect[3] == rect[1];
            let rotation = pdfcraft_organize::page_rotation(doc, *page)?;
            // The picture as displayed: a quarter turn swaps its sides in user space.
            let (dw, dh) = if matches!(rotation, 90 | 270) { (sh, sw) } else { (sw, sh) };
            let rect = if natural {
                let k = (200.0 / sw.max(sh)).min(1.0);
                let (w, h) = (dw * k, dh * k);
                [rect[0] - w / 2.0, rect[1] - h / 2.0, rect[0] + w / 2.0, rect[1] + h / 2.0]
            } else {
                fit_picture(*rect, dw, dh)
            };
            let shape = Shape::CustomStamp { rect, name: name.clone(), picture: src.xobject, image: src.image, size: (sw, sh) };
            let style = pdfcraft_annot::Style::default_for(&shape);
            let new = NewAnnotation { page: *page, shape, style, contents: name.clone(), author: author.clone() };
            let index = pdfcraft_annot::add_annotation(doc, &new, &cx.meta())?;
            pdfcraft_annot::orient_image_stamp(doc, *page, index, rotation)?;
        }
        Edit::DeleteAnnotation { page, index } => pdfcraft_annot::delete_annotation(doc, *page, *index)?,
        Edit::SetAnnotationContents { page, index, text } => pdfcraft_annot::set_contents(doc, *page, *index, text, &cx.meta())?,
        Edit::ReplyToAnnotation { page, index, text, author } => {
            pdfcraft_annot::add_reply(doc, *page, *index, text, author, &cx.meta())?;
        }
        Edit::SetAnnotationStatus { page, index, state, author } => {
            pdfcraft_annot::set_review_state(doc, *page, *index, *state, author, &cx.meta())?;
        }
        Edit::EraseInk { page, index, path, radius } => {
            pdfcraft_annot::erase_ink(doc, *page, *index, path, *radius, &cx.meta())?;
        }
        Edit::AddAttachment { name, bytes, description, mime } => {
            pdfcraft_organize::add_attachment(doc, name, bytes, description.as_deref(), mime.as_deref(), cx.date.as_deref())?;
        }
        Edit::DeleteAttachment { key } => pdfcraft_organize::delete_attachment(doc, key)?,
        Edit::ReplaceText { page, quads, text, author, strike, caret } => {
            // One `/NM` each: the caret replies to the strikeout, and XFDF names that reply by
            // the strikeout's `/NM` (#2666).
            let (strike_meta, caret_meta) = (cx.meta(), cx.meta());
            pdfcraft_annot::add_text_replacement(
                doc,
                pdfcraft_annot::TextReplacement {
                    page: *page,
                    quads,
                    replacement: text,
                    author,
                    strike,
                    caret,
                    strike_meta: &strike_meta,
                    caret_meta: &caret_meta,
                },
            )?;
        }
        Edit::MarkAnnotation { page, index, marked, author } => {
            pdfcraft_annot::set_marked(doc, *page, *index, *marked, author, &cx.meta())?;
        }
        Edit::LockAnnotation { page, index, locked } => pdfcraft_annot::set_locked(doc, *page, *index, *locked)?,
        Edit::MoveAnnotation { page, index, dx, dy } => pdfcraft_annot::move_annotation(doc, *page, *index, *dx, *dy, &cx.meta())?,
        Edit::ReshapeAnnotation { page, index, points } => pdfcraft_annot::set_points(doc, *page, *index, points, &cx.meta())?,
        Edit::ResizeAnnotation { page, index, rect } => pdfcraft_annot::set_rect(doc, *page, *index, *rect, &cx.meta())?,
        Edit::StyleAnnotation { page, index, color, opacity, width, endings } => {
            pdfcraft_annot::set_style(
                doc,
                *page,
                *index,
                pdfcraft_annot::StyleChange { color: *color, opacity: *opacity, width: *width, endings: endings.clone() },
                &cx.meta(),
            )?;
        }
        Edit::FillAnnotation { page, index, fill } => pdfcraft_annot::set_fill(doc, *page, *index, *fill, &cx.meta())?,
        Edit::FontSizeAnnotation { page, index, size } => pdfcraft_annot::set_font_size(doc, *page, *index, *size, &cx.meta())?,
        Edit::SetAnnotationInfo { page, index, author, subject, icon } => {
            pdfcraft_annot::set_info(doc, *page, *index, author.as_deref(), subject.as_deref(), *icon, &cx.meta())?;
        }
        Edit::SetFieldValue { name, value } => {
            let mut set = |doc: &mut pdfcraft_cos::Document, value: &FieldValue| match cx.js.as_mut() {
                Some(js) => pdfcraft_forms::set_value_with(doc, name, value, js),
                None => pdfcraft_forms::set_value(doc, name, value),
            };
            match set(doc, value) {
                Ok(()) => {}
                // A date the field's format doesn't read may be typed by another of an XFA date
                // field's patterns: it is kept in the first. Only a format refusal is tried
                // again: it comes before any of the field's scripts run, so they run once.
                Err(refused @ pdfcraft_forms::FormError::Format(_)) => match xfa::typed_date(doc, cx.xfa.as_deref(), name, value) {
                    Some(typed) => set(doc, &typed)?,
                    None => return Err(refused.into()),
                },
                Err(refused) => return Err(refused.into()),
            }
        }
        Edit::ApplyScriptChanges { changes } => match cx.js.as_mut() {
            Some(js) => pdfcraft_forms::apply_script_changes(doc, changes, js)?,
            None => pdfcraft_forms::apply_script_changes(doc, changes, &mut pdfcraft_forms::NoScripts)?,
        },
        Edit::XfaEvent { som, activity } => {
            let tpl = cx.xfa.clone().ok_or_else(|| EditError::Invalid("this is not a laid-out XFA form, or JavaScript is off".into()))?;
            let mut run =
                xfa::XfaRun { page_count: cx.pages, out: &mut cx.xfa_out, datasets: cx.xfa_datasets, ran_away: false, time_scale: cx.xfa_time_scale };
            let ran = xfa::on_event(doc, &tpl, som, activity, &mut run);
            cx.xfa_datasets = run.datasets;
            cx.xfa_ran_away |= run.ran_away;
            ran.map_err(EditError::Invalid)?;
        }
        Edit::SetFieldImage { name, image } => {
            let (img, _) = embed_image(doc, name, image)?;
            let px = match &*doc.get(img) {
                pdfcraft_cos::Object::Stream(s) => (s.dict.int(b"Width").unwrap_or(1) as u32, s.dict.int(b"Height").unwrap_or(1) as u32),
                _ => (1, 1),
            };
            // An XFA image field keeps the picture in the data too (as Designer saves it), and a
            // laid-out form shows it from there: so only pictures the layout can show, and only
            // for image fields (a laid-out one carries its SOM path; a static form's field name
            // is its path).
            let is_xfa = pdfcraft_xfa::read_packets(doc).ok().flatten().is_some();
            let image_field = is_xfa
                && pdfcraft_forms::fields(doc).iter().any(|f| f.name == *name && f.button == Some(pdfcraft_forms::af::ButtonAction::ImportIcon));
            if image_field {
                let Some(content_type) = image_content_type(image) else {
                    return Err(EditError::Invalid(format!("{name}: an XFA form keeps a PNG, JPEG, GIF, BMP or TIFF picture; choose one of those")));
                };
                pdfcraft_forms::set_button_icon(doc, name, img, px)?;
                let som = pdfcraft_xfa::fields_by_som(doc).into_iter().find(|(_, n)| n == name).map(|(s, _)| s).unwrap_or_else(|| name.clone());
                let path = match cx.xfa.as_deref() {
                    Some(tpl) => pdfcraft_xfa::data_path(tpl, &som),
                    None => pdfcraft_xfa::som_to_path(&som),
                };
                let r =
                    pdfcraft_xfa::write_data_image(doc, &path, content_type, image, cx.xfa_datasets).map_err(|e| EditError::Write(e.to_string()))?;
                if !r.written && r.warnings.is_empty() {
                    return Err(EditError::Write(format!("{name}: the picture could not be written into the form's data")));
                }
                cx.xfa_datasets = r.stream.or(cx.xfa_datasets);
                cx.xfa_out.errors.extend(r.warnings);
                xfa_after_data_change(doc, cx, false)?;
            } else {
                pdfcraft_forms::set_button_icon(doc, name, img, px)?;
            }
        }
        Edit::ResetForm { names } => {
            pdfcraft_forms::reset(doc, names.as_deref())?;
        }
        Edit::AddField { page, rect, kind, name } => {
            pdfcraft_forms::add_field(doc, *page, *rect, kind, name.as_deref())?;
        }
        Edit::SetFieldProps { name, props } => {
            pdfcraft_forms::set_props(doc, name, props)?;
        }
        Edit::DuplicateField { name, pages } => {
            if pdfcraft_forms::duplicate_field(doc, name, pages)? == 0 {
                return Err(EditError::Form(pdfcraft_forms::FormError::Invalid(format!("{name} is already on those pages"))));
            }
        }
        Edit::DeleteField { name } => pdfcraft_forms::delete_field(doc, name)?,
        Edit::DeleteWidget { name, widget } => pdfcraft_forms::delete_widget(doc, name, *widget)?,
        Edit::SetTabOrder { pages, order } => pdfcraft_forms::set_tab_order(doc, pages, *order)?,
        Edit::MoveInTabOrder { name, earlier } => pdfcraft_forms::move_in_tab_order(doc, name, *earlier)?,
        // As after a script edit: the values the new order gives show at once.
        Edit::SetCalculationOrder { names } => {
            pdfcraft_forms::set_calculation_order(doc, names)?;
            match cx.js.as_mut() {
                Some(js) => pdfcraft_forms::recalculate_with(doc, js)?,
                None => pdfcraft_forms::recalculate(doc)?,
            };
        }
        Edit::SetInitialView(v) => pdfcraft_organize::set_initial_view(doc, v)?,
        Edit::AutoTagText => {
            a11y::autotag_text(doc).map_err(|e| EditError::Accessibility(e.to_string()))?;
        }
        Edit::SetAltText { figure, alt } => {
            let r = pdfcraft_cos::ObjRef::new(*figure, doc.generation(*figure));
            a11y::set_alt(doc, r, alt.as_deref()).map_err(|e| EditError::Accessibility(e.to_string()))?;
        }
        Edit::AutoTagFormFields => {
            a11y::tag_form_fields(doc).map_err(EditError::Accessibility)?;
        }
        Edit::SetTagProperty { object, property, value } => {
            a11y::set_tag_property(doc, *object, *property, value.as_deref()).map_err(|e| EditError::Accessibility(e.to_string()))?;
        }
        Edit::MoveTag { object, anchor, placement } => {
            a11y::move_tag(doc, *object, *anchor, *placement).map_err(|e| EditError::Accessibility(e.to_string()))?;
        }
        Edit::EditTextLine { page, line, text } => {
            pdfcraft_edit::replace_line(doc, *page, *line, text)?;
        }
        Edit::SetFieldScript { name, event, script } => {
            pdfcraft_forms::set_field_script(doc, name, event, script.as_deref())?;
            match cx.js.as_mut() {
                Some(js) => pdfcraft_forms::recalculate_with(doc, js)?,
                None => pdfcraft_forms::recalculate(doc)?,
            };
        }
        Edit::ConvertPdfA { level } => {
            pdfcraft_preflight::convert(doc, *level).map_err(|e| EditError::Invalid(e.to_string()))?;
        }
        Edit::RemoveStandardId { standard } => pdfcraft_organize::remove_standard_id(doc, *standard)?,
        Edit::SetDocumentScript { name, script } => pdfcraft_forms::set_document_script(doc, name, script.as_deref())?,
        Edit::AddOcrText { page, words } => crate::ocr::add_text(doc, *page, words)?,
        Edit::EditPageImage { page, index, change } => {
            let img = pdfcraft_edit::page_images(doc, *page)?
                .into_iter()
                .nth(*index)
                .ok_or_else(|| EditError::Edit(pdfcraft_edit::EditError::Invalid(format!("page {} has no image {}", page + 1, index + 1))))?;
            let c = match change {
                ImageEdit::Move(to) => pdfcraft_edit::ImageChange::Transform(pdfcraft_edit::rect_to_rect(img.rect, *to)),
                ImageEdit::Rotate(q) => {
                    pdfcraft_edit::ImageChange::Transform(pdfcraft_edit::turn_about_centre(img.visible_rect.unwrap_or(img.rect), *q, false, false))
                }
                ImageEdit::RotateBy(degrees) => {
                    let t = pdfcraft_edit::turn_by_degrees(img.visible_rect.unwrap_or(img.rect), *degrees)
                        .ok_or_else(|| EditError::Edit(pdfcraft_edit::EditError::Invalid("the rotation angle must be a finite number".into())))?;
                    pdfcraft_edit::ImageChange::Transform(t)
                }
                ImageEdit::Flip { horizontal } => pdfcraft_edit::ImageChange::Transform(pdfcraft_edit::turn_about_centre(
                    img.visible_rect.unwrap_or(img.rect),
                    0,
                    *horizontal,
                    !*horizontal,
                )),
                ImageEdit::Replace { name, bytes } => {
                    if img.is_form {
                        return Err(EditError::Edit(pdfcraft_edit::EditError::Invalid(
                            "grouped Form artwork cannot be replaced with a bitmap".into(),
                        )));
                    }
                    pdfcraft_edit::ImageChange::Replace(embed_image(doc, name, bytes)?.0)
                }
                ImageEdit::Crop(keep) => pdfcraft_edit::ImageChange::Crop(*keep),
                ImageEdit::Delete => pdfcraft_edit::ImageChange::Delete,
            };
            pdfcraft_edit::change_image(doc, *page, *index, &c)?;
        }
        Edit::MoveObjects { page, objects, offset } => pdfcraft_edit::move_objects(doc, *page, objects, *offset)?,
        Edit::EditTextBlock { page, block, text, style } => {
            pdfcraft_edit::rewrite_block(doc, *page, *block, Some(text), style)?;
        }
        Edit::RegisterTextFont { page, bytes } => {
            pdfcraft_edit::text::register_text_font(doc, *page, bytes)?;
        }
        Edit::MarkDecorative { figure } => {
            let r = pdfcraft_cos::ObjRef::new(*figure, doc.generation(*figure));
            a11y::mark_decorative(doc, r).map_err(|e| EditError::Accessibility(e.to_string()))?;
        }
        Edit::MarkTagArtifact { object } => {
            a11y::mark_tag_artifact(doc, *object).map_err(|e| EditError::Accessibility(e.to_string()))?;
        }
        Edit::AddHeaderFooter { pages, settings, replace } => {
            let date = cx.today;
            pdfcraft_edit::add_header_footer(doc, pages, settings, *replace, &pdfcraft_edit::Context { date })?;
        }
        Edit::AddWatermark { pages, settings, replace, file } => {
            let mut s = settings.clone();
            if let Some(f) = file {
                s.source = Some(mark_source(doc, f)?);
            }
            pdfcraft_edit::add_watermark(doc, pages, &s, *replace)?
        }
        Edit::AddBackground { pages, settings, replace, file } => {
            let mut s = settings.clone();
            if let Some(f) = file {
                s.source = Some(mark_source(doc, f)?);
            }
            pdfcraft_edit::add_background(doc, pages, &s, *replace)?
        }
        Edit::RemoveMarks { kind } => {
            let n = pdfcraft_model::pages(doc).len();
            if pdfcraft_edit::remove_marks(doc, &(0..n).collect::<Vec<_>>(), *kind)? == 0 {
                return Err(EditError::Edit(pdfcraft_edit::EditError::Invalid("there is nothing to remove".into())));
            }
        }
        Edit::AddText { page, text } => {
            pdfcraft_edit::add_content(doc, *page, &AddedContent::Text(text.clone()))?;
        }
        Edit::AddImage { page, rect, name, bytes } => {
            let (image, natural) = embed_image(doc, name, bytes)?;
            let rect = match rect {
                Some(r) => *r,
                None => {
                    let p = pdfcraft_model::pages(doc).swap_remove(*page);
                    let (pw, ph) = p.display_size(doc);
                    let k = ((pw * 0.8) / natural.0).min((ph * 0.8) / natural.1).min(1.0);
                    let (w, h) = (natural.0 * k, natural.1 * k);
                    [(pw - w) / 2.0, (ph - h) / 2.0, (pw + w) / 2.0, (ph + h) / 2.0]
                }
            };
            pdfcraft_edit::add_content(doc, *page, &AddedContent::Image(pdfcraft_edit::AddedImage::new(rect, image)))?;
        }
        Edit::UpdateContent { page, index, content } => pdfcraft_edit::update_content(doc, *page, *index, content)?,
        Edit::DeleteContent { page, index } => pdfcraft_edit::delete_content(doc, *page, *index)?,
        Edit::ReplaceImage { page, index, name, bytes } => {
            let item = pdfcraft_edit::list_added(doc).into_iter().filter(|a| a.page == *page).nth(*index);
            let Some(AddedContent::Image(old)) = item.map(|a| a.content) else {
                return Err(EditError::Edit(pdfcraft_edit::EditError::Invalid("that item is not an image".into())));
            };
            let (image, _) = embed_image(doc, name, bytes)?;
            pdfcraft_edit::update_content(doc, *page, *index, &AddedContent::Image(pdfcraft_edit::AddedImage { image, ..old }))?;
        }
        Edit::PasteContent { page, content, file } => {
            let content = match content {
                AddedContent::Text(_) => content.clone(),
                AddedContent::Image(img) => {
                    let Some((name, bytes)) = file else {
                        return Err(EditError::Edit(pdfcraft_edit::EditError::Invalid("a pasted image needs its picture".into())));
                    };
                    // The same picture in this document (copied here, pasted here before, or an
                    // identical file): drawn again from the one image object rather than stored twice.
                    let image = if copy_image::is_picture(doc, img.image, bytes) {
                        img.image
                    } else {
                        match copy_image::added_picture(doc, bytes) {
                            Some(image) => image,
                            None => embed_image(doc, name, bytes)?.0,
                        }
                    };
                    AddedContent::Image(pdfcraft_edit::AddedImage { image, ..img.clone() })
                }
            };
            pdfcraft_edit::add_content(doc, *page, &content)?;
        }
        Edit::ApplyRedactions { pages } => {
            cx.redaction_overlays = pdfcraft_redact::apply(doc, pages.as_deref())?.overlays;
        }
        Edit::ClearRedactions => {
            pdfcraft_redact::clear_marks(doc, None)?;
        }
        Edit::AddLink { page, rect, action, style } => {
            pdfcraft_annot::links::add(doc, *page, *rect, action, style)?;
        }
        Edit::SetLink { page, index, rect, action, style } => pdfcraft_annot::links::set(doc, *page, *index, *rect, action.as_ref(), style.as_ref())?,
        Edit::DeleteLink { page, index } => pdfcraft_annot::links::delete(doc, *page, *index)?,
        Edit::RemoveLinks { pages } => {
            if pdfcraft_annot::links::remove_all(doc, pages.as_deref())? == 0 {
                return Err(EditError::Edit(pdfcraft_edit::EditError::Invalid("there are no links to remove".into())));
            }
        }
        Edit::AddLinks { links, style } => {
            if pdfcraft_annot::links::add_many(doc, links, style)? == 0 {
                return Err(EditError::Edit(pdfcraft_edit::EditError::Invalid("no web addresses were found".into())));
            }
        }
        Edit::ImportData { name, bytes } if bytes.starts_with(b"%PDF-") => {
            let src = comment_source(bytes).map_err(|p| EditError::Source(format!("{name}: {p}")))?;
            import_pdf_comments(doc, &src)?;
        }
        Edit::ImportData { name, bytes } => {
            // An XFA form takes XFA data files (XDP, datasets, or a data document) as its data;
            // XFDF, FDF, Acrobat's XML and tables go the usual way.
            let text = (pdfcraft_xfa::read_packets(doc).ok().flatten().is_some()).then(|| decode_data_file(bytes)).flatten();
            match text {
                Some(Ok(text)) if is_xfa_data_file(&text) => {
                    let expect_root = cx.xfa.as_ref().and_then(|t| t.root.common.name.clone());
                    let r = pdfcraft_xfa::import_data(doc, &text, expect_root.as_deref(), cx.xfa_datasets)
                        .map_err(|e| EditError::Write(e.to_string()))?;
                    cx.xfa_datasets = r.stream.or(cx.xfa_datasets);
                    cx.xfa_out.errors.extend(r.warnings);
                    cx.xfa_out.console.push(format!("Imported XFA data: {} node(s)", r.nodes));
                    xfa_after_data_change(doc, cx, true)?;
                }
                Some(Err(why)) if !bytes[..bytes.len().min(1024)].windows(5).any(|w| w == b"%PDF-") => return Err(EditError::Invalid(why)),
                _ => {
                    // A PDF with bytes before its header, within the 1,024 the reader allows, is read
                    // as a PDF when it is not a data file. A file the PDF reader cannot open keeps
                    // the data importer's error.
                    let mut data = doc.clone();
                    match pdfcraft_xfdf::import(&mut data, bytes) {
                        Ok(_) => *doc = data,
                        Err(e) if bytes[..bytes.len().min(1024)].windows(5).any(|w| w == b"%PDF-") => match comment_source(bytes) {
                            Ok(src) => import_pdf_comments(doc, &src)?,
                            Err(SourceProblem::Unreadable(_)) => return Err(e.into()),
                            Err(p) => return Err(EditError::Source(format!("{name}: {p}"))),
                        },
                        Err(e) => return Err(e.into()),
                    }
                }
            }
        }
        Edit::RemoveHidden { which } => {
            pdfcraft_redact::sanitize::remove_hidden(doc, which)?;
        }
        Edit::Sanitize => {
            pdfcraft_redact::sanitize::sanitize(doc)?;
        }
        Edit::Flatten { comments, fields } => {
            // An XFA form's fields baked into the pages: the XFA goes too, or Adobe's viewers
            // would lay the form out again from its template over the baked pages. A dynamic
            // form that was never laid out keeps its XFA: without it only placeholders remain.
            let shape = (*fields).then(|| pdfcraft_xfa::xfa_shape(doc)).flatten();
            let n = pdfcraft_model::pages(doc).len();
            pdfcraft_edit::flatten(doc, &(0..n).collect::<Vec<_>>(), *comments, *fields)?;
            match shape {
                Some(shape) if shape.strippable() => strip_xfa(doc, cx)?,
                Some(_) => cx.xfa_removed.push("The XFA was kept: this dynamic form was not laid out, so its fields were not baked in".into()),
                None => {}
            }
        }
        Edit::XfaToStatic => {
            let Some(shape) = pdfcraft_xfa::xfa_shape(doc) else {
                return Err(EditError::Invalid("this document is not an XFA form".into()));
            };
            if !shape.strippable() {
                return Err(EditError::Invalid("this dynamic XFA form was not laid out, so there is no static form to keep".into()));
            }
            strip_xfa(doc, cx)?;
        }
        Edit::FlattenFillSign => {
            let n = pdfcraft_model::pages(doc).len();
            pdfcraft_edit::flatten_fill_sign(doc, &(0..n).collect::<Vec<_>>())?;
        }
        Edit::Protect(p) => {
            p.validate()?;
            let seed = cx.entropy();
            // Without a permissions password nothing is restricted, so the owner password is a
            // random one nobody needs.
            let random_owner: String = cx.entropy().iter().map(|b| format!("{b:02x}")).collect();
            let params = pdfcraft_cos::NewEncryption {
                algorithm: p.algorithm,
                user_password: p.open_password.as_deref().unwrap_or(""),
                owner_password: p.permissions_password.as_deref().unwrap_or(&random_owner),
                permissions: p.permission_bits(),
                encrypt_metadata: p.encrypt_metadata,
                seed,
            };
            doc.set_encryption(&params).map_err(|e| EditError::Protection(e.to_string()))?;
        }
        Edit::RemoveProtection => {
            if doc.output_handler().is_none() {
                return Err(EditError::Protection("the document isn't protected".into()));
            }
            doc.remove_encryption();
        }
        Edit::Batch { edits, .. } => {
            for e in edits {
                run_edit(doc, e, cx)?;
            }
        }
    }
    Ok(())
}
