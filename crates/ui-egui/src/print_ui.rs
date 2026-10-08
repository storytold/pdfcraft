//! The Print dialog (Acrobat's File ▸ Print, execution plan M10.5): printer, copies, grayscale;
//! pages to print (all, current, range with labels; odd/even, reverse); page sizing & handling
//! (Size, Poster, Multiple, Booklet); orientation; comments & forms; and a live preview of the
//! sheets. Printing sends the print-ready PDF to the system spooler; "Save as PDF" writes it.

use egui::{Color32, Pos2, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::print::{self, Binding, BookletSubset, Content, Layout, Orientation, PAPERS, PageOrder, SizeMode, Subset, spool};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    All,
    Current,
    Range,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handling {
    Size,
    Poster,
    Multiple,
    Booklet,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PrintDraft {
    pub printers: Vec<spool::Printer>,
    /// `None` = Save as PDF.
    pub printer: Option<String>,
    pub copies: u32,
    pub collate: bool,
    pub grayscale: bool,
    pub duplex: spool::Duplex,
    pub which: Which,
    pub range: String,
    pub subset: Subset,
    pub reverse: bool,
    pub handling: Handling,
    pub size: SizeMode,
    pub custom_scale: f64,
    pub per_sheet: usize,
    pub order: PageOrder,
    pub border: bool,
    pub auto_rotate: bool,
    pub booklet_subset: BookletSubset,
    pub binding: Binding,
    pub poster_scale: f64,
    pub overlap: f64,
    pub cut_marks: bool,
    pub orientation: Orientation,
    pub content: Content,
    pub paper: usize,
    /// The sheet shown in the preview (0-based).
    pub sheet: usize,
    pub current_page: usize,
}

impl Default for PrintDraft {
    fn default() -> Self {
        PrintDraft {
            printers: Vec::new(),
            printer: None,
            copies: 1,
            collate: true,
            grayscale: false,
            duplex: spool::Duplex::Off,
            which: Which::All,
            range: String::new(),
            subset: Subset::All,
            reverse: false,
            handling: Handling::Size,
            size: SizeMode::Fit,
            custom_scale: 100.0,
            per_sheet: 2,
            order: PageOrder::Horizontal,
            border: false,
            auto_rotate: true,
            booklet_subset: BookletSubset::BothSides,
            binding: Binding::Left,
            poster_scale: 200.0,
            overlap: 18.0,
            cut_marks: true,
            orientation: Orientation::Auto,
            content: Content::DocumentAndMarkups,
            paper: 0,
            sheet: 0,
            current_page: 0,
        }
    }
}

impl PrintDraft {
    /// The engine settings for this draft (page count and labels from the document).
    pub fn settings(&self, count: usize, labels: &[String]) -> Result<print::Settings, String> {
        let range = match self.which {
            Which::All => None,
            Which::Current => Some((self.current_page + 1).to_string()),
            Which::Range => Some(self.range.clone()),
        };
        let pages = print::select_pages(count, range.as_deref(), labels, self.subset, self.reverse).map_err(|e| e.to_string())?;
        let layout = match self.handling {
            Handling::Size => Layout::Size(match self.size {
                SizeMode::Custom(_) => SizeMode::Custom(self.custom_scale),
                m => m,
            }),
            Handling::Multiple => match Layout::multiple(self.per_sheet) {
                Layout::Multiple { cols, rows, .. } => {
                    Layout::Multiple { cols, rows, order: self.order, border: self.border, auto_rotate: self.auto_rotate }
                }
                other => other,
            },
            Handling::Booklet => Layout::Booklet { subset: self.booklet_subset, binding: self.binding },
            Handling::Poster => Layout::Poster { scale: self.poster_scale, overlap: self.overlap, cut_marks: self.cut_marks },
        };
        Ok(print::Settings { pages, paper: PAPERS[self.paper.min(PAPERS.len() - 1)].1, orientation: self.orientation, layout, content: self.content })
    }

    pub fn job(&self, title: &str) -> spool::Job {
        spool::Job {
            printer: self.printer.clone(),
            copies: self.copies.max(1),
            collate: self.collate,
            duplex: self.duplex,
            grayscale: self.grayscale,
            title: title.to_string(),
        }
    }
}

impl PdfCraftApp {
    pub fn open_print(&mut self) {
        let Some((i, _)) = self.active_ids() else { return };
        let printers = spool::printers();
        let default = printers.iter().find(|p| p.default).or(printers.first()).map(|p| p.name.clone());
        let current = self.views[i].current;
        let keep = std::mem::take(&mut self.print_draft);
        self.print_draft = PrintDraft { printers, printer: default, current_page: current, sheet: 0, ..keep };
        self.dialog = Some(crate::Dialog::Print);
    }

    /// Print (or save) with the dialog's settings. Returns `true` on success.
    pub fn print_now(&mut self) -> bool {
        let Some((_, id)) = self.active_ids() else { return false };
        let Some(doc) = self.session.get(id) else { return false };
        let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
        let settings = match self.print_draft.settings(doc.info.pages.len(), &labels) {
            Ok(s) => s,
            Err(e) => {
                self.notify(e);
                return false;
            }
        };
        let name = doc.name.clone();
        let bytes = match self.session.print_pdf(id, &settings) {
            Ok(b) => b,
            Err(e) => {
                self.notify(e.to_string());
                return false;
            }
        };
        match self.print_draft.printer.clone() {
            Some(printer) => match spool::submit(&bytes, &self.print_draft.job(&name)) {
                Ok(msg) => {
                    self.notify(if msg.is_empty() { format!("Sent to {printer}") } else { format!("Sent to {printer}: {msg}") });
                    true
                }
                Err(e) => {
                    self.notify(e.to_string());
                    false
                }
            },
            None => {
                let path = match self.save_override.clone() {
                    Some(p) => Some(std::path::PathBuf::from(p)),
                    #[cfg(not(target_arch = "wasm32"))]
                    None => {
                        let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF");
                        rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(format!("{stem} (print).pdf")).save_file()
                    }
                    #[cfg(target_arch = "wasm32")]
                    None => None,
                };
                let Some(path) = path else { return false };
                match std::fs::write(&path, &bytes) {
                    Ok(()) => {
                        self.notify(format!("Saved the print-ready PDF to {}", path.display()));
                        true
                    }
                    Err(e) => {
                        self.notify(format!("Could not save: {e}"));
                        false
                    }
                }
            }
        }
    }
}

fn combo<T: PartialEq + Copy>(ui: &mut egui::Ui, id: &str, value: &mut T, choices: &[(T, &str)], width: f32) {
    let shown = choices.iter().find(|c| c.0 == *value).map_or("", |c| c.1);
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(width).show_ui(ui, |ui| {
        for (v, label) in choices {
            ui.selectable_value(value, *v, *label);
        }
    });
}

/// Draw the dialog. `thumb` gives a page's thumbnail texture when there is one. Returns
/// (print, cancel).
pub(crate) fn body(
    ui: &mut egui::Ui,
    d: &mut PrintDraft,
    t: &Tokens,
    sizes: &[(f64, f64)],
    labels: &[String],
    thumb: &dyn Fn(usize) -> Option<egui::TextureId>,
) -> (bool, bool) {
    ui.set_width(820.0);
    ui.label(egui::RichText::new("Print").font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let settings = d.settings(sizes.len(), labels);
    let sheets = settings.as_ref().ok().and_then(|s| print::layout(sizes, s).ok()).unwrap_or_default();
    d.sheet = d.sheet.min(sheets.len().saturating_sub(1));
    ui.horizontal_top(|ui| {
        // Settings.
        ui.vertical(|ui| {
            ui.set_width(470.0);
            egui::Grid::new("print-top").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                ui.label("Printer:");
                let shown = d.printer.clone().unwrap_or_else(|| "Save as PDF".into());
                egui::ComboBox::from_id_salt("printer").selected_text(shown).width(260.0).show_ui(ui, |ui| {
                    for p in &d.printers {
                        let label = if p.default { format!("{} (default)", p.name) } else { p.name.clone() };
                        ui.selectable_value(&mut d.printer, Some(p.name.clone()), label);
                    }
                    ui.selectable_value(&mut d.printer, None, "Save as PDF");
                });
                ui.end_row();
                ui.label("Copies:");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut d.copies).range(1..=999));
                    ui.checkbox(&mut d.collate, "Collate");
                    ui.checkbox(&mut d.grayscale, "Print in grayscale");
                });
                ui.end_row();
                ui.label("Two-sided:");
                combo(
                    ui,
                    "duplex",
                    &mut d.duplex,
                    &[(spool::Duplex::Off, "Off"), (spool::Duplex::LongEdge, "Flip on long edge"), (spool::Duplex::ShortEdge, "Flip on short edge")],
                    160.0,
                );
                ui.end_row();
                ui.label("Paper:");
                egui::ComboBox::from_id_salt("paper").selected_text(PAPERS[d.paper].0).width(160.0).show_ui(ui, |ui| {
                    for (i, (name, _)) in PAPERS.iter().enumerate() {
                        ui.selectable_value(&mut d.paper, i, *name);
                    }
                });
                ui.end_row();
            });
            ui.add_space(6.0);
            widgets::section_title(ui, "Pages to Print");
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.which, Which::All, "All");
                ui.radio_value(&mut d.which, Which::Current, "Current page");
                ui.radio_value(&mut d.which, Which::Range, "Pages");
                let r = ui.add_enabled(
                    d.which == Which::Range,
                    egui::TextEdit::singleline(&mut d.range).hint_text(format!("1-{}", sizes.len())).desired_width(110.0),
                );
                if r.gained_focus() {
                    d.which = Which::Range;
                }
            });
            ui.horizontal(|ui| {
                ui.label("More options:");
                combo(
                    ui,
                    "subset",
                    &mut d.subset,
                    &[(Subset::All, "All pages in range"), (Subset::Odd, "Odd pages only"), (Subset::Even, "Even pages only")],
                    150.0,
                );
                ui.checkbox(&mut d.reverse, "Reverse pages");
            });
            ui.add_space(6.0);
            widgets::section_title(ui, "Page Sizing & Handling");
            ui.horizontal(|ui| {
                for (h, label) in
                    [(Handling::Size, "Size"), (Handling::Poster, "Poster"), (Handling::Multiple, "Multiple"), (Handling::Booklet, "Booklet")]
                {
                    if widgets::mode_tab(ui, label, d.handling == h).clicked() {
                        d.handling = h;
                        d.sheet = 0;
                    }
                }
            });
            ui.add_space(4.0);
            match d.handling {
                Handling::Size => {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut d.size, SizeMode::Fit, "Fit");
                        ui.radio_value(&mut d.size, SizeMode::Actual, "Actual size");
                        ui.radio_value(&mut d.size, SizeMode::Shrink, "Shrink oversized pages");
                    });
                    ui.horizontal(|ui| {
                        let custom = matches!(d.size, SizeMode::Custom(_));
                        if ui.radio(custom, "Custom scale:").clicked() {
                            d.size = SizeMode::Custom(d.custom_scale);
                        }
                        ui.add_enabled(custom, egui::DragValue::new(&mut d.custom_scale).range(1.0..=1000.0).suffix(" %"));
                    });
                }
                Handling::Poster => {
                    ui.horizontal(|ui| {
                        ui.label("Tile scale:");
                        ui.add(egui::DragValue::new(&mut d.poster_scale).range(10.0..=1000.0).suffix(" %"));
                        ui.label("Overlap:");
                        ui.add(egui::DragValue::new(&mut d.overlap).range(0.0..=144.0).suffix(" pt"));
                        ui.checkbox(&mut d.cut_marks, "Cut marks");
                    });
                }
                Handling::Multiple => {
                    ui.horizontal(|ui| {
                        ui.label("Pages per sheet:");
                        combo(ui, "per-sheet", &mut d.per_sheet, &[(2, "2"), (4, "4"), (6, "6"), (9, "9"), (16, "16")], 60.0);
                        ui.label("Page order:");
                        combo(
                            ui,
                            "order",
                            &mut d.order,
                            &[
                                (PageOrder::Horizontal, "Horizontal"),
                                (PageOrder::HorizontalReversed, "Horizontal reversed"),
                                (PageOrder::Vertical, "Vertical"),
                                (PageOrder::VerticalReversed, "Vertical reversed"),
                            ],
                            150.0,
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut d.border, "Print page border");
                        ui.checkbox(&mut d.auto_rotate, "Auto-rotate pages");
                    });
                }
                Handling::Booklet => {
                    ui.horizontal(|ui| {
                        ui.label("Booklet subset:");
                        combo(
                            ui,
                            "booklet",
                            &mut d.booklet_subset,
                            &[
                                (BookletSubset::BothSides, "Both sides"),
                                (BookletSubset::FrontOnly, "Front side only"),
                                (BookletSubset::BackOnly, "Back side only"),
                            ],
                            130.0,
                        );
                        ui.label("Binding:");
                        combo(ui, "binding", &mut d.binding, &[(Binding::Left, "Left"), (Binding::Right, "Right")], 80.0);
                    });
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Orientation:");
                ui.radio_value(&mut d.orientation, Orientation::Auto, "Auto portrait/landscape");
                ui.radio_value(&mut d.orientation, Orientation::Portrait, "Portrait");
                ui.radio_value(&mut d.orientation, Orientation::Landscape, "Landscape");
            });
            ui.add_space(6.0);
            widgets::section_title(ui, "Comments & Forms");
            combo(
                ui,
                "content",
                &mut d.content,
                &[
                    (Content::Document, "Document"),
                    (Content::DocumentAndMarkups, "Document and markups"),
                    (Content::DocumentAndStamps, "Document and stamps"),
                    (Content::FormFieldsOnly, "Form fields only"),
                ],
                220.0,
            );
        });
        ui.add_space(12.0);
        // Preview.
        ui.vertical(|ui| {
            ui.set_width(320.0);
            let (area, _) = ui.allocate_exact_size(vec2(320.0, 380.0), egui::Sense::hover());
            ui.painter().rect_filled(area, 6.0, t.hover);
            match (&settings, sheets.get(d.sheet)) {
                (Err(e), _) => {
                    ui.put(area.shrink(16.0), egui::Label::new(egui::RichText::new(e).color(t.text_muted)).wrap());
                }
                (Ok(_), Some(sheet)) => {
                    let k = ((area.width() - 24.0) / sheet.size.0 as f32).min((area.height() - 24.0) / sheet.size.1 as f32);
                    let paper = Rect::from_center_size(area.center(), vec2(sheet.size.0 as f32 * k, sheet.size.1 as f32 * k));
                    ui.painter().rect_filled(paper, 0.0, Color32::WHITE);
                    ui.painter().rect_stroke(paper, 0.0, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                    let to_screen = |x: f64, y: f64| -> Pos2 { pos2(paper.left() + x as f32 * k, paper.bottom() - y as f32 * k) };
                    let painter = ui.painter().with_clip_rect(paper);
                    for pl in &sheet.placed {
                        let (dw, dh) = sizes[pl.page];
                        let [x0, y0, x1, y1] = pl.clip;
                        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| {
                            let (sx, sy) = pl.matrix.apply(x, y);
                            to_screen(sx, sy)
                        });
                        // UVs of the visible part (texture y runs down from the page top).
                        let uv = |x: f64, y: f64| pos2((x / dw) as f32, (1.0 - y / dh) as f32);
                        let uvs = [uv(x0, y0), uv(x1, y0), uv(x1, y1), uv(x0, y1)];
                        match thumb(pl.page) {
                            Some(tex) => {
                                let mut mesh = egui::Mesh::with_texture(tex);
                                for (p, u) in corners.iter().zip(uvs) {
                                    mesh.vertices.push(egui::epaint::Vertex {
                                        pos: *p,
                                        uv: u,
                                        color: if d.grayscale { Color32::from_gray(235) } else { Color32::WHITE },
                                    });
                                }
                                mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
                                painter.add(egui::Shape::mesh(mesh));
                            }
                            None => {
                                painter.add(egui::Shape::convex_polygon(
                                    corners.to_vec(),
                                    Color32::from_gray(245),
                                    Stroke::new(0.5, Color32::from_gray(180)),
                                ));
                                let c = corners.iter().fold(vec2(0.0, 0.0), |a, p| a + p.to_vec2()) / 4.0;
                                painter.text(
                                    c.to_pos2(),
                                    egui::Align2::CENTER_CENTER,
                                    (pl.page + 1).to_string(),
                                    theme::regular(11.0),
                                    Color32::from_gray(120),
                                );
                            }
                        }
                    }
                    for b in &sheet.borders {
                        painter.rect_stroke(
                            Rect::from_two_pos(to_screen(b[0], b[1]), to_screen(b[2], b[3])),
                            0.0,
                            Stroke::new(0.6, Color32::BLACK),
                            egui::StrokeKind::Middle,
                        );
                    }
                    for l in &sheet.lines {
                        painter.line_segment([to_screen(l[0], l[1]), to_screen(l[2], l[3])], Stroke::new(0.6, Color32::BLACK));
                    }
                }
                _ => {}
            }
            ui.horizontal(|ui| {
                let n = sheets.len();
                if ui.add_enabled(d.sheet > 0, egui::Button::new("‹")).on_hover_text("Previous sheet").clicked() {
                    d.sheet -= 1;
                }
                ui.label(if n == 0 { "No sheets".to_string() } else { format!("Sheet {} of {n}", d.sheet + 1) });
                if ui.add_enabled(d.sheet + 1 < n, egui::Button::new("›")).on_hover_text("Next sheet").clicked() {
                    d.sheet += 1;
                }
            });
            if let Some(s) = sheets.first() {
                let (w, h) = (s.size.0 / 72.0, s.size.1 / 72.0);
                ui.label(egui::RichText::new(format!("{w:.2} × {h:.2} in")).small().color(t.text_muted));
            }
        });
    });
    ui.add_space(12.0);
    let (mut go, mut cancel) = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let label = if d.printer.is_some() { "Print" } else { "Save as PDF" };
        if ui.add_enabled_ui(settings.is_ok() && !sheets.is_empty(), |ui| widgets::pill_button(ui, label, true)).inner.clicked() {
            go = true;
        }
        if widgets::pill_button(ui, "Cancel", false).clicked() {
            cancel = true;
        }
    });
    (go, cancel)
}
