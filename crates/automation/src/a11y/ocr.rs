use super::{Args, Automation, Result, ToolError, Value, child, failed, json, write_atomic};

impl Automation {
    /// The settings every OCR tool shares: dpi, language, the engine/strategy choice, the four
    /// preprocessing flags, the skip flags and the output mode — each named exactly as its tool
    /// schema says. An unknown engine, strategy or mode is an honest error, never a silent
    /// fallback: a tool run must not quietly do something other than what it was asked.
    pub(crate) fn ocr_settings(&self, a: &Args) -> Result<pdfcraft_engine::ocr::OcrSettings> {
        use pdfcraft_engine::ocr::{EngineChoice, MergeStrategy, OutputMode};
        let mut settings = pdfcraft_engine::ocr::OcrSettings::default();
        if let Some(d) = a.opt_num("dpi")? {
            settings.dpi = d.clamp(72.0, 600.0) as f32;
        }
        if let Some(l) = a.opt_str("language")? {
            if !pdfcraft_engine::ocr::LANGUAGES.iter().any(|x| x.0 == l) {
                return Err(ToolError::InvalidArgs(format!(
                    "unsupported language {l:?} (supported: {})",
                    pdfcraft_engine::ocr::LANGUAGES.iter().map(|(c, _)| *c).collect::<Vec<_>>().join(", ")
                )));
            }
            settings.language = l.into();
        }
        if let Some(e) = a.opt_str("engine")? {
            settings.engine = match e.trim().to_ascii_lowercase().as_str() {
                "ocrs" => EngineChoice::Ocrs,
                "tesseract" => EngineChoice::Tesseract,
                "auto" | "automatic" | "ensemble" => EngineChoice::Auto,
                other => return Err(ToolError::InvalidArgs(format!("unsupported engine {other:?} (ocrs, tesseract or auto)"))),
            };
        }
        settings.strategy = match a.opt_str("strategy")? {
            Some(s) => match s.trim().to_ascii_lowercase().as_str() {
                "primary" | "primary-only" | "primary_only" => MergeStrategy::PrimaryOnly,
                "confidence" | "confidence-weighted" | "confidence_weighted" => MergeStrategy::ConfidenceWeighted,
                "rover" | "rover-vote" | "rover_vote" | "vote" => MergeStrategy::RoverVote,
                other => return Err(ToolError::InvalidArgs(format!("unsupported strategy {other:?} (primary, confidence or rover)"))),
            },
            // Unset: the engine choice decides (auto brings in the second engine when the
            // first reading is weak; a named engine reads alone).
            None => settings.engine.default_strategy(),
        };
        if let Some(m) = a.opt_str("output_mode")? {
            settings.output_mode = match m.trim().to_ascii_lowercase().as_str() {
                "searchable" | "searchable-image" | "searchable_image" => OutputMode::Searchable,
                "editable" | "editable-text" | "editable_text" => OutputMode::EditableText,
                other => return Err(ToolError::InvalidArgs(format!("unsupported output mode {other:?} (searchable or editable)"))),
            };
        }
        for (key, flag) in [
            ("auto_rotate", &mut settings.auto_rotate),
            ("deskew", &mut settings.deskew),
            ("denoise", &mut settings.denoise),
            ("binarize", &mut settings.binarize),
        ] {
            if let Some(on) = a.opt_bool(key)? {
                *flag = on;
            }
        }
        for (key, flag) in [
            ("skip_text_pages", &mut settings.skip_text_pages),
            ("skip_text_files", &mut settings.skip_text_files),
            ("force_ocr", &mut settings.force_ocr),
        ] {
            if let Some(on) = a.opt_bool(key)? {
                *flag = on;
            }
        }
        Ok(settings)
    }

    /// The optional `region` argument ([x0, y0, x1, y1] in points from the top-left of the
    /// displayed page) as the job's pixel crop, at the resolution that page is really read at.
    /// A region reads exactly one page.
    fn ocr_region(
        &self,
        a: &Args,
        id: pdfcraft_engine::DocId,
        page: usize,
        settings: &pdfcraft_engine::ocr::OcrSettings,
    ) -> Result<Option<[f32; 4]>> {
        const BAD: &str = "region must be [x0, y0, x1, y1] in points";
        let Some(v) = a.get("region") else { return Ok(None) };
        let arr = v.as_array().ok_or_else(|| ToolError::InvalidArgs(BAD.into()))?;
        if arr.len() != 4 {
            return Err(ToolError::InvalidArgs(BAD.into()));
        }
        let mut pts = [0.0f64; 4];
        for (i, x) in arr.iter().enumerate() {
            pts[i] = x.as_f64().filter(|f| f.is_finite()).ok_or_else(|| ToolError::InvalidArgs(BAD.into()))?;
        }
        let scale = f64::from(
            self.session.ocr_job(id, &[page], settings.clone()).ok_or_else(|| failed("the document could not be read"))?.render_scale(page),
        );
        let px: Vec<f32> = pts.iter().map(|p| scale * p).map(|p| if p.is_finite() { p } else { 0.0 }).map(|p| p as f32).collect();
        Ok(Some([px[0], px[1], px[2], px[3]]))
    }

    /// The per-page result entries the OCR tools return: recognised text or why the page was
    /// passed over. The text itself is only echoed when the document allows copying; the
    /// counts and metadata always are.
    fn ocr_pages_json(found: &[pdfcraft_engine::ocr::OcrPage], include_text: bool) -> Vec<Value> {
        found
            .iter()
            .map(|p| match &p.skipped {
                Some(why) => json!({ "page": p.page + 1, "skipped": why }),
                None => {
                    let mut page = json!({ "page": p.page + 1, "words": p.words.len(), "already_on_page": p.already_on_page });
                    // The editable output's substitution warnings (characters no font could
                    // show) are part of the result: reported per page, never dropped.
                    if !p.notes.is_empty() {
                        page["notes"] = json!(p.notes);
                    }
                    if include_text {
                        page["text"] = json!(p.text());
                    }
                    page
                }
            })
            .collect()
    }

    pub(crate) fn ocr_recognize_files(&mut self, a: &Args) -> Result<Value> {
        let settings = self.ocr_settings(a)?;
        let folder = self.resolve(a.str("folder")?, true)?;
        // A path that leaves the read root refuses the whole call before any work starts;
        // a file that is merely missing stays a per-file error below.
        let paths = a.strs("paths")?;
        for p in &paths {
            self.resolve_for(p, crate::file_access::Access::Read, true)?;
        }
        // Two inputs of one file name (from different folders) would write the same output:
        // the second would replace the first while both report success. The whole batch is
        // refused up front — before the folder is created, anything is read or the engines
        // load — naming the file, the way the default filesystem compares names.
        let targets: Vec<std::path::PathBuf> = paths.iter().map(|p| child(&folder, &input_file_name(p))).collect();
        let shared = shared_file_names(&targets);
        if !shared.is_empty() {
            return Err(ToolError::InvalidArgs(format!(
                "several paths share a file name, so their outputs in folder would overwrite each other (rename or run them separately): {}",
                shared.join(", ")
            )));
        }
        std::fs::create_dir_all(&folder).map_err(|e| failed(e.to_string()))?;
        let names: Vec<String> = paths.iter().map(|p| input_file_name(p)).collect();
        // Collision pre-flight: a searchable copy never silently overwrites. Every target is
        // checked before the first page is read; a collision aborts the whole batch with the
        // names in the error.
        let mut collisions = std::collections::BTreeSet::new();
        for name in names {
            if child(&folder, &name).exists() {
                collisions.insert(name);
            }
        }
        if !collisions.is_empty() {
            let mut list: Vec<String> = collisions.iter().take(5).cloned().collect();
            if collisions.len() > list.len() {
                list.push(format!("and {} more", collisions.len() - list.len()));
            }
            return Err(failed(format!(
                "the output folder already holds {} (delete or move them, or choose another folder): {}",
                if collisions.len() == 1 { "a file of that name" } else { "files of those names" },
                list.join(", ")
            )));
        }
        // The engines are built (and named if they cannot be) before the first file is read.
        let recognizers = pdfcraft_engine::ocr::recognizers(&settings).map_err(failed)?;
        let mut out = Vec::new();
        let (mut ok, mut skipped_files, mut words) = (0usize, 0usize, 0usize);
        for p in paths {
            let name = input_file_name(p);
            let result =
                self.resolve(p, false).map_err(|e| e.to_string()).and_then(|src| crate::read_input_bytes(&src).map_err(|e| e.to_string())).and_then(
                    |bytes| {
                        pdfcraft_engine::ocr::recognize_file(&name, std::sync::Arc::new(bytes), None, settings.clone(), &recognizers, |_, _| true)
                    },
                );
            match result {
                Ok(r) => {
                    let skipped_pages: Vec<usize> = r.pages.iter().filter(|p| p.skipped.is_some()).map(|p| p.page + 1).collect();
                    if r.skipped() {
                        // A file passed over by the skip-files option is its own bucket: its
                        // original is left alone and nothing is written.
                        skipped_files += 1;
                        out.push(json!({ "path": p, "skipped": r.file_skipped, "skipped_pages": skipped_pages }));
                        continue;
                    }
                    let target = child(&folder, &name);
                    write_atomic(&target, &r.bytes)?;
                    ok += 1;
                    words += r.words();
                    let mut entry = json!({ "path": p, "output": target.to_string_lossy(), "words": r.words(), "skipped_pages": skipped_pages });
                    // The batch's low-confidence note: at or under the settings' threshold (60
                    // by default), on 1-based sorted pages.
                    let threshold = settings.low_confidence_threshold;
                    if let Some((count, pages)) = pdfcraft_engine::ocr::low_confidence_pages(&r.pages, threshold) {
                        entry["low_confidence"] = json!({ "count": count, "pages": pages, "threshold": threshold });
                    }
                    out.push(entry);
                }
                Err(e) => out.push(json!({ "path": p, "error": e })),
            }
        }
        Ok(json!({ "files": out, "recognized": ok, "words": words, "skipped_files": skipped_files }))
    }

    pub(crate) fn ocr_recognize(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::ocr::OutputMode;
        let mut settings = self.ocr_settings(a)?;
        let pages = if a.opt_ints("pages")?.is_some() { self.pages(a, "pages")? } else { Vec::new() };
        // Recognizing needs only permission to change the document; the words it found are
        // returned only when copying is allowed too.
        let (id, copy) = {
            let doc = self.doc(a)?;
            (doc.id, doc.allows_copying())
        };
        // The optional region reads exactly one page.
        if a.get("region").is_some() {
            if pages.len() != 1 {
                return Err(ToolError::InvalidArgs("region reads one page: name it in pages".into()));
            }
            settings.region = self.ocr_region(a, id, pages[0], &settings)?;
        }
        // The editable output's destination is checked before anything runs: a NEW file, never
        // the open source and never an existing file replaced silently.
        let out = match settings.output_mode {
            OutputMode::Searchable => {
                // `out` belongs to the editable output; a searchable run writes into the open
                // document, so an `out` here would be silently ignored (and never checked
                // against the root).
                if a.get("out").is_some() {
                    return Err(ToolError::InvalidArgs("out is only used with output_mode editable; remove it, or choose editable".into()));
                }
                None
            }
            OutputMode::EditableText => {
                // The editable output writes the recognized text out of the document: a
                // document that forbids copying refuses here, before anything runs.
                self.doc_to_copy(a)?;
                // A write: the file does not exist yet, and write_atomic makes its folder.
                let out = self.resolve(a.str("out")?, true)?;
                if let Some(source) = self.session.get(id).and_then(|d| d.path.clone())
                    && let (Ok(a), Ok(b)) = (std::fs::canonicalize(&out), std::fs::canonicalize(&source))
                    && a == b
                {
                    return Err(failed("out is the source document; the editable output is a new file, so choose another path"));
                }
                if out.exists() {
                    return Err(failed(format!(
                        "out already exists ({}); the editable output is a new file, so choose a path that does not exist yet",
                        out.to_string_lossy()
                    )));
                }
                Some(out)
            }
        };
        // Honest up front: the engines are built (and named if they cannot be) before anything
        // is read or written.
        let recognizers = pdfcraft_engine::ocr::recognizers(&settings).map_err(failed)?;
        match out {
            None => {
                let found = self.session.recognize_text(id, &pages, settings).map_err(failed)?;
                let words = found.iter().map(|p| p.words.len()).sum::<usize>();
                Ok(json!({ "words": words, "pages": Self::ocr_pages_json(&found, copy) }))
            }
            Some(out) => {
                let job = self.session.ocr_job(id, &pages, settings).ok_or_else(|| failed("the document could not be read"))?;
                let found = job.run(&recognizers, |_, _| true);
                let bytes = self.session.editable_ocr_bytes(id, &found).map_err(failed)?;
                write_atomic(&out, &bytes)?;
                Ok(json!({
                    "out": out.to_string_lossy(),
                    "words": found.iter().map(|p| p.words.len()).sum::<usize>(),
                    "pages": Self::ocr_pages_json(&found, copy),
                }))
            }
        }
    }

    /// The words of a recognition, for reviewing without the GUI: text, box (points from the
    /// top-left of the displayed page), confidence, band, and which words are suspects.
    /// Changes nothing; needs the engines (ocr_status).
    pub(crate) fn ocr_words(&mut self, a: &Args) -> Result<Value> {
        let mut settings = self.ocr_settings(a)?;
        let pages = if a.opt_ints("pages")?.is_some() { self.pages(a, "pages")? } else { Vec::new() };
        // The word list is the document's text, word by word: refused when the document
        // forbids copying it out.
        let id = self.doc_to_copy(a)?.id;
        if a.get("region").is_some() {
            if pages.len() != 1 {
                return Err(ToolError::InvalidArgs("region reads one page: name it in pages".into()));
            }
            settings.region = self.ocr_region(a, id, pages[0], &settings)?;
        }
        let retain = a.opt_bool("retain_review")?.unwrap_or(false);
        if retain && !self.ocr_reviews.contains_key(&id) && self.ocr_reviews.len() >= 8 {
            return Err(failed("at most 8 OCR reviews may be retained; discard one first"));
        }
        let recognizers = pdfcraft_engine::ocr::recognizers(&settings).map_err(failed)?;
        let job = self.session.ocr_job(id, &pages, settings.clone()).ok_or_else(|| failed("the document could not be read"))?;
        let infos: std::collections::HashMap<usize, pdfcraft_render::PageInfo> =
            self.session.get(id).map(|d| d.info.pages.clone()).unwrap_or_default().into_iter().enumerate().collect();
        let guard = job.guard.clone();
        let found = job.run(&recognizers, |_, _| true);
        if found.len() > 65536 {
            return Err(failed("at most 65536 OCR review pages are allowed"));
        }
        pdfcraft_engine::ocr::validate_ocr_words(found.iter().flat_map(|p| &p.words)).map_err(failed)?;
        let band_name = |band: pdfcraft_engine::ocr::ConfidenceBand| match band {
            pdfcraft_engine::ocr::ConfidenceBand::High => "high",
            pdfcraft_engine::ocr::ConfidenceBand::Medium => "medium",
            pdfcraft_engine::ocr::ConfidenceBand::Low => "low",
        };
        let mut suspects = 0usize;
        let list: Vec<Value> = found
            .iter()
            .map(|p| {
                let info = infos.get(&p.page);
                let words: Vec<Value> = p
                    .words
                    .iter()
                    .map(|w| {
                        let suspect = pdfcraft_engine::ocr::is_suspect(w.confidence);
                        if suspect {
                            suspects += 1;
                        }
                        // The word's corners in view space, as the box [x, y, w, h] in points
                        // from the top-left of the displayed page.
                        let view = |pt: [f64; 2]| info.map(|i| i.user_to_view(pt[0] as f32, pt[1] as f32));
                        let corners = [
                            view(w.origin),
                            view([w.origin[0] + w.across[0], w.origin[1] + w.across[1]]),
                            view([w.origin[0] + w.up[0], w.origin[1] + w.up[1]]),
                            view([w.origin[0] + w.across[0] + w.up[0], w.origin[1] + w.across[1] + w.up[1]]),
                        ];
                        let xs: Vec<f32> = corners.iter().flatten().map(|v| v[0]).collect();
                        let ys: Vec<f32> = corners.iter().flatten().map(|v| v[1]).collect();
                        let (min_x, min_y) = (xs.iter().cloned().fold(f32::INFINITY, f32::min), ys.iter().cloned().fold(f32::INFINITY, f32::min));
                        let (w_pt, h_pt) = (
                            xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max) - min_x,
                            ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max) - min_y,
                        );
                        json!({
                            "text": w.text,
                            "box": [min_x, min_y, w_pt, h_pt],
                            "confidence": w.confidence.map(|c| (c * 10.0).round() / 10.0),
                            "band": w.confidence.map(|c| band_name(pdfcraft_engine::ocr::band_for(c))),
                            "suspect": suspect,
                            "source": w.source,
                        })
                    })
                    .collect();
                let mut page =
                    json!({ "page": p.page + 1, "already_on_page": p.already_on_page, "words": words, "mean_confidence": p.mean_confidence().map(|c| (c * 10.0).round() / 10.0) });
                if let Some(why) = &p.skipped {
                    page["skipped"] = json!(why);
                }
                page
            })
            .collect();
        let mut out = json!({ "words": found.iter().map(|p| p.words.len()).sum::<usize>(), "suspects": suspects, "pages": list });
        if retain {
            self.next_ocr_review =
                self.next_ocr_review.checked_add(1).filter(|n| *n <= i64::MAX as u64).ok_or_else(|| failed("OCR review handles exhausted"))?;
            out["review"] = json!(self.next_ocr_review);
            self.ocr_reviews.insert(id, crate::OcrReview { handle: self.next_ocr_review, guard, pages: found });
        }
        Ok(out)
    }

    /// Apply only caller corrections to retained original boxes, through the stored guard.
    pub(crate) fn ocr_review_apply(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        let handle = a.int("review")?;
        let review = self
            .ocr_reviews
            .get(&id)
            .filter(|r| i64::try_from(r.handle).ok() == Some(handle))
            .ok_or_else(|| failed("unknown OCR review for this document; call ocr_words with retain_review"))?;
        if a.opt_bool("discard")?.unwrap_or(false) {
            if a.get("corrections").is_some() {
                return Err(ToolError::InvalidArgs("discard cannot include corrections".into()));
            }
            self.ocr_reviews.remove(&id);
            return Ok(json!({ "discarded": handle }));
        }
        if let Some(why) = self.session.stale_reason(&review.guard) {
            return Err(failed(why));
        }
        let corrections = a.get("corrections").map(|v| v.as_array().ok_or_else(|| Args::wrong("corrections", "an array"))).transpose()?;
        if corrections.is_some_and(|c| c.len() > 65536) {
            return Err(failed("at most 65536 OCR corrections are allowed"));
        }
        // Prevalidate and cap every replacement before cloning the retained result.
        let mut edits = Vec::new();
        let mut bytes = 0usize;
        let mut seen = std::collections::BTreeSet::new();
        let page_indices: std::collections::BTreeMap<_, _> = review.pages.iter().enumerate().map(|(i, p)| (p.page, i)).collect();
        for item in corrections.into_iter().flatten() {
            if !item.is_object() || item.as_object().is_some_and(|o| o.keys().any(|k| !matches!(k.as_str(), "page" | "word" | "text"))) {
                return Err(ToolError::InvalidArgs("corrections accept only page, word and text".into()));
            }
            let c = Args(item);
            let page =
                usize::try_from(c.int("page")?).ok().and_then(|p| p.checked_sub(1)).ok_or_else(|| Args::wrong("page", "a 1-based position"))?;
            let word =
                usize::try_from(c.int("word")?).ok().and_then(|w| w.checked_sub(1)).ok_or_else(|| Args::wrong("word", "a 1-based position"))?;
            let text = c.str("text")?;
            bytes = bytes.checked_add(text.len()).filter(|n| *n <= 4 * 1024 * 1024).ok_or_else(|| failed("OCR corrections exceed 4 MiB"))?;
            if !seen.insert((page, word)) {
                return Err(failed("duplicate OCR word correction"));
            }
            let pi = page_indices.get(&page).copied().ok_or_else(|| failed("page is not in this OCR review"))?;
            if review.pages.get(pi).and_then(|p| p.words.get(word)).is_none() {
                return Err(failed("word is not in this OCR review"));
            }
            edits.push((pi, word, text.to_owned()));
        }
        let mut pages = review.pages.clone();
        for (pi, wi, text) in edits {
            if let Some(w) = pages.get_mut(pi).and_then(|p| p.words.get_mut(wi)) {
                w.text = text;
            }
        }
        pdfcraft_engine::ocr::validate_ocr_words(pages.iter().flat_map(|p| &p.words)).map_err(failed)?;
        for page in &mut pages {
            page.words.retain(|w| !w.text.trim().is_empty());
        }
        if pages.iter().all(|p| p.words.is_empty()) {
            return Err(failed("the OCR review has no words to apply"));
        }
        let out = match a.opt_str("output_mode")?.unwrap_or("searchable") {
            "searchable" => {
                if a.get("out").is_some() {
                    return Err(ToolError::InvalidArgs("out is only for editable output".into()));
                }
                let count = self.session.apply_ocr_checked(id, &pages, &review.guard).map_err(failed)?;
                json!({ "applied": count, "review": handle })
            }
            "editable" => {
                // The editable output writes the review's words out of the document: a
                // document that forbids copying refuses here (the review is kept).
                self.doc_to_copy(a)?;
                let target = self.resolve(a.str("out")?, true)?;
                if self
                    .session
                    .get(id)
                    .and_then(|d| d.path.as_deref())
                    .and_then(|p| std::fs::canonicalize(p).ok())
                    .as_ref()
                    .is_some_and(|source| std::fs::canonicalize(&target).as_ref().ok() == Some(source))
                {
                    return Err(failed("out is the source document; choose a new file"));
                }
                // A NEW file: an existing one is never replaced silently (the review is kept).
                if target.exists() {
                    return Err(failed(format!("out already exists ({}); choose a path that does not exist yet", target.to_string_lossy())));
                }
                let bytes = self.session.editable_ocr_bytes(id, &pages).map_err(failed)?;
                write_atomic(&target, &bytes)?;
                json!({ "out": target.to_string_lossy(), "review": handle, "words": pages.iter().map(|p| p.words.len()).sum::<usize>() })
            }
            _ => return Err(ToolError::InvalidArgs("output_mode must be searchable or editable".into())),
        };
        self.ocr_reviews.remove(&id);
        Ok(out)
    }

    /// Read/update the same per-language dictionary file used by OCR Verify.
    pub(crate) fn ocr_dictionary(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::ocr::dictionary;
        use std::io::Read as _;
        for key in ["add", "remove"] {
            if let Some(value) = a.get(key) {
                let values = value.as_array().ok_or_else(|| Args::wrong(key, "an array of strings"))?;
                if values.len() > 65536 {
                    return Err(failed("at most 65536 dictionary words are allowed"));
                }
            }
        }
        let add = if a.get("add").is_some() { a.strs("add")? } else { Vec::new() };
        let remove = if a.get("remove").is_some() { a.strs("remove")? } else { Vec::new() };
        let writing = a.get("add").is_some() || a.get("remove").is_some();
        // Updating sorts, lower-cases and rewrites the file, so this tool names dictionary files only:
        // pointed at `.env` or a README it would corrupt it.
        let file = std::path::Path::new(a.str("path")?).file_name().and_then(|n| n.to_str()).unwrap_or("");
        let plain = file.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if file.len() > 64 || !plain || !file.starts_with("ocr-dictionary") || !file.ends_with(".txt") {
            return Err(failed("path must name a dictionary file: ocr-dictionary-<language>.txt (letters, digits, '-', '_' and '.' only)"));
        }
        let path = self.resolve(a.str("path")?, writing)?;
        let mut input = String::new();
        match std::fs::File::open(&path) {
            Ok(file) => {
                file.take((dictionary::MAX_BYTES + 1) as u64).read_to_string(&mut input).map_err(|e| failed(format!("OCR dictionary: {e}")))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && writing => {}
            Err(e) => return Err(failed(format!("OCR dictionary: {e}"))),
        }
        let text = dictionary::update(&input, &add, &remove).map_err(failed)?;
        if writing {
            write_atomic(&path, text.as_bytes())?;
        }
        let words = dictionary::words(&text).map_err(failed)?;
        Ok(json!({ "path": path.to_string_lossy(), "count": words.len(), "words": words }))
    }

    pub(crate) fn ocr_status(&self) -> Result<Value> {
        use pdfcraft_engine::ocr;
        let dirs: Vec<String> = ocr::Models::search_dirs().iter().map(|d| d.to_string_lossy().into_owned()).collect();
        let langs: Vec<Value> = ocr::LANGUAGES.iter().map(|(c, n)| json!({ "code": c, "name": n })).collect();
        let engines: Vec<Value> = ocr::engines_status()
            .iter()
            .map(|e| {
                let mut v = json!({ "id": e.id, "available": e.available });
                if let Some(p) = &e.program {
                    v["path"] = json!(p);
                }
                if let Some(vv) = &e.version {
                    v["version"] = json!(vv);
                }
                if let Some(why) = &e.why_not {
                    v["unavailable"] = json!(why);
                }
                v
            })
            .collect();
        Ok(json!({ "available": ocr::available(), "search_dirs": dirs, "languages": langs, "engines": engines }))
    }
}

/// The file name an input is written under: its own file name, or `document.pdf` when it has none.
fn input_file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document.pdf".into())
}

/// The output file names that two or more of `targets` share, each spelling listed once, in input
/// order. Names are compared without regard to case on Windows and macOS and exactly elsewhere; on
/// Windows a trailing dot or space is not part of the name (`scan.pdf.` is `scan.pdf`). Unicode
/// normalization forms (NFC and NFD spellings of one name) are not compared, so such inputs are
/// not caught. The folder is the same for every target, so only the names are compared.
fn shared_file_names(targets: &[std::path::PathBuf]) -> Vec<String> {
    let mut groups: std::collections::BTreeMap<String, (usize, Vec<String>)> = std::collections::BTreeMap::new();
    for target in targets {
        let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let key = if cfg!(windows) {
            name.trim_end_matches(['.', ' ']).to_lowercase()
        } else if cfg!(target_os = "macos") {
            // Case only; NFC and NFD spellings of one name stay distinct here (a known limit).
            name.to_lowercase()
        } else {
            name.clone()
        };
        let (count, spellings) = groups.entry(key).or_insert_with(|| (0, Vec::new()));
        *count += 1;
        if !spellings.contains(&name) {
            spellings.push(name);
        }
    }
    groups.into_values().filter(|(count, _)| *count > 1).flat_map(|(_, spellings)| spellings).collect()
}

#[cfg(test)]
mod shared_output_name_tests {
    use super::*;

    #[test]
    fn shared_output_names_follow_the_default_filesystem() {
        let targets = |names: &[&str]| -> Vec<std::path::PathBuf> { names.iter().map(|n| std::path::Path::new("out").join(n)).collect() };
        // A spelling that repeats is one shared name, listed once.
        assert_eq!(shared_file_names(&targets(&["a.pdf", "b.pdf", "a.pdf"])), vec!["a.pdf".to_string()]);
        assert!(shared_file_names(&targets(&["a.pdf", "b.pdf"])).is_empty());
        // Case is ignored on Windows and macOS only; a trailing dot is ignored on Windows only.
        let case = shared_file_names(&targets(&["scan.pdf", "Scan.PDF"]));
        let dot = shared_file_names(&targets(&["scan.pdf", "scan.pdf."]));
        assert_eq!(case.len(), if cfg!(any(windows, target_os = "macos")) { 2 } else { 0 }, "{case:?}");
        assert_eq!(dot.len(), if cfg!(windows) { 2 } else { 0 }, "{dot:?}");
    }
}
