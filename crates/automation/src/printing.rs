//! Printing tools: list printers, and print (or save the print-ready PDF) with Acrobat's Print
//! dialog options.

use pdfcraft_engine::print::{self, Binding, BookletSubset, Content, Layout, Orientation, PAPERS, PageOrder, SizeMode, Subset, spool};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed, write_atomic};

fn bad(m: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(m.into())
}

impl Automation {
    pub(crate) fn printers(&self) -> Result<Value> {
        let list: Vec<Value> = spool::printers().into_iter().map(|p| json!({ "name": p.name, "default": p.default })).collect();
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
                    Layout::Multiple { cols, rows, .. } => Layout::Multiple {
                        cols,
                        rows,
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
        if matches!(layout, Layout::Multiple { order: PageOrder::CutStack, .. }) && a.opt_str("duplex")?.unwrap_or("off") != "off" {
            return Err(bad("cut-and-stack printing needs duplex off (single-sided sheets)"));
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
        let content = match a.opt_str("comments_forms")?.unwrap_or("document-and-markups") {
            "document" => Content::Document,
            "document-and-markups" => Content::DocumentAndMarkups,
            "document-and-stamps" => Content::DocumentAndStamps,
            "form-fields-only" => Content::FormFieldsOnly,
            c => return Err(bad(format!("unknown comments_forms {c:?}"))),
        };
        let paper = match a.opt_str("paper")? {
            None => PAPERS[0].1,
            Some(p) => PAPERS
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(p) || n.replace("US ", "").eq_ignore_ascii_case(p))
                .map(|x| x.1)
                .ok_or_else(|| bad(format!("unknown paper {p:?} (Letter, Legal, Tabloid, A3, A4, A5)")))?,
        };
        let settings = print::Settings { pages, paper, orientation, layout, content };
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
                let name = self.doc(a)?.name.clone();
                let job = spool::Job {
                    printer: if printer == "default" { None } else { Some(printer.to_string()) },
                    copies: a.opt_int("copies")?.unwrap_or(1).clamp(1, 999) as u32,
                    collate: a.opt_bool("collate")?.unwrap_or(true),
                    duplex: match a.opt_str("duplex")?.unwrap_or("off") {
                        "off" => spool::Duplex::Off,
                        "long-edge" => spool::Duplex::LongEdge,
                        "short-edge" => spool::Duplex::ShortEdge,
                        d => return Err(bad(format!("unknown duplex {d:?}"))),
                    },
                    grayscale: a.opt_bool("grayscale")?.unwrap_or(false),
                    title: name,
                    options: match a.get("options") {
                        None => Vec::new(),
                        Some(Value::Object(m)) => m
                            .iter()
                            .map(|(k, v)| match v.as_str() {
                                Some(v) => Ok((k.clone(), v.to_string())),
                                None => Err(bad(format!("options.{k} must be a string (a choice from printer_options)"))),
                            })
                            .collect::<Result<Vec<_>>>()?,
                        Some(_) => return Err(bad("options must be an object of option keys and choices from printer_options")),
                    },
                };
                out["job"] = json!(spool::submit(&bytes, &job).map_err(|e| failed(e.to_string()))?);
            }
            _ => return Err(bad("pass either path (save the print-ready PDF) or printer (a name from printers, or \"default\")")),
        }
        Ok(out)
    }
}
