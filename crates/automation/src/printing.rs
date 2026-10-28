//! Printing tools: list printers, and print (or save the print-ready PDF) with Acrobat's Print
//! dialog options.

use pdfcraft_engine::print::{self, Binding, BookletSubset, Content, Layout, Orientation, PAPERS, PageOrder, SizeMode, Subset, spool};
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
/// decides two-sided itself (#2707), as the Print dialog does (`print::booklet_duplex`).
fn job_duplex(value: &str, layout: &Layout) -> Result<Option<spool::Duplex>> {
    let chosen = duplex(value, matches!(layout, Layout::Multiple { order: PageOrder::CutStack, .. }))?;
    Ok(match layout {
        Layout::Booklet { subset, .. } => Some(print::booklet_duplex(*subset)),
        _ => chosen,
    })
}

/// An invalid-arguments error saying `m`.
fn bad(m: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(m.into())
}

/// `doc_print`'s `paper`, by its name or a short form ("Letter", "ARCH D"), as its `PAPERS`
/// index and size; US Letter when none is given.
fn paper_named(name: Option<&str>) -> Result<(usize, (f64, f64))> {
    let mut papers = PAPERS.iter().enumerate();
    let found = match name {
        None => papers.next(),
        Some(p) => papers.find(|(_, (n, _))| {
            n.eq_ignore_ascii_case(p)
                || n.replace("US ", "").eq_ignore_ascii_case(p)
                || n.split(" (").next().is_some_and(|name| name.eq_ignore_ascii_case(p))
        }),
    };
    found.map(|(index, (_, size))| (index, *size)).ok_or_else(|| {
        bad(format!(
            "unknown paper {:?}; choose one of {}",
            name.unwrap_or_default(),
            PAPERS.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", ")
        ))
    })
}

/// The sheet a job is laid out on, in points, and the `media` option its job names.
type PaperChoice = ((f64, f64), Option<(String, String)>);

/// The sheet `doc_print` lays out on and the `media` its job names, for `name` among the standard
/// sizes and the printer's own (`own`, from [`spool::printer_papers`]). A standard size is named by
/// the printer's own keyword for that size when it has one, else by its CUPS name
/// ([`print::media_option`]); one of the printer's own sizes by its keyword. With none named, the
/// printer's default size when its driver reports one, else US Letter.
fn paper_choice(name: Option<&str>, own: &[spool::PrinterPaper]) -> Result<PaperChoice> {
    let media = |key: &str| ("media".to_string(), key.to_string());
    if name.is_none()
        && let Some(q) = own.iter().find(|q| q.default)
    {
        return Ok((q.size, Some(media(&q.key))));
    }
    match paper_named(name) {
        Ok((index, size)) => {
            let named = spool::paper_of_size(own, size).and_then(|i| own.get(i)).map(|q| media(&q.key));
            Ok((size, named.or_else(|| print::media_option(index))))
        }
        Err(e) => {
            let p = name.unwrap_or_default();
            match own.iter().find(|q| q.key.eq_ignore_ascii_case(p) || q.label.eq_ignore_ascii_case(p)) {
                Some(q) => Ok((q.size, Some(media(&q.key)))),
                None => Err(match e {
                    ToolError::InvalidArgs(m) => bad(format!("{m}, or one of the printer's own sizes (printer_papers)")),
                    other => other,
                }),
            }
        }
    }
}

/// The spooler job for `doc_print` to `printer`. It feeds the paper the PDF was laid out on
/// (`media`, from [`paper_choice`], after the driver's `options`) rather than the queue's default
/// paper, which a job naming none got (A4 under Letter sheets on the queue in #3752).
fn spool_job(a: &Args, printer: &str, title: String, duplex: Option<spool::Duplex>, media: Option<(String, String)>) -> Result<spool::Job> {
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
        grayscale: a.opt_bool("grayscale")?.unwrap_or(false),
        title,
        options: options.into_iter().chain(media).collect(),
    })
}

impl Automation {
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

    pub(crate) fn printer_papers(&self, a: &Args) -> Result<Value> {
        let printer = a.str("printer")?;
        let list: Vec<Value> = spool::printer_papers(printer)
            .into_iter()
            .map(|p| json!({ "key": p.key, "label": p.label, "width": p.size.0, "height": p.size.1, "default": p.default }))
            .collect();
        Ok(json!({ "printer": printer, "count": list.len(), "papers": list }))
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
            "fit" => Layout::Size(SizeMode::Fit),
            "actual" => Layout::Size(SizeMode::Actual),
            "shrink" => Layout::Size(SizeMode::Shrink),
            "custom" => Layout::Size(SizeMode::Custom(a.opt_num("scale")?.ok_or_else(|| bad("custom needs scale (percent)"))?)),
            "multiple" => {
                let per = a.opt_int("per_sheet")?.unwrap_or(2).clamp(1, 256) as usize;
                let order = match a.opt_str("order")?.unwrap_or("horizontal") {
                    "horizontal" => PageOrder::Horizontal,
                    "horizontal-reversed" => PageOrder::HorizontalReversed,
                    "vertical" => PageOrder::Vertical,
                    "vertical-reversed" => PageOrder::VerticalReversed,
                    "cut-stack" => PageOrder::CutStack,
                    o => return Err(bad(format!("unknown order {o:?}"))),
                };
                match Layout::multiple(per) {
                    Layout::Multiple { cols, rows, per_sheet, .. } => Layout::Multiple {
                        cols,
                        rows,
                        per_sheet,
                        order,
                        border: a.opt_bool("border")?.unwrap_or(false),
                        auto_rotate: a.opt_bool("auto_rotate")?.unwrap_or(true),
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
            },
            "poster" => Layout::Poster {
                scale: a.opt_num("scale")?.unwrap_or(200.0),
                overlap: a.opt_num("overlap")?.unwrap_or(18.0),
                cut_marks: a.opt_bool("cut_marks")?.unwrap_or(true),
            },
            l => return Err(bad(format!("unknown layout {l:?}"))),
        };
        let duplex = job_duplex(a.opt_str("duplex")?.unwrap_or("off"), &layout)?;
        if a.opt_str("order")? == Some("cut-stack") && !matches!(layout, Layout::Multiple { .. }) {
            return Err(bad("cut-stack order needs layout multiple"));
        }
        let orientation = match a.opt_str("orientation")?.unwrap_or("auto") {
            "auto" => Orientation::Auto,
            "portrait" => Orientation::Portrait,
            "landscape" => Orientation::Landscape,
            o => return Err(bad(format!("unknown orientation {o:?}"))),
        };
        let content = match a.opt_str("comments_forms")?.unwrap_or("document-and-markups") {
            "document" => Content::Document,
            "document-and-markups" => Content::DocumentAndMarkups,
            "document-and-stamps" => Content::DocumentAndStamps,
            "form-fields-only" => Content::FormFieldsOnly,
            c => return Err(bad(format!("unknown comments_forms {c:?}"))),
        };
        // The printer's own paper sizes (its PPD's, on CUPS): a paper may be one of them, by
        // keyword or label. "default" is the default queue's (the job itself still goes to the
        // spooler's default).
        let own = match a.opt_str("printer")? {
            Some("default") => spool::printers().into_iter().find(|p| p.default).map(|p| spool::printer_papers(&p.name)).unwrap_or_default(),
            Some(printer) => spool::printer_papers(printer),
            None => Vec::new(),
        };
        let (paper, media) = paper_choice(a.opt_str("paper")?, &own)?;
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
        let settings = print::Settings { pages, paper, orientation, layout, content, raster, crops: Vec::new() };
        let sizes: Vec<(f64, f64)> = self.doc(a)?.info.pages.iter().map(|p| (p.width as f64, p.height as f64)).collect();
        let sheets = print::layout(&sizes, &settings).map_err(|e| bad(e.to_string()))?.len();
        let bytes = self.session.print_pdf(id, &settings).map_err(failed)?;
        let mut out = json!({ "sheets": sheets, "pages": settings.pages.len(), "bytes": bytes.len() });
        match (a.opt_str("path")?, a.opt_str("printer")?) {
            (Some(p), None) => {
                let path = self.resolve(p, true)?;
                write_atomic(&path, &bytes)?;
                out["path"] = json!(path.to_string_lossy());
            }
            (None, Some(printer)) => {
                let job = spool_job(a, printer, self.doc(a)?.name.clone(), duplex, media)?;
                out["job"] = json!(spool::submit(&bytes, &job).map_err(|e| failed(e.to_string()))?);
            }
            _ => return Err(bad("pass either path (save the print-ready PDF) or printer (a name from printers, or \"default\")")),
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #2707: a booklet sets two-sided itself whatever `duplex` asks; a bad name is still an error.
    #[test]
    fn a_booklet_decides_two_sided_itself() {
        let booklet = |subset| Layout::Booklet { subset, binding: Binding::Left };
        for asked in ["printer", "off", "long-edge", "short-edge"] {
            assert_eq!(job_duplex(asked, &booklet(BookletSubset::BothSides)).ok(), Some(Some(spool::Duplex::ShortEdge)), "{asked}");
            assert_eq!(job_duplex(asked, &booklet(BookletSubset::FrontOnly)).ok(), Some(Some(spool::Duplex::Off)), "{asked}");
            assert_eq!(job_duplex(asked, &booklet(BookletSubset::BackOnly)).ok(), Some(Some(spool::Duplex::Off)), "{asked}");
        }
        assert!(job_duplex("sideways", &booklet(BookletSubset::BothSides)).is_err());
        assert_eq!(job_duplex("long-edge", &Layout::Size(SizeMode::Fit)).ok(), Some(Some(spool::Duplex::LongEdge)), "other layouts keep it");
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

    /// #3752: `doc_print` to a printer sent no paper, so the queue's default paper stood in for
    /// the one the sheets were laid out on (A4 under Letter sheets on the report's queue).
    #[test]
    fn printing_to_a_printer_feeds_the_laid_out_paper() {
        let job = |args: Value, paper_name: Option<&str>| {
            let (index, _) = paper_named(paper_name).unwrap();
            spool_job(&Args(&args), "Office", "a.pdf".into(), None, print::media_option(index)).unwrap()
        };
        let media = |m: &str| ("media".to_string(), m.to_string());
        // US Letter when no paper is asked for, after the driver's own options.
        let letter = job(json!({ "options": { "InputSlot": "Tray3" } }), None);
        assert_eq!(letter.options, [("InputSlot".to_string(), "Tray3".to_string()), media("Letter")]);
        assert_eq!((letter.printer.as_deref(), letter.title.as_str()), (Some("Office"), "a.pdf"));
        // Every paper, by its full name, goes out as its own media name.
        for (i, (name, size)) in PAPERS.iter().enumerate() {
            assert_eq!(paper_named(Some(name)).ok(), Some((i, *size)), "{name}");
            assert_eq!(job(json!({}), Some(name)).options, print::media_option(i).into_iter().collect::<Vec<_>>(), "{name}");
        }
        // Short forms find the same paper.
        assert_eq!(job(json!({}), Some("letter")).options, [media("Letter")]);
        assert_eq!(job(json!({}), Some("ARCH D")).options, [media("na_arch-d_24x36in")]);
        assert!(matches!(paper_named(Some("Quarto")), Err(ToolError::InvalidArgs(m)) if m.contains("US Letter")));
        assert_eq!(spool_job(&Args(&json!({})), "default", String::new(), None, print::media_option(0)).ok().map(|j| j.printer), Some(None));
        assert!(spool_job(&Args(&json!({ "options": { "InputSlot": 3 } })), "Office", String::new(), None, print::media_option(0)).is_err());
    }

    /// `doc_print`'s paper among the printer's own sizes: with none named, the printer's default
    /// when its driver reports one; a standard size by the printer's own keyword for it; one of the
    /// printer's own by key or label.
    #[test]
    fn the_printers_own_sizes_are_offered_to_doc_print() {
        let paper =
            |key: &str, label: &str, size: (f64, f64), default: bool| spool::PrinterPaper { key: key.into(), label: label.into(), size, default };
        let media = |m: &str| Some(("media".to_string(), m.to_string()));
        let label = vec![paper("Letter", "US Letter", (612.0, 792.0), false), paper("w162h90", "2.25 x 1.25 in", (162.0, 90.0), true)];
        assert_eq!(paper_choice(None, &label).unwrap(), ((162.0, 90.0), media("w162h90")), "the printer's default");
        assert_eq!(paper_choice(Some("2.25 x 1.25 in"), &label).unwrap(), ((162.0, 90.0), media("w162h90")), "by label");
        assert_eq!(paper_choice(Some("W162H90"), &label).unwrap(), ((162.0, 90.0), media("w162h90")), "by key");
        assert_eq!(paper_choice(Some("Letter"), &label).unwrap(), ((612.0, 792.0), media("Letter")), "a standard size the printer has");
        // A standard size the printer doesn't list goes out under its CUPS name.
        let (a4, a4_size) = paper_named(Some("A4")).unwrap();
        assert_eq!(paper_choice(Some("A4"), &label).unwrap(), (a4_size, print::media_option(a4)));
        // Without the printer's sizes (Save as PDF, no PPD): US Letter, as before.
        assert_eq!(paper_choice(None, &[]).unwrap(), (PAPERS[0].1, print::media_option(0)));
        let unknown = paper_choice(Some("Quarto"), &label).unwrap_err().to_string();
        assert!(unknown.contains("US Letter") && unknown.contains("printer_papers"), "{unknown}");
    }
}
