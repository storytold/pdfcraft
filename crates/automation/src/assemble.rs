use super::error::{Result, ToolError, failed};
use super::json::{Args, one_based, summary};
use super::paths::{child, read_input, read_input_bytes, unused, write_atomic};
use super::{Arc, Automation, CombineSource, Edit, Path, PathBuf, Value, json};

impl Automation {
    pub(crate) fn doc_create(&mut self, a: &Args) -> Result<Value> {
        let (name, bytes) = match a.str("from")? {
            "blank" => {
                let n = a.opt_int("pages")?.unwrap_or(1).clamp(1, 10_000) as usize;
                let (w, h) = (a.opt_num("width")?.unwrap_or(612.0), a.opt_num("height")?.unwrap_or(792.0));
                ("Untitled.pdf".to_string(), self.session.create_blank(w, h, n).map_err(failed)?)
            }
            "images" => {
                let resolution = a.opt_num("dpi")?.map_or(pdfcraft_engine::ImageResolution::Embedded, pdfcraft_engine::ImageResolution::Dpi);
                let mut images = Vec::new();
                for p in a.strs("paths")? {
                    let path = self.resolve(p, false)?;
                    let bytes = read_input(&path)?;
                    images.push((path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), bytes));
                }
                let name = if images.len() == 1 {
                    format!("{}.pdf", images[0].0.rsplit_once('.').map_or(images[0].0.as_str(), |(s, _)| s))
                } else {
                    "Images.pdf".into()
                };
                (name, self.session.create_from_images_with_resolution(&images, resolution).map_err(failed)?)
            }
            "text" => {
                let (title, text) = match (a.opt_str("text")?, a.opt_str("path")?) {
                    (Some(t), _) => ("Text".to_string(), t.to_string()),
                    (None, Some(p)) => {
                        let path = self.resolve(p, false)?;
                        let t = read_input(&path)?;
                        (path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), pdfcraft_engine::decode_text(&t))
                    }
                    (None, None) => return Err(ToolError::InvalidArgs("text needs `text` or `path`".into())),
                };
                // The page font is Helvetica/WinAnsi: text it would turn into `?` is refused (with
                // its line) by `pdfcraft_create::from_text`, as for every other caller.
                let bytes = self.session.create_from_text(&title, &text).map_err(|e| match e {
                    pdfcraft_engine::EditError::Create(pdfcraft_engine::CreateError::Invalid(m)) => ToolError::InvalidArgs(m),
                    e => failed(e),
                })?;
                (format!("{title}.pdf"), bytes)
            }
            other => return Err(ToolError::InvalidArgs(format!("unknown source {other:?}"))),
        };
        let name = a.opt_str("name")?.map(str::to_owned).unwrap_or(name);
        let id = self.session.open_new(name, bytes).map_err(failed)?;
        Ok(summary(self.session.get(id).ok_or_else(|| failed("the document vanished"))?))
    }

    pub(crate) fn page_transition_set(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::{Transition, TransitionSpec};
        let n = self.doc(a)?.info.pages.len();
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..n).collect(),
        };
        let transition = match a.opt_str("style")? {
            None | Some("none") => None,
            Some(style) => {
                let mut t = Transition::new(style);
                if let Some(d) = a.opt_num("duration")? {
                    t.duration = d as f32;
                }
                if let Some(dm) = a.opt_str("dimension")? {
                    t.horizontal = match dm {
                        "horizontal" => true,
                        "vertical" => false,
                        other => return Err(ToolError::InvalidArgs(format!("dimension must be horizontal or vertical, not {other:?}"))),
                    };
                }
                if let Some(m) = a.opt_str("motion")? {
                    t.inward = match m {
                        "inward" => true,
                        "outward" => false,
                        other => return Err(ToolError::InvalidArgs(format!("motion must be inward or outward, not {other:?}"))),
                    };
                }
                match a.get("direction") {
                    None => {}
                    Some(Value::String(s)) if s == "none" => t.direction = None,
                    Some(v) => {
                        t.direction = Some(
                            v.as_u64()
                                .and_then(|d| u16::try_from(d).ok())
                                .ok_or_else(|| ToolError::InvalidArgs("direction must be 0, 90, 180, 270, 315 or \"none\"".into()))?,
                        );
                    }
                }
                if let Some(s) = a.opt_num("scale")? {
                    t.scale = s as f32;
                }
                if let Some(b) = a.opt_bool("opaque")? {
                    t.opaque = b;
                }
                Some(t)
            }
        };
        let display = a.opt_num("display")?.map(|s| s as f32);
        self.apply(a, Edit::SetTransition { pages, spec: TransitionSpec { transition, display } })
    }

    pub(crate) fn page_set_box(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::{BoxSpec, PageBox};
        let doc = self.doc(a)?;
        let n = doc.info.pages.len();
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..n).collect(),
        };
        let which = match a.opt_str("box")? {
            None => PageBox::Crop,
            Some(b) => PageBox::from_name(b).ok_or_else(|| ToolError::InvalidArgs(format!("unknown box {b:?}")))?,
        };
        let nums = |key: &str| -> Result<Option<[f64; 4]>> {
            a.get(key)
                .map(|v| {
                    let v: Vec<f64> = v.as_array().map(|x| x.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                    <[f64; 4]>::try_from(v).map_err(|_| ToolError::InvalidArgs(format!("{key} must be 4 numbers")))
                })
                .transpose()
        };
        let spec = match (nums("margins")?, nums("rect")?) {
            (Some(_), Some(_)) => return Err(ToolError::InvalidArgs("pass margins or rect, not both".into())),
            (Some(m), None) => BoxSpec::Margins(m),
            (None, Some(r)) => {
                // Top-left-origin points on the displayed page → user space (one page at a time).
                if pages.len() != 1 {
                    return Err(ToolError::InvalidArgs("rect applies to a single page; use margins for several".into()));
                }
                let p = &doc.info.pages[pages[0]];
                let (u0, u1) = (p.view_to_user(r[0] as f32, r[1] as f32), p.view_to_user(r[2] as f32, r[3] as f32));
                BoxSpec::Rect([u0[0].min(u1[0]) as f64, u0[1].min(u1[1]) as f64, u0[0].max(u1[0]) as f64, u0[1].max(u1[1]) as f64])
            }
            (None, None) => BoxSpec::Remove,
        };
        let mut out = self.apply(a, Edit::SetPageBox { pages, which, spec })?;
        out["page_sizes"] = json!(self.doc(a)?.info.pages.iter().map(|p| [p.width, p.height]).collect::<Vec<_>>());
        Ok(out)
    }

    /// Crops multiple pages in a document using uniform, alternating odd/even, or auto-detected content margins.
    pub(crate) fn page_bulk_crop(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::{BoxSpec, Edit, PageBox};
        let doc = self.doc(a)?;
        let doc_id = doc.id;
        let n = doc.info.pages.len();
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..n).collect(),
        };
        let padding = a.opt_num("padding")?.unwrap_or(18.0);
        let auto_crop = a.opt_bool("auto_crop")?.unwrap_or(false);

        let nums = |key: &str| -> Result<Option<[f64; 4]>> {
            a.get(key)
                .map(|v| {
                    let v: Vec<f64> = v.as_array().map(|x| x.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                    <[f64; 4]>::try_from(v).map_err(|_| ToolError::InvalidArgs(format!("{key} must be 4 numbers")))
                })
                .transpose()
        };

        let odd_margins = nums("odd_margins")?;
        let even_margins = nums("even_margins")?;
        let margins = nums("margins")?;

        if margins.is_some() && (odd_margins.is_some() || even_margins.is_some()) {
            return Err(ToolError::InvalidArgs("cannot combine uniform margins with odd_margins or even_margins".into()));
        }

        if odd_margins.is_some() || even_margins.is_some() {
            if !auto_crop && (odd_margins.is_none() || even_margins.is_none()) {
                return Err(ToolError::InvalidArgs("both odd_margins and even_margins must be provided, or use auto_crop: true".into()));
            }

            let odd_p: Vec<usize> = pages.iter().copied().filter(|p| p % 2 == 0).collect();
            let even_p: Vec<usize> = pages.iter().copied().filter(|p| p % 2 != 0).collect();

            let odd_margins = match odd_margins {
                Some(m) => m,
                None => {
                    if odd_p.is_empty() {
                        [0.0; 4]
                    } else {
                        self.session
                            .detect_bulk_auto_crop(doc_id, &odd_p, padding)
                            .ok_or_else(|| ToolError::Failed("no content could be detected to crop on odd pages".into()))?
                    }
                }
            };

            let even_margins = match even_margins {
                Some(m) => m,
                None => {
                    if even_p.is_empty() {
                        [0.0; 4]
                    } else {
                        self.session
                            .detect_bulk_auto_crop(doc_id, &even_p, padding)
                            .ok_or_else(|| ToolError::Failed("no content could be detected to crop on even pages".into()))?
                    }
                }
            };

            let mut edits = Vec::new();
            if !odd_p.is_empty() {
                edits.push(Edit::SetPageBox { pages: odd_p, which: PageBox::Crop, spec: BoxSpec::Margins(odd_margins) });
            }
            if !even_p.is_empty() {
                edits.push(Edit::SetPageBox { pages: even_p, which: PageBox::Crop, spec: BoxSpec::Margins(even_margins) });
            }

            let mut out = self.apply(a, Edit::Batch { label: "Bulk crop pages".into(), edits })?;
            out["page_sizes"] = json!(self.doc(a)?.info.pages.iter().map(|p| [p.width, p.height]).collect::<Vec<_>>());
            return Ok(out);
        }

        let chosen_margins = match (margins, auto_crop) {
            (Some(m), _) => m,
            (None, true) => self
                .session
                .detect_bulk_auto_crop(doc_id, &pages, padding)
                .ok_or_else(|| ToolError::Failed("no content could be detected to crop".into()))?,
            (None, false) => {
                return Err(ToolError::InvalidArgs("must provide margins, odd_margins and even_margins, or auto_crop: true".into()));
            }
        };

        let spec = BoxSpec::Margins(chosen_margins);
        let mut out = self.apply(a, Edit::SetPageBox { pages, which: PageBox::Crop, spec })?;
        out["page_sizes"] = json!(self.doc(a)?.info.pages.iter().map(|p| [p.width, p.height]).collect::<Vec<_>>());
        Ok(out)
    }

    pub(crate) fn insert_blank(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let at = self.position(a, "at")?;
        // Default to the size of the neighbouring page, as Acrobat does; Letter for empty files.
        let near = doc.info.pages.get(at.saturating_sub(1)).or(doc.info.pages.first());
        let (w, h) = near.map(|p| (f64::from(p.width), f64::from(p.height))).unwrap_or((612.0, 792.0));
        let width = a.opt_num("width")?.unwrap_or(w);
        let height = a.opt_num("height")?.unwrap_or(h);
        if !(1.0..=14400.0).contains(&width) || !(1.0..=14400.0).contains(&height) {
            return Err(ToolError::InvalidArgs("width and height must be between 1 and 14400 points".into()));
        }
        self.apply(a, Edit::InsertBlankPage { at, width, height })
    }

    pub(crate) fn insert_file(&mut self, a: &Args) -> Result<Value> {
        let at = self.position(a, "at")?;
        let path = self.resolve(a.str("path")?, false)?;
        let bytes = read_input(&path)?;
        let pages = a.opt_ints("pages")?.map(|p| one_based(&p)).transpose()?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.apply(a, Edit::InsertPagesFrom { name, bytes: Arc::new(bytes), pages, at, field_conflicts: pdfcraft_organize::FieldConflicts::Rename })
    }

    /// Write freshly created bytes to `out` and/or open them as a new document.
    pub(crate) fn deliver(&mut self, a: &Args, name: &str, bytes: Arc<Vec<u8>>) -> Result<Value> {
        let mut result = json!({ "bytes": bytes.len() });
        let out = a.opt_str("out")?;
        if let Some(out) = out {
            let path = self.resolve(out, true)?;
            write_atomic(&path, &bytes)?;
            result["path"] = json!(path.to_string_lossy());
        }
        if a.opt_bool("open")?.unwrap_or(out.is_none()) {
            let id = self.session.open_new(name, bytes).map_err(failed)?;
            result["document"] = summary(self.session.get(id).ok_or_else(|| failed("the document vanished"))?);
        }
        Ok(result)
    }

    pub(crate) fn doc_create_multiple(&mut self, a: &Args) -> Result<Value> {
        let paths = a.strs("paths")?;
        if paths.is_empty() {
            return Err(ToolError::InvalidArgs("paths must list at least one file".into()));
        }
        let separate = match a.opt_str("mode")? {
            None | Some("combine") => false,
            Some("separate") => true,
            Some(m) => return Err(ToolError::InvalidArgs(format!("mode must be \"combine\" or \"separate\", not {m:?}"))),
        };
        if separate {
            return self.create_separate(a, &paths);
        }
        let ranges: Vec<Option<String>> = match a.get("pages") {
            None | Some(Value::Null) => vec![None; paths.len()],
            Some(Value::Array(v)) if v.len() == paths.len() => v.iter().map(|x| x.as_str().map(str::to_owned)).collect(),
            Some(_) => return Err(ToolError::InvalidArgs("pages must list a range (or null) for each path".into())),
        };
        // Each file is read, converted and copied in turn, so a long list holds one file at a time.
        let sources = paths.into_iter().zip(ranges).map(|(p, range)| -> std::result::Result<pdfcraft_engine::CombineSource, ToolError> {
            let path = self.resolve(p, false)?;
            let bytes = read_input(&path)?;
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let (_, pdf) = self.session.convert_to_pdf(&name, &Arc::new(bytes)).map_err(failed)?;
            let title = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Ok((title, pdf, range))
        });
        let bytes = self.session.combine_each(sources)?.ok_or_else(|| ToolError::InvalidArgs("paths must list at least one file".into()))?;
        self.deliver(a, "Combined", bytes)
    }

    /// One PDF per file, written into `out_dir`; a file that fails doesn't stop the others.
    fn create_separate(&mut self, a: &Args, paths: &[&str]) -> Result<Value> {
        let dir = self.resolve(a.str("out_dir").map_err(|_| ToolError::InvalidArgs("mode \"separate\" needs out_dir".into()))?, true)?;
        std::fs::create_dir_all(&dir).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
        let mut out = Vec::new();
        for &p in paths {
            let converted = self.resolve(p, false).map_err(|e| e.to_string()).and_then(|src| {
                let bytes = read_input_bytes(&src).map_err(|e| format!("{}: {e}", src.display()))?;
                let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let stem = src.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
                let (kind, pdf) = self.session.convert_to_pdf(&name, &Arc::new(bytes)).map_err(|e| e.to_string())?;
                Ok((kind, stem, pdf))
            });
            out.push(match converted {
                Ok((pdfcraft_engine::SourceKind::Pdf, ..)) => json!({ "path": p, "skipped": "already a PDF" }),
                Ok((_, stem, pdf)) => {
                    let target = unused(&dir, &stem);
                    write_atomic(&target, &pdf)?;
                    json!({ "path": p, "output": target.to_string_lossy(), "bytes": pdf.len() })
                }
                Err(e) => json!({ "path": p, "error": e }),
            });
        }
        Ok(json!({ "files": out }))
    }

    pub(crate) fn page_extract(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let (id, name) = (doc.id, format!("{} (extract)", doc.name));
        let stem = Path::new(&doc.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "page".into());
        let pages = self.pages(a, "pages")?;
        let mut out = if a.opt_bool("separate")?.unwrap_or(false) {
            // Each page as its own file.
            let dir = self.resolve(a.str("out_dir").map_err(|_| ToolError::InvalidArgs("separate files need out_dir".into()))?, true)?;
            std::fs::create_dir_all(&dir).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
            let mut files = Vec::new();
            for &p in &pages {
                let bytes = self.session.extract(id, &[p]).map_err(failed)?;
                let path = child(&dir, &format!("{stem}-page{}.pdf", p + 1));
                write_atomic(&path, &bytes)?;
                files.push(path.to_string_lossy().into_owned());
            }
            json!({ "files": files })
        } else {
            let bytes = self.session.extract(id, &pages).map_err(failed)?;
            self.deliver(a, &name, bytes)?
        };
        if a.opt_bool("delete")?.unwrap_or(false) {
            let deleted = self.apply(a, Edit::DeletePages { pages })?;
            out["original"] = deleted;
        }
        Ok(out)
    }

    pub(crate) fn doc_combine(&mut self, a: &Args) -> Result<Value> {
        let paths = a.strs("paths")?;
        if paths.len() < 2 {
            return Err(ToolError::InvalidArgs("combine needs at least two files".into()));
        }
        let ranges: Vec<Option<String>> = match a.get("pages") {
            None | Some(Value::Null) => vec![None; paths.len()],
            Some(Value::Array(v)) if v.len() == paths.len() => v
                .iter()
                .enumerate()
                .map(|(index, value)| match value {
                    Value::Null => Ok(None),
                    Value::String(range) => Ok(Some(range.clone())),
                    _ => Err(ToolError::InvalidArgs(format!("pages[{index}] must be a range string or null"))),
                })
                .collect::<Result<_>>()?,
            Some(_) => return Err(ToolError::InvalidArgs("pages must list a range (or null) for each path".into())),
        };
        let passwords: Vec<Option<String>> = match a.get("passwords") {
            None | Some(Value::Null) => vec![None; paths.len()],
            Some(Value::Array(v)) if v.len() == paths.len() && v.iter().all(|x| x.is_string() || x.is_null()) => {
                v.iter().map(|x| x.as_str().map(str::to_owned)).collect()
            }
            Some(_) => return Err(ToolError::InvalidArgs("passwords must list a password (or null) for each path".into())),
        };
        // Entries sharing a group number are one file split into parts placed apart.
        let groups: Option<Vec<Option<u64>>> = match a.get("groups") {
            None | Some(Value::Null) => None,
            Some(Value::Array(v)) if v.len() == paths.len() && v.iter().all(|x| x.is_u64() || x.is_null()) => {
                Some(v.iter().map(Value::as_u64).collect())
            }
            Some(_) => return Err(ToolError::InvalidArgs("groups must list a number (or null) for each path".into())),
        };
        let field_conflicts = match a.opt_str("field_conflicts")? {
            None | Some("rename") => pdfcraft_organize::FieldConflicts::Rename,
            Some("merge") => pdfcraft_organize::FieldConflicts::Merge,
            Some(other) => {
                return Err(ToolError::InvalidArgs(format!("field_conflicts must be \"rename\" or \"merge\", not {other:?}")));
            }
        };
        let mut sources: Vec<CombineSource> = Vec::new();
        let mut firsts: Vec<(u64, PathBuf, usize)> = Vec::new();
        for (i, (p, range)) in paths.into_iter().zip(ranges).enumerate() {
            let path = self.resolve(p, false)?;
            let group = groups.as_ref().and_then(|g| g.get(i).copied().flatten());
            let first = group.and_then(|g| firsts.iter().find(|f| f.0 == g));
            let bytes = match first {
                Some((g, first_path, at)) => {
                    if *first_path != path {
                        return Err(ToolError::InvalidArgs(format!("groups[{i}]: group {g} is {}, not {}", first_path.display(), path.display())));
                    }
                    sources.get(*at).map(|s| s.1.clone()).ok_or_else(|| failed("group source missing"))?
                }
                None => {
                    if let Some(g) = group {
                        firsts.push((g, path.clone(), sources.len()));
                    }
                    let bytes = Arc::new(read_input(&path)?);
                    // Images and text files are made into PDFs first (#569), as in the Combine files tab.
                    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.session.as_pdf(&file_name, &bytes).map_err(failed)?
                }
            };
            let name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            sources.push((name, bytes, range));
        }
        let passwords: Vec<Option<&str>> = passwords.iter().map(Option::as_deref).collect();
        let combined = match groups {
            None => self.session.combine_unlocked_reporting(&sources, &passwords, field_conflicts),
            Some(groups) => {
                // A path without a group is a file of its own: give it a key no group uses.
                let mut next = groups.iter().flatten().max().map_or(Some(0), |m| m.checked_add(1));
                let mut keys = Vec::with_capacity(groups.len());
                for g in &groups {
                    keys.push(match g {
                        Some(g) => *g,
                        None => {
                            let own = next.ok_or_else(|| ToolError::InvalidArgs("group numbers are too large".into()))?;
                            next = own.checked_add(1);
                            own
                        }
                    });
                }
                self.session.combine_grouped_reporting(&sources, &keys, &passwords, field_conflicts)
            }
        }
        .map_err(failed)?;
        let mut result = self.deliver(a, "Combined", combined.bytes)?;
        if !combined.renamed_fields.is_empty() {
            result["renamed_fields"] = json!(combined.renamed_fields.iter().map(|(old, new)| json!([old, new])).collect::<Vec<_>>());
        }
        Ok(result)
    }

    pub(crate) fn doc_split(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let id = doc.id;
        let stem = Path::new(&doc.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "part".into());
        let bookmarks = a.opt_bool("bookmarks")?.unwrap_or(false);
        let max_mb = a.opt_num("max_mb")?;
        let chosen = [a.get("every").is_some(), a.get("before").is_some(), bookmarks, max_mb.is_some()].iter().filter(|x| **x).count();
        if chosen != 1 {
            return Err(ToolError::InvalidArgs(
                "pass exactly one of every (pages per file), before (page numbers), bookmarks: true, or max_mb".into(),
            ));
        }
        let mut titles: Vec<(usize, String)> = Vec::new();
        let parts = if let Some(mb) = max_mb {
            if !mb.is_finite() || mb <= 0.0 {
                return Err(ToolError::InvalidArgs("max_mb must be positive".into()));
            }
            self.session.split_by_size(id, (mb * 1_048_576.0) as usize).map_err(failed)?
        } else {
            let by = match (a.opt_int("every")?, a.opt_ints("before")?) {
                (Some(n), None) if n > 0 => pdfcraft_organize::SplitBy::PageCount(n as usize),
                (None, Some(b)) => pdfcraft_organize::SplitBy::Before(one_based(&b)?),
                _ if bookmarks => {
                    titles = self.session.bookmark_splits(id);
                    if titles.is_empty() {
                        return Err(failed("the document has no top-level bookmarks"));
                    }
                    pdfcraft_organize::SplitBy::Before(titles.iter().map(|t| t.0).collect())
                }
                _ => return Err(ToolError::InvalidArgs("every must be a positive page count".into())),
            };
            self.session.split(id, &by).map_err(failed)?
        };
        let dir = self.resolve(a.str("out_dir")?, true)?;
        std::fs::create_dir_all(&dir).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
        let mut files = Vec::new();
        let mut used = std::collections::HashSet::new();
        for (i, (first, last, bytes)) in parts.iter().enumerate() {
            let safe = |t: &str| t.chars().map(|c| if c.is_alphanumeric() || " -_.,()".contains(c) { c } else { '_' }).collect::<String>();
            let mut base = match titles.iter().find(|t| t.0 + 1 == *first) {
                Some((_, t)) => format!("{stem}-{}", safe(t)),
                None => format!("{stem}-part{}", i + 1),
            };
            // A file name is at most 255 bytes: keep the base to 200, cut between characters, which leaves room for ".pdf" and a -N suffix.
            if let Some((end, _)) = base.char_indices().find(|(at, c)| at + c.len_utf8() > 200) {
                base.truncate(end);
            }
            let file = format!("{base}.pdf");
            // Equal titles (or titles that sanitize alike) get -2, -3, ... so no part overwrites another; compared case-insensitively.
            let mut unique = file;
            let mut n = 1;
            while !used.insert(unique.to_lowercase()) {
                n += 1;
                unique = format!("{base}-{n}.pdf");
            }
            let path = child(&dir, &unique);
            write_atomic(&path, bytes)?;
            files.push(json!({ "path": path.to_string_lossy(), "first_page": first, "last_page": last }));
        }
        Ok(json!({ "files": files }))
    }
}
