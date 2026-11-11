//! Printing tools: list printers, and print (or save the print-ready PDF) with Acrobat's Print
//! dialog options.

use pdfcraft_engine::print::{self, Binding, BookletSubset, Layout, Orientation, PAPERS, PageOrder, SizeMode, Subset, spool};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed, write_atomic};

/// The job's two-sided setting for `doc_print`'s `duplex`: `printer` sends none, so the printer's
/// own default applies (as the Print dialog's Printer default, #2422), except that cut and stack
/// needs single-sided sheets.
fn duplex(value: &str, cut_stack: bool) -> Result<Option<spool::Duplex>> {
    Ok(match value {
        "printer" if cut_stack => Some(spool::Duplex::Off),
        "printer" => None,
        "off" => Some(spool::Duplex::Off),
        "long-edge" if cut_stack => return Err(bad("cut-and-stack printing needs duplex off (single-sided sheets)")),
        "short-edge" if cut_stack => return Err(bad("cut-and-stack printing needs duplex off (single-sided sheets)")),
        "long-edge" => Some(spool::Duplex::LongEdge),
        "short-edge" => Some(spool::Duplex::ShortEdge),
        d => return Err(bad(format!("unknown duplex {d:?}"))),
    })
}

/// [`duplex`] for a job laid out as `layout`: the name is checked either way, and a booklet
/// decides two-sided itself (#2707) as far as the printer's driver takes it, as the Print dialog
/// does (`print::booklet_job_duplex`). `two_sided` says whether it does; it is only asked for a
/// booklet's both sides, the one job it changes.
fn job_duplex(value: &str, layout: &Layout, two_sided: impl FnOnce() -> Option<bool>) -> Result<Option<spool::Duplex>> {
    let chosen = duplex(value, matches!(layout, Layout::Multiple { order: PageOrder::CutStack, .. }))?;
    Ok(match layout {
        Layout::Booklet { subset: BookletSubset::BothSides, .. } => print::booklet_job_duplex(BookletSubset::BothSides, two_sided()),
        Layout::Booklet { subset, .. } => print::booklet_job_duplex(*subset, None),
        _ => chosen,
    })
}

/// `printer` (a name, or "default" for the default printer) in `printers`; `None` when the list
/// has no such printer.
fn printer_in(printers: Vec<spool::Printer>, printer: &str) -> Option<spool::Printer> {
    printers.into_iter().find(|p| if printer == "default" { p.default } else { p.name == printer })
}

/// An invalid-arguments error saying `m`.
fn bad(m: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(m.into())
}

/// Parse explicit custom dimensions like "300x500", "300 x 500", "300x500 pt", "300pt x 500pt", "Custom.300x500".
fn parse_custom_dimensions(s: &str) -> Option<(f64, f64)> {
    let s = s.trim().trim_start_matches("Custom.").trim_start_matches("custom.").trim();
    let (w_str, h_str) = s.split_once(['x', 'X', '*'])?;
    let parse_dim = |d: &str| {
        let d = d.trim().trim_end_matches("points").trim_end_matches("pt").trim();
        d.parse::<f64>().ok()
    };
    let w = parse_dim(w_str)?;
    let h = parse_dim(h_str)?;
    if w.is_finite() && h.is_finite() && w >= 72.0 && h >= 72.0 && w <= 14400.0 && h <= 14400.0 { Some((w, h)) } else { None }
}

/// `doc_print`'s `paper`, by its name, short form, driver-detected paper name, or custom dimensions.
/// Returns `((width, height), media_name)`.
fn resolve_paper(name: Option<&str>, printer: Option<&str>) -> Result<((f64, f64), Option<String>)> {
    if let Some(p) = name {
        // 1. Match standard PAPERS presets.
        for (index, (n, size)) in PAPERS.iter().enumerate() {
            if n.eq_ignore_ascii_case(p)
                || n.replace("US ", "").eq_ignore_ascii_case(p)
                || n.split(" (").next().is_some_and(|label| label.eq_ignore_ascii_case(p))
            {
                let media = print::cups_media(index).map(String::from);
                return Ok((*size, media));
            }
        }
        // 2. Match detected driver papers for the given printer.
        if let Some(printer_name) = printer {
            let driver_papers = spool::printer_papers(printer_name);
            if let Some((name, size)) = driver_papers.iter().find(|(n, _)| n.eq_ignore_ascii_case(p)) {
                return Ok((*size, Some(print::paper_media(Some(name), *size))));
            }
        }
        // 3. Match custom dimensions ("<w>x<h>").
        if let Some(size) = parse_custom_dimensions(p) {
            return Ok((size, Some(print::paper_media(None, size))));
        }
        // 4. Explicit units request the same stock as point dimensions.
        if let Some(size) = paper_size(p) {
            return Ok((size, Some(print::paper_media(None, size))));
        }
        return Err(bad(format!(
            "unknown paper {p:?}; choose one of {}, a paper from printer_papers, or a size such as 8.5x14in or 210x297mm (1 to 200 inches a side)",
            PAPERS.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", ")
        )));
    }

    // When paper is omitted and a printer is given, use the printer's configured default paper if readable.
    if let Some(printer_name) = printer
        && let Some((name, size)) = spool::printer_default_paper(printer_name)
    {
        return Ok((size, Some(print::paper_media(Some(&name), size))));
    }

    let (name, size) = PAPERS[0];
    let media = print::paper_media(Some(name), size);
    Ok((size, Some(media)))
}

/// The spooler job for `doc_print` to `printer`. It feeds the paper the PDF was laid out on
/// (as `media` after the driver's `options`) rather than the queue's default paper, which a job
/// naming none got (A4 under Letter sheets on the queue in #3752), and tells CUPS which way the
/// laid-out `sheets` run, as the Print dialog does (#2713): an orientation the caller chose in
/// `options` stands. It asks the driver for grayscale only when the pages aren't grey pictures
/// already ([`print::driver_needs_grayscale`]).
fn spool_job(
    a: &Args,
    printer: &str,
    title: String,
    duplex: Option<spool::Duplex>,
    media: Option<&str>,
    raster: Option<print::Raster>,
    sheets: &[print::Sheet],
) -> Result<spool::Job> {
    let options = match a.get("options") {
        None => Vec::new(),
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| match v.as_str() {
                Some(v) => Ok((k.clone(), v.to_string())),
                None => Err(bad(format!("options.{k} must be a string (a choice from printer_options)"))),
            })
            .collect::<Result<Vec<_>>>()?,
        Some(_) => return Err(bad("options must be an object of option keys and choices from printer_options")),
    };
    Ok(spool::Job {
        printer: if printer == "default" { None } else { Some(printer.to_string()) },
        copies: a.opt_int("copies")?.unwrap_or(1).clamp(1, 999) as u32,
        collate: a.opt_bool("collate")?.unwrap_or(true),
        duplex,
        scaling: spool::Scaling::Exact,
        grayscale: print::driver_needs_grayscale(a.opt_bool("grayscale")?.unwrap_or(false), raster),
        // Set when the copy sent is made grey (`spooler_copy`, #5806).
        grey_pages: false,
        title,
        options: {
            let mut options: Vec<(String, String)> = options.into_iter().chain(media.map(|m| ("media".to_string(), m.to_string()))).collect();
            print::add_cups_orientation(&mut options, sheets);
            options
        },
    })
}

impl Automation {
    /// Print production ▸ Convert colours: every colour on the pages in gray, rgb or cmyk.
    pub(crate) fn convert_colours(&mut self, a: &Args) -> Result<Value> {
        let n = self.doc(a)?.info.pages.len();
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..n).collect(),
        };
        let target = match a.opt_str("to")?.unwrap_or("gray") {
            "gray" | "grayscale" => pdfcraft_engine::ColourTarget::Gray,
            "rgb" => pdfcraft_engine::ColourTarget::Rgb,
            "cmyk" => pdfcraft_engine::ColourTarget::Cmyk,
            other => return Err(ToolError::InvalidArgs(format!("unknown target {other:?} (gray, rgb, cmyk)"))),
        };
        let settings = pdfcraft_engine::ConvertColours {
            target,
            preserve_black: a.opt_bool("preserve_black")?.unwrap_or(true),
            images: a.opt_bool("images")?.unwrap_or(true),
            annotations: a.opt_bool("annotations")?.unwrap_or(false),
        };
        let id = self.doc(a)?.id;
        let found = self.session.count_colours(id, &pages, &settings);
        if found.colours == 0 && found.images == 0 {
            return Ok(json!({ "colours": 0, "images": 0, "images_left": found.images_left, "pages": 0, "loses_conformance": [] }));
        }
        // PDF/X has no declaration of its own here: it always carries an output intent.
        let declared = self.doc(a)?.standards();
        let pdfa = declared.pdfa.map(|(part, level)| format!("PDF/A-{part}{}", level.to_lowercase()));
        let loses: Vec<String> = pdfa.into_iter().chain(declared.output_intents.iter().map(|i| format!("output intent {i}"))).collect();
        let r = self.apply(a, pdfcraft_engine::Edit::ConvertColours { pages, settings })?;
        Ok(json!({
            "colours": found.colours,
            "images": found.images,
            "images_left": found.images_left,
            "pages": found.pages,
            "loses_conformance": loses,
            "undo": r["undo"],
        }))
    }

    /// `printers`: the system's printers, the default marked, and each one's two-sided support
    /// where the spooler reports it.
    pub(crate) fn printers(&self) -> Result<Value> {
        let list: Vec<Value> = spool::printers()
            .into_iter()
            .map(|p| {
                let mut v = json!({ "name": p.name, "default": p.default });
                if let Some(two_sided) = p.two_sided {
                    v["two_sided"] = json!(two_sided);
                }
                v
            })
            .collect();
        Ok(json!({ "count": list.len(), "printers": list }))
    }

    pub(crate) fn printer_options(&self, a: &Args) -> Result<Value> {
        let printer = a.str("printer")?;
        let list: Vec<Value> = spool::printer_options(printer)
            .into_iter()
            .map(|o| {
                let choices: Vec<Value> = o.choices.iter().map(|(value, label)| json!({ "value": value, "label": label })).collect();
                json!({ "key": o.key, "label": o.label, "group": o.group, "default": o.default, "choices": choices })
            })
            .collect();
        Ok(json!({ "printer": printer, "count": list.len(), "options": list }))
    }

    /// `printer_papers`: the paper sizes (names and dimensions in points) and default paper
    /// reported by the printer driver.
    pub(crate) fn printer_papers(&self, a: &Args) -> Result<Value> {
        let printer = a.str("printer")?;
        let resolved = if printer == "default" {
            spool::printers().into_iter().find(|p| p.default).map(|p| p.name).unwrap_or_else(|| printer.to_string())
        } else {
            printer.to_string()
        };
        let (papers, default_paper) = spool::printer_paper_info(&resolved);
        let list: Vec<Value> = papers.into_iter().map(|(name, (width, height))| json!({ "name": name, "width": width, "height": height })).collect();
        let default_val = default_paper.map(|(name, (width, height))| json!({ "name": name, "width": width, "height": height }));
        Ok(json!({ "printer": printer, "count": list.len(), "papers": list, "default": default_val }))
    }

    /// `doc_print`: lay the document out with the Print dialog's options, then save the
    /// print-ready PDF to `path` or send it to `printer` with the chosen paper.
    pub(crate) fn doc_print(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let (id, count) = (doc.id, doc.info.pages.len());
        let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
        let subset = match a.opt_str("subset")?.unwrap_or("all") {
            "all" => Subset::All,
            "odd" => Subset::Odd,
            "even" => Subset::Even,
            s => return Err(bad(format!("unknown subset {s:?}"))),
        };
        let pages = print::select_pages(count, a.opt_str("pages")?, &labels, subset, a.opt_bool("reverse")?.unwrap_or(false))
            .map_err(|e| bad(e.to_string()))?;
        let layout = match a.opt_str("layout")?.unwrap_or("fit") {
            "custom" => Layout::Size(SizeMode::Custom(a.opt_num("scale")?.ok_or_else(|| bad("custom needs scale (percent)"))?)),
            "multiple" => {
                // Custom pages per sheet: `cols` across by `rows` down, both or neither.
                let grid = match (a.opt_int("cols")?, a.opt_int("rows")?) {
                    (None, None) => None,
                    (Some(cols), Some(rows)) => {
                        if a.opt_int("per_sheet")?.is_some() {
                            return Err(bad("give per_sheet, or cols and rows, not both"));
                        }
                        let grid = usize::try_from(cols).ok().zip(usize::try_from(rows).ok()).and_then(|(cols, rows)| Layout::grid(cols, rows));
                        Some(grid.ok_or_else(|| bad(format!("cols and rows must each be 1 to {}", print::MAX_GRID)))?)
                    }
                    _ => return Err(bad("a custom grid needs both cols and rows")),
                };
                let per = a.opt_int("per_sheet")?.unwrap_or(2).clamp(1, 256) as usize;
                let order = match a.opt_str("order")?.unwrap_or("horizontal") {
                    "horizontal" => PageOrder::Horizontal,
                    "horizontal-reversed" => PageOrder::HorizontalReversed,
                    "vertical" => PageOrder::Vertical,
                    "vertical-reversed" => PageOrder::VerticalReversed,
                    "cut-stack" => PageOrder::CutStack,
                    o => return Err(bad(format!("unknown order {o:?}"))),
                };
                match grid.unwrap_or_else(|| Layout::multiple(per)) {
                    Layout::Multiple { cols, rows, per_sheet, custom, .. } => Layout::Multiple {
                        cols,
                        rows,
                        per_sheet,
                        order,
                        border: a.opt_bool("border")?.unwrap_or(false),
                        auto_rotate: a.opt_bool("auto_rotate")?.unwrap_or(true),
                        custom,
                    },
                    l => l,
                }
            }
            "booklet" => Layout::Booklet {
                subset: match a.opt_str("booklet_subset")?.unwrap_or("both") {
                    "both" => BookletSubset::BothSides,
                    "front" => BookletSubset::FrontOnly,
                    "back" => BookletSubset::BackOnly,
                    s => return Err(bad(format!("unknown booklet_subset {s:?}"))),
                },
                binding: if a.opt_str("binding")? == Some("right") { Binding::Right } else { Binding::Left },
                creep: a.opt_num("creep")?.unwrap_or(0.0),
            },
            "poster" => Layout::Poster {
                scale: a.opt_num("scale")?.unwrap_or(200.0),
                overlap: a.opt_num("overlap")?.unwrap_or(18.0),
                cut_marks: a.opt_bool("cut_marks")?.unwrap_or(true),
            },
            l => Layout::Size(print::names::parse_size_mode(l, 100.0).ok_or_else(|| bad(format!("unknown layout {l:?}")))?),
        };
        let printer = a.opt_str("printer")?;
        // The printer whose driver a booklet's both sides were decided for. The job goes to it by
        // name, so "default" can't become another printer while the document is rendered. The
        // queues are listed without asking every driver (#2192); only this one is asked.
        let mut asked = None;
        let duplex = job_duplex(a.opt_str("duplex")?.unwrap_or("off"), &layout, || {
            asked = printer.and_then(|p| printer_in(spool::printer_queues(), p));
            spool::two_sided(&asked.as_ref()?.name)
        })?;
        if (a.opt_int("cols")?.is_some() || a.opt_int("rows")?.is_some()) && !matches!(layout, Layout::Multiple { .. }) {
            return Err(bad("cols and rows need layout multiple"));
        }
        if a.opt_str("order")? == Some("cut-stack") && !matches!(layout, Layout::Multiple { .. }) {
            return Err(bad("cut-stack order needs layout multiple"));
        }
        let orientation = match a.opt_str("orientation")?.unwrap_or("auto") {
            "auto" => Orientation::Auto,
            "portrait" => Orientation::Portrait,
            "landscape" => Orientation::Landscape,
            o => return Err(bad(format!("unknown orientation {o:?}"))),
        };
        let content = a.opt_str("comments_forms")?.unwrap_or("document-and-markups");
        let content = print::names::parse_content(content).ok_or_else(|| bad(format!("unknown comments_forms {content:?}")))?;
        // Use the same queue for duplex detection, paper discovery and submission, even if the
        // system default changes while the document is rendered.
        let resolved_printer = printer.map(|p| {
            if let Some(asked) = &asked {
                asked.name.clone()
            } else if p == "default" {
                spool::printer_queues().into_iter().find(|pr| pr.default).map(|pr| pr.name).unwrap_or_else(|| p.to_string())
            } else {
                p.to_string()
            }
        });
        let (paper, media) = resolve_paper(a.opt_str("paper")?, resolved_printer.as_deref())?;
        // Print as image (Acrobat's Advanced Print Setup): pictures at `dpi`, grey when
        // `grayscale` is also asked for.
        let raster = if a.opt_bool("as_image")?.unwrap_or(false) {
            let dpi = a.opt_num("dpi")?.unwrap_or(300.0);
            if !(72.0..=600.0).contains(&dpi) {
                return Err(bad("dpi must be between 72 and 600"));
            }
            Some(print::Raster { dpi, grayscale: a.opt_bool("grayscale")?.unwrap_or(false) })
        } else {
            None
        };
        let sizes: Vec<(f64, f64)> = self.doc(a)?.info.pages.iter().map(|p| (p.width as f64, p.height as f64)).collect();
        // Print a selected area: points from the top-left of the displayed page, as other tools
        // take rectangles; the print crate takes it y up.
        let area = match crate::content::rect_arg(a, "area")? {
            None => None,
            Some(r) => {
                let [page] = pages.as_slice() else { return Err(bad("area prints one page: give pages as a single page")) };
                let Some(&(_, h)) = sizes.get(*page) else { return Err(bad("area's page is not in the document")) };
                Some([r[0].min(r[2]), h - r[1].max(r[3]), r[0].max(r[2]), h - r[1].min(r[3])])
            }
        };
        let settings = print::Settings { pages, paper, orientation, layout, content, raster, crops: Vec::new(), area };
        let laid_out = print::layout(&sizes, &settings).map_err(|e| bad(e.to_string()))?;
        let sheets = laid_out.len();
        let bytes = self.session.print_pdf(id, &settings).map_err(failed)?;
        let mut out = json!({ "sheets": sheets, "pages": settings.pages.len(), "bytes": bytes.len() });
        match (a.opt_str("path")?, printer) {
            (Some(p), None) => {
                let path = self.resolve(p, true)?;
                write_atomic(&path, &bytes)?;
                out["path"] = json!(path.to_string_lossy());
            }
            (None, Some(printer)) => {
                let actual_printer = resolved_printer.as_deref().unwrap_or(printer);
                let mut job = spool_job(a, actual_printer, self.doc(a)?.name.clone(), duplex, media.as_deref(), settings.raster, &laid_out)?;
                // A GDI (v3) driver gets transparent sheets flattened (#6155, #5838, #5940); XPS
                // (v4) drivers print them as they are.
                let flatten = job.printer.as_deref().is_some_and(spool::gdi_driver);
                let bytes = pdfcraft_engine::spooler::spooler_copy(bytes, &mut job, flatten);
                out["job"] = json!(spool::submit(&bytes, &job).map_err(|e| failed(e.to_string()))?);
            }
            _ => return Err(bad("pass either path (save the print-ready PDF) or printer (a name from printers, or \"default\")")),
        }
        Ok(out)
    }
}

/// "WxH" with a unit (in, mm, cm or pt) as a portrait paper size in points; each side 1 to 200
/// inches (PDF's own page limit).
fn paper_size(spec: &str) -> Option<(f64, f64)> {
    let s = spec.trim().to_ascii_lowercase();
    let (body, per_point) = [("in", 1.0 / 72.0), ("mm", 25.4 / 72.0), ("cm", 2.54 / 72.0), ("pt", 1.0)]
        .into_iter()
        .find_map(|(unit, per_point)| s.strip_suffix(unit).map(|b| (b, per_point)))?;
    let (w, h) = body.split_once('x')?;
    let side = |t: &str| t.trim().parse::<f64>().ok().map(|v| v / per_point).filter(|v| (72.0..=14_400.0).contains(v));
    let (w, h) = (side(w)?, side(h)?);
    Some((w.min(h), w.max(h)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #2707: a booklet sets two-sided itself whatever `duplex` asks; a bad name is still an error.
    /// #5772: its both sides leave two-sided to a driver that doesn't take the setting, and only
    /// they look the printer up.
    #[test]
    fn a_booklet_decides_two_sided_itself() {
        let booklet = |subset| Layout::Booklet { subset, binding: Binding::Left, creep: 0.0 };
        let unread = || -> Option<bool> { panic!("the printer's two-sided support is read for nothing") };
        for asked in ["printer", "off", "long-edge", "short-edge"] {
            assert_eq!(job_duplex(asked, &booklet(BookletSubset::BothSides), || Some(true)).ok(), Some(Some(spool::Duplex::ShortEdge)), "{asked}");
            assert_eq!(job_duplex(asked, &booklet(BookletSubset::BothSides), || None).ok(), Some(Some(spool::Duplex::ShortEdge)), "{asked}");
            assert_eq!(job_duplex(asked, &booklet(BookletSubset::BothSides), || Some(false)).ok(), Some(None), "{asked}");
            assert_eq!(job_duplex(asked, &booklet(BookletSubset::FrontOnly), unread).ok(), Some(Some(spool::Duplex::Off)), "{asked}");
            assert_eq!(job_duplex(asked, &booklet(BookletSubset::BackOnly), unread).ok(), Some(Some(spool::Duplex::Off)), "{asked}");
        }
        assert!(job_duplex("sideways", &booklet(BookletSubset::BothSides), || None).is_err());
        assert_eq!(job_duplex("long-edge", &Layout::Size(SizeMode::Fit), unread).ok(), Some(Some(spool::Duplex::LongEdge)), "other layouts keep it");
    }

    /// #5772: a booklet's printer is found by name, or as the default for "default", and only
    /// what the list says about it counts. "default" is found by its name, which the job is sent
    /// to.
    #[test]
    fn a_booklets_printer_comes_from_the_list() {
        let printer = |name: &str, default, two_sided| spool::Printer { name: name.into(), default, two_sided, description: String::new() };
        let list = [printer("Epson", false, Some(false)), printer("Laser", true, Some(true)), printer("Network", false, None)];
        let found = |list: &[spool::Printer], name| printer_in(list.to_vec(), name).map(|p| (p.name, p.two_sided));
        assert_eq!(found(&list, "Epson"), Some(("Epson".into(), Some(false))));
        assert_eq!(found(&list, "Laser"), Some(("Laser".into(), Some(true))));
        assert_eq!(found(&list, "default"), Some(("Laser".into(), Some(true))), "the default printer");
        assert_eq!(found(&list, "Network"), Some(("Network".into(), None)), "the list doesn't say");
        assert_eq!(found(&list, "Gone"), None, "no such printer");
        assert_eq!(found(&list, "epson"), None, "names are exact");
        let default_one_sided = [printer("Epson", true, Some(false)), printer("Laser", false, Some(true))];
        assert_eq!(found(&default_one_sided, "default"), Some(("Epson".into(), Some(false))), "the default printer");
        let without_default = [printer("Epson", false, Some(false))];
        assert_eq!(found(&without_default, "default"), None);
    }

    /// `printer` leaves two-sided to the printer's own default (#2422); cut and stack still asks
    /// for single-sided sheets, and refuses an explicit edge.
    #[test]
    fn duplex_printer_leaves_two_sided_to_the_printer() {
        assert_eq!(duplex("printer", false).ok(), Some(None));
        assert_eq!(duplex("printer", true).ok(), Some(Some(spool::Duplex::Off)));
        assert_eq!(duplex("off", false).ok(), Some(Some(spool::Duplex::Off)));
        assert_eq!(duplex("long-edge", false).ok(), Some(Some(spool::Duplex::LongEdge)));
        assert_eq!(duplex("short-edge", false).ok(), Some(Some(spool::Duplex::ShortEdge)));
        assert!(duplex("long-edge", true).is_err());
        assert!(duplex("sideways", false).is_err());
    }

    /// #2713, #3739: `doc_print` to a printer tells CUPS when every sheet runs landscape, after
    /// the paper, so a landscape sheet is turned onto the portrait paper rather than printed
    /// unrotated and cut off. Portrait and mixed jobs say nothing, and an orientation passed in
    /// `options` stands and isn't sent twice.
    #[test]
    fn printing_landscape_sheets_to_a_printer_asks_cups_to_turn_them() {
        let sheets = |sizes: &[(f64, f64)]| sizes.iter().map(|&size| print::Sheet { size, ..Default::default() }).collect::<Vec<_>>();
        let job = |args: Value, s: &[print::Sheet]| spool_job(&Args(&args), "Office", "a.pdf".into(), None, Some("Letter"), None, s).unwrap().options;
        let media = ("media".to_string(), "Letter".to_string());
        let landscape = sheets(&[(792.0, 612.0), (792.0, 612.0)]);
        let turned = if cfg!(windows) { vec![media.clone()] } else { vec![media.clone(), ("orientation-requested".to_string(), "4".to_string())] };
        assert_eq!(job(json!({}), &landscape), turned);
        assert_eq!(job(json!({}), &sheets(&[(612.0, 792.0)])), vec![media.clone()], "portrait sheets");
        assert_eq!(job(json!({}), &sheets(&[(792.0, 612.0), (612.0, 792.0)])), vec![media.clone()], "a job that mixes them");
        let chosen = ("orientation-requested".to_string(), "5".to_string());
        assert_eq!(job(json!({ "options": { "orientation-requested": "5" } }), &landscape), [chosen, media], "the caller's own choice, once");
    }

    /// #3752: `doc_print` to a printer sent no paper, so the queue's default paper stood in for
    /// the one the sheets were laid out on (A4 under Letter sheets on the report's queue).
    #[test]
    fn printing_to_a_printer_feeds_the_laid_out_paper() {
        let job = |args: Value, paper_name: Option<&str>| {
            let (_, media) = resolve_paper(paper_name, None).unwrap();
            spool_job(&Args(&args), "Office", "a.pdf".into(), None, media.as_deref(), None, &[]).unwrap()
        };
        let media = |m: &str| ("media".to_string(), m.to_string());
        // US Letter when no paper is asked for, after the driver's own options.
        let letter = job(json!({ "options": { "InputSlot": "Tray3" } }), None);
        assert_eq!(letter.options, [("InputSlot".to_string(), "Tray3".to_string()), media("Letter")]);
        assert_eq!((letter.printer.as_deref(), letter.title.as_str()), (Some("Office"), "a.pdf"));
        // Every paper, by its full name, goes out as its own media name.
        for (i, (name, size)) in PAPERS.iter().enumerate() {
            assert_eq!(resolve_paper(Some(name), None).ok(), Some((*size, Some(print::cups_media(i).unwrap().to_string()))), "{name}");
            assert_eq!(job(json!({}), Some(name)).options, print::media_option(i).into_iter().collect::<Vec<_>>(), "{name}");
        }
        // Short forms find the same paper.
        assert_eq!(job(json!({}), Some("letter")).options, [media("Letter")]);
        assert_eq!(job(json!({}), Some("ARCH D")).options, [media("na_arch-d_24x36in")]);
        // Non-preset dimensions and driver papers carry their media selection.
        assert_eq!(job(json!({}), Some("300x500")).options, [media("Custom.300x500")]);
        assert_eq!(job(json!({}), Some("300 x 500 pt")).options, [media("Custom.300x500")]);
        assert_eq!(job(json!({}), Some("300pt x 500pt")).options, [media("Custom.300x500")]);
        assert_eq!(job(json!({}), Some("300 pt x 500 pt")).options, [media("Custom.300x500")]);
        assert_eq!(job(json!({}), Some("Custom.300x500")).options, [media("Custom.300x500")]);
        assert_eq!(job(json!({}), Some("Custom.300x500pt")).options, [media("Custom.300x500")]);

        assert!(matches!(resolve_paper(Some("Quarto"), None), Err(ToolError::InvalidArgs(m)) if m.contains("US Letter")));
        // Every accepted dimension syntax requests the same stock from the queue.
        for (input, size) in [
            ("8.5x14in", (612.0, 1008.0)),
            ("210x297mm", (210.0 * 72.0 / 25.4, 297.0 * 72.0 / 25.4)),
            ("10x15cm", (10.0 * 72.0 / 2.54, 15.0 * 72.0 / 2.54)),
            ("300x500pt", (300.0, 500.0)),
        ] {
            let expected = print::paper_media(None, size);
            let (resolved_size, resolved_media) = resolve_paper(Some(input), None).unwrap();
            assert!((resolved_size.0 - size.0).abs() < 0.01 && (resolved_size.1 - size.1).abs() < 0.01, "{input}");
            assert_eq!(resolved_media.as_deref(), Some(expected.as_str()), "{input}");
            assert_eq!(job(json!({}), Some(input)).options, [media(&expected)], "{input}");
        }
        assert_eq!(spool_job(&Args(&json!({})), "default", String::new(), None, Some("Letter"), None, &[]).ok().map(|j| j.printer), Some(None));
        assert!(spool_job(&Args(&json!({ "options": { "InputSlot": 3 } })), "Office", String::new(), None, Some("Letter"), None, &[]).is_err());
    }

    /// #5806: grayscale with Print as image sends grey pictures, so the driver isn't asked for
    /// grayscale, which Microsoft Print to PDF keeps no setting for and the job was refused.
    #[test]
    fn grey_pictures_dont_ask_the_driver_for_grayscale() {
        let grey = print::Raster { dpi: 300.0, grayscale: true };
        let colour = print::Raster { dpi: 300.0, grayscale: false };
        let job = |args: Value, raster: Option<print::Raster>| {
            spool_job(&Args(&args), "Office", String::new(), None, Some("Letter"), raster, &[]).unwrap().grayscale
        };
        assert!(job(json!({ "grayscale": true }), None), "vector pages: the driver makes them grey");
        assert!(!job(json!({ "grayscale": true }), Some(grey)), "grey pictures: nothing for the driver to do");
        assert!(job(json!({ "grayscale": true }), Some(colour)), "colour pictures still need the driver");
        assert!(!job(json!({}), None));
    }

    #[test]
    fn a_paper_size_reads_its_numbers_and_needs_a_unit() {
        assert_eq!(paper_size("8.5 x 14 in"), Some((612.0, 1008.0)));
        assert_eq!(paper_size("1e3x1e3pt"), Some((1000.0, 1000.0)));
        for spec in ["0x0mm", "-5x10cm", "5x-10cm", "10x15"] {
            assert_eq!(paper_size(spec), None, "{spec}");
        }
    }
}
