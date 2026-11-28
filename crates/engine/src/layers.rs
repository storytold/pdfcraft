//! Layer (optional content) visibility: what the view shows and what printing writes.

use std::sync::Arc;

use pdfcraft_render::{Layer, LayerOp, RenderPool};

use crate::{Document, render_threads};

/// Most usage application dictionaries (`/AS` entries) that [`print_layer_states`] reads, and
/// most category names and groups it reads in all of them together (entries can share one
/// array, so the count is over every entry, not each).
const MAX_AUTO_STATES: usize = 1024;
const MAX_AUTO_ITEMS: usize = 65_536;

/// The usage dictionary entry and its state key that a `/Category` name of a usage application
/// dictionary reads when printing (ISO 32000-2 Table 101): Print, View and Export each name a
/// state of the group's own. Zoom, User and Language depend on the magnification, the user and
/// the system language, which printing doesn't fix, so they give no recommendation here.
fn print_category(name: &[u8]) -> Option<(&'static [u8], &'static [u8])> {
    match name {
        b"Print" => Some((b"Print", b"PrintState")),
        b"View" => Some((b"View", b"ViewState")),
        b"Export" => Some((b"Export", b"ExportState")),
        _ => None,
    }
}

/// The layers as they print: `layers` as shown, then the default configuration's auto state for
/// the Print event (ISO 32000-2 §8.11.4.4, Table 101). Each usage application dictionary
/// (`/AS` entry) whose `/Event` is `/Print` recommends, for each group in its `/OCGs` and each of
/// its categories ([`print_category`]), the state the group's usage dictionary gives (for the
/// Print category, `/Usage /Print /PrintState`). A group with recommendations is on only if all
/// of them, in all the entries naming it, are ON; other groups keep their state.
///
/// Precedence: a layer the user explicitly toggled away from its initial visibility
/// (`l.visible != l.initial`, via the Layers panel, a `SetOCGState` action, or script) keeps its
/// user-chosen state even if a Print auto-state entry recommends otherwise; untouched layers
/// (`l.visible == l.initial`) follow the Print auto state. `/Category` is an array; a single name
/// is read as one. `None` when no `/AS` entry is for printing.
pub(crate) fn print_layer_states(cos: &pdfcraft_cos::Document, layers: &[Layer]) -> Option<Vec<Layer>> {
    use pdfcraft_cos::Object;
    let root = cos.trailer().get(b"Root").map(|r| cos.resolve(r))?;
    let properties = root.as_dict()?.get(b"OCProperties").map(|p| cos.resolve(p))?;
    let config = properties.as_dict()?.get(b"D").map(|d| cos.resolve(d))?;
    let auto = config.as_dict()?.get(b"AS").map(|a| cos.resolve(a))?;
    let mut printing = false;
    let mut recommended: std::collections::HashMap<(u32, u16), bool> = std::collections::HashMap::new();
    let mut items_left = MAX_AUTO_ITEMS;
    for entry in auto.as_array()?.iter().take(MAX_AUTO_STATES) {
        let entry = cos.resolve(entry);
        let Some(entry) = entry.as_dict() else { continue };
        if entry.get(b"Event").map(|e| cos.resolve(e)).as_deref().and_then(Object::as_name) != Some(b"Print") {
            continue;
        }
        printing = true;
        let categories = entry.get(b"Category").map(|c| cos.resolve(c));
        let mut read: Vec<(&[u8], &[u8])> = Vec::new();
        let mut consider = |name: Option<&[u8]>| {
            if let Some(category) = name.and_then(print_category).filter(|c| !read.contains(c)) {
                read.push(category);
            }
        };
        match categories.as_deref() {
            Some(Object::Array(names)) => {
                let count = names.len().min(items_left);
                items_left -= count;
                names.iter().take(count).for_each(|n| consider(cos.resolve(n).as_name()));
            }
            Some(Object::Name(n)) => consider(Some(n.as_slice())),
            _ => {}
        }
        if read.is_empty() {
            continue;
        }
        let Some(groups) = entry.get(b"OCGs").map(|g| cos.resolve(g)) else { continue };
        for group in groups.as_array().into_iter().flatten() {
            let Some(left) = items_left.checked_sub(1) else { break };
            items_left = left;
            let Some(id) = group.as_ref() else { continue };
            let ocg = cos.resolve(group);
            let usage = ocg.as_dict().and_then(|g| g.get(b"Usage").map(|u| cos.resolve(u)));
            for (entry_key, state_key) in &read {
                // No state given (a Print category without `/PrintState`): no recommendation, so
                // the group is left as it is.
                let state = usage
                    .as_deref()
                    .and_then(|u| u.as_dict().and_then(|u| u.get(entry_key).map(|e| cos.resolve(e))))
                    .and_then(|e| e.as_dict().and_then(|e| e.get(state_key).map(|s| cos.resolve(s))))
                    .and_then(|s| match s.as_name() {
                        Some(b"ON") => Some(true),
                        Some(b"OFF") => Some(false),
                        _ => None,
                    });
                if let Some(on) = state {
                    let all_on = recommended.entry((id.num, id.generation)).or_insert(true);
                    *all_on = *all_on && on;
                }
            }
        }
    }
    printing.then(|| {
        layers
            .iter()
            .map(|l| {
                let visible = if l.visible != l.initial { l.visible } else { recommended.get(&l.id).copied().unwrap_or(l.visible) };
                Layer { visible, ..l.clone() }
            })
            .collect()
    })
}

/// Strip content belonging to optional-content layers that are OFF in an imposed print-ready
/// PDF's default configuration (`/OCProperties /D`), leaving `/OCProperties` intact.
///
/// Spoolers and RIPs such as macOS CoreGraphics / CUPS (`cgpdftoraster`, `cgpdftops`),
/// Preview.app, and AirPrint / IPP Everywhere printers ignore `/OCProperties` and paint every
/// layer present in the content stream; stripping hidden-layer content ensures hidden layers do
/// not appear on paper. Returns `None` when nothing needed stripping or the copy could not be
/// rewritten cleanly (leaving `/OCProperties /D` as a fallback without failing the print).
pub(crate) fn strip_print_hidden_layers(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(bytes.to_vec())).ok()?;
    let removed = pdfcraft_redact::sanitize::strip_off_layer_content(&mut doc).ok()?;
    if removed == 0 {
        return None;
    }
    pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).ok()
}

/// Write `layers`' visibility into the default optional-content configuration of `cos` (a copy
/// made for printing): `/BaseState /ON`, `/ON` the visible layers, `/OFF` the hidden ones. The
/// states written are final, so what would set them again goes: the auto state (`/AS`), and the
/// layers' own `/Usage`, which some readers apply when they draw (a `/ViewState /ON` turns a layer
/// that prints off back on). The configuration's other entries (`/Order`, `/RBGroups`…) are kept.
/// A document without `/OCProperties` is left alone.
pub(crate) fn set_default_layer_state(cos: &mut pdfcraft_cos::Document, layers: &[Layer]) {
    use pdfcraft_cos::{Dict, ObjRef, Object};
    let Some(root_ref) = cos.trailer().get(b"Root").and_then(Object::as_ref) else { return };
    let Some(mut root) = cos.get(root_ref).as_dict().cloned() else { return };
    let Some(properties) = root.get(b"OCProperties").cloned() else { return };
    let Some(mut properties) = cos.resolve(&properties).as_dict().cloned() else { return };
    let mut config = properties.get(b"D").and_then(|d| cos.resolve(d).as_dict().cloned()).unwrap_or_else(Dict::new);
    let refs =
        |visible: bool| Object::Array(layers.iter().filter(|l| l.visible == visible).map(|l| Object::Ref(ObjRef::new(l.id.0, l.id.1))).collect());
    config.set(b"BaseState".to_vec(), Object::name("ON"));
    config.set(b"ON".to_vec(), refs(true));
    config.set(b"OFF".to_vec(), refs(false));
    config.remove(b"AS");
    properties.set(b"D".to_vec(), Object::Dict(config));
    root.set(b"OCProperties".to_vec(), Object::Dict(properties));
    cos.set(root_ref, Object::Dict(root));
    for layer in layers {
        let id = ObjRef::new(layer.id.0, layer.id.1);
        let Some(mut group) = cos.get(id).as_dict().cloned() else { continue };
        if group.remove(b"Usage").is_some() {
            cos.set(id, Object::Dict(group));
        }
    }
}

/// Render `doc` with the layer visibility in its `info.layers`.
pub(crate) fn use_layer_choices(doc: &mut Document) {
    let overrides: Vec<(i32, i32, bool)> = doc.info.layers.iter().map(|l| (l.id.0 as i32, l.id.1 as i32, l.visible)).collect();
    doc.config.layers = Arc::new(overrides);
    doc.config.print_layers = print_preview_layers(doc.editor.as_ref().map(|e| &e.cos), &doc.info.layers);
    doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), crate::live_scripts::screen_config(doc));
}

/// The layer overrides a print preview draws with (`RenderConfig::print_layers`): the layers as
/// they print ([`print_layer_states`]), so the preview shows the page as it prints. `None` when
/// the document has no auto state for printing (or no structure to read it from).
pub(crate) fn print_preview_layers(cos: Option<&pdfcraft_cos::Document>, layers: &[Layer]) -> Option<Arc<Vec<(i32, i32, bool)>>> {
    let printed = print_layer_states(cos?, layers)?;
    Some(Arc::new(printed.iter().map(|l| (l.id.0 as i32, l.id.1 as i32, l.visible)).collect()))
}

/// Apply a set-layer-visibility action to `layers`, one change at a time, so a toggle flips the
/// state the changes before it left. Returns whether any layer ended up changed.
pub(crate) fn apply_layer_state(layers: &mut [Layer], groups: &[Vec<(u32, u16)>], changes: &[(LayerOp, (u32, u16))], preserve_rb: bool) -> bool {
    let before: Vec<bool> = layers.iter().map(|l| l.visible).collect();
    let index: std::collections::HashMap<(u32, u16), usize> = layers.iter().enumerate().map(|(i, l)| (l.id, i)).collect();
    for &(op, ocg) in changes {
        let Some(layer) = index.get(&ocg).and_then(|&i| layers.get_mut(i)) else { continue };
        let on = match op {
            LayerOp::On => true,
            LayerOp::Off => false,
            LayerOp::Toggle => !layer.visible,
        };
        layer.visible = on;
        // Turning a layer off leaves the rest of its groups alone.
        if on && preserve_rb {
            for other in groups.iter().filter(|g| g.contains(&ocg)).flatten().filter(|&&o| o != ocg) {
                if let Some(l) = index.get(other).and_then(|&i| layers.get_mut(i)) {
                    l.visible = false;
                }
            }
        }
    }
    layers.iter().zip(before).any(|(l, was)| l.visible != was)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfcraft_cos::{Document as CosDoc, Object, SaveOptions, write_full};
    use pdfcraft_render::{PRINT_TAG, PageRenderer, RenderConfig, RenderRequest};

    use crate::{Session, display_config, print};

    /// Build a minimal valid PDF from numbered indirect object bodies (1-based; object 1 is Catalog).
    fn pdf_of(objs: &[&[u8]]) -> Vec<u8> {
        let mut pdf = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::with_capacity(objs.len());
        for (i, body) in objs.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            pdf.extend_from_slice(body);
            pdf.extend_from_slice(b"\nendobj\n");
        }
        let xref = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for off in offsets {
            pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        pdf
    }

    /// Remove `/OCProperties` from `bytes` to simulate a spooler/RIP that ignores PDF layers
    /// (e.g. macOS CoreGraphics `cgpdftoraster`/`cgpdftops`, Preview.app, AirPrint RIPs).
    fn ignoring_oc_properties(bytes: &[u8]) -> Vec<u8> {
        let mut cos = CosDoc::open(Arc::new(bytes.to_vec())).unwrap();
        let root = cos.root().unwrap();
        cos.update_dict(root, |d| {
            d.remove(b"OCProperties");
        })
        .unwrap();
        write_full(&cos, &SaveOptions::default()).unwrap()
    }

    /// Count `(red, green, blue, yellow)` pixels on page 0 of `bytes` rendered with `config` and `tag`.
    fn colour_counts(bytes: Arc<Vec<u8>>, config: RenderConfig, tag: u64) -> (usize, usize, usize, usize) {
        let mut renderer = PageRenderer::new(bytes, config);
        let page = renderer.render(RenderRequest { page: 0, scale: 1.0, tag, ..Default::default() });
        assert_eq!(page.error, None);
        let px = page.rgba.as_chunks::<4>().0;
        let red = px.iter().filter(|p| p[0] > 200 && p[1] < 60 && p[2] < 60).count();
        let green = px.iter().filter(|p| p[1] > 200 && p[0] < 60 && p[2] < 60).count();
        let blue = px.iter().filter(|p| p[2] > 200 && p[0] < 60 && p[1] < 60).count();
        let yellow = px.iter().filter(|p| p[0] > 200 && p[1] > 200 && p[2] < 60).count();
        (red, green, blue, yellow)
    }

    /// Names of OCGs listed in `/OCProperties /D /OFF` of `bytes`.
    fn off_layer_names(bytes: &[u8]) -> Vec<String> {
        let cos = CosDoc::open(Arc::new(bytes.to_vec())).unwrap();
        let root = cos.get(cos.root().unwrap());
        let props = cos.resolve(root.as_dict().unwrap().get(b"OCProperties").unwrap());
        let d = cos.resolve(props.as_dict().unwrap().get(b"D").unwrap());
        let Some(off) = d.as_dict().and_then(|d| d.get(b"OFF")).map(|o| cos.resolve(o)) else {
            return Vec::new();
        };
        off.as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| cos.resolve(r).as_dict().and_then(|g| g.get(b"Name")).and_then(Object::as_string).map(|s| s.to_text()))
            .collect()
    }

    /// #8226: hiding a layer via the Layers panel API (`set_layer_visible`) must exclude its
    /// marked content, `/OC` Form XObjects, `/OC` annotations, and `/OCMD` (`/VE`) content from
    /// both vector print (even when the spooler/RIP ignores `/OCProperties`, as macOS CUPS /
    /// CoreGraphics does) and Print as image, while keeping the visible layer present.
    #[test]
    fn hidden_layer_content_is_absent_from_printed_pdf_even_when_oc_properties_are_ignored() {
        let content = b"/OC /S BDC 1 0 0 rg 10 10 40 40 re f EMC \
                        /OC /M BDC 0 1 0 rg 60 10 40 40 re f EMC \
                        /OC /Both BDC 0 0 1 rg 110 10 40 40 re f EMC \
                        /FmS Do";
        let form_s = b"1 0 0 rg 10 60 40 30 re f";
        let annot_ap = b"1 0 0 rg 0 0 40 30 re f";
        let content_obj = [format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(), content, b"\nendstream"].concat();
        let form_obj = [
            format!("<< /Type /XObject /Subtype /Form /BBox [0 0 200 100] /OC 5 0 R /Length {} >>\nstream\n", form_s.len()).as_bytes(),
            form_s,
            b"\nendstream",
        ]
        .concat();
        let ap_obj = [
            format!("<< /Type /XObject /Subtype /Form /BBox [0 0 40 30] /Length {} >>\nstream\n", annot_ap.len()).as_bytes(),
            annot_ap,
            b"\nendstream",
        ]
        .concat();
        let pdf = pdf_of(&[
            b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R] /D << /Order [5 0 R 6 0 R] >> >> >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Annots [10 0 R] \
              /Resources << /Properties << /S 5 0 R /M 6 0 R /Both 7 0 R >> /XObject << /FmS 8 0 R >> >> >>",
            &content_obj,
            b"<< /Type /OCG /Name (Size S) >>",
            b"<< /Type /OCG /Name (Size M) >>",
            b"<< /Type /OCMD /VE [/And 5 0 R 6 0 R] >>",
            &form_obj,
            &ap_obj,
            b"<< /Type /Annot /Subtype /Square /F 4 /Rect [110 60 150 90] /OC 5 0 R /AP << /N 9 0 R >> >>",
        ]);

        let mut s = Session::new();
        let id = s.open("pattern.pdf", None, Arc::new(pdf), None).unwrap();
        let size_s = s.get(id).unwrap().info.layers.iter().position(|l| l.name == "Size S").unwrap();
        assert!(s.set_layer_visible(id, size_s, false));

        // Vector print (default Size layout and Booklet/Multiple imposition).
        for layout in [print::Layout::Size(print::SizeMode::Actual), print::Layout::multiple(2)] {
            let settings = print::Settings { pages: vec![0], paper: (200.0, 100.0), layout, ..Default::default() };
            let printed = s.print_pdf(id, &settings).unwrap();
            assert_eq!(off_layer_names(&printed), vec!["Size S"]);

            // Even with `/OCProperties` stripped (simulating macOS CoreGraphics/CUPS), the hidden
            // layer's marked content, Form XObject, annotation, and OCMD content are gone.
            let no_oc = ignoring_oc_properties(&printed);
            let (red, green, blue, _) = colour_counts(Arc::new(no_oc), RenderConfig::default(), 0);
            assert_eq!(red, 0, "hidden layer Size S (marked content + Form + annot) must not print");
            assert_eq!(blue, 0, "OCMD [/And SizeS SizeM] must not print when Size S is hidden");
            assert!(green > 100, "visible layer Size M must still print: {green} green");
        }

        // Print as image (raster path).
        let raster_settings = print::Settings {
            pages: vec![0],
            paper: (200.0, 100.0),
            layout: print::Layout::Size(print::SizeMode::Actual),
            raster: Some(print::Raster { dpi: 72.0, grayscale: false, colour: None }),
            ..Default::default()
        };
        let raster_pdf = s.print_pdf(id, &raster_settings).unwrap();
        let (red, green, blue, _) = colour_counts(Arc::new(raster_pdf), RenderConfig::default(), 0);
        assert_eq!(red, 0, "hidden layer must be absent from Print as image");
        assert_eq!(blue, 0, "OCMD must be absent from Print as image");
        assert!(green > 100, "visible layer must be present in Print as image: {green} green");
    }

    /// #8226: an explicit user hide in the Layers panel wins over a `/AS` `/Event /Print` entry
    /// recommending `/PrintState /ON`, while untouched layers still follow the document's Print
    /// auto state (ISO 32000-2 §8.11.4.4).
    #[test]
    fn explicit_user_hide_wins_over_print_auto_state_on() {
        let content = b"/OC /S BDC 1 0 0 rg 10 10 40 80 re f EMC \
                        /OC /M BDC 0 1 0 rg 60 10 40 80 re f EMC \
                        /OC /P BDC 0 0 1 rg 110 10 40 80 re f EMC \
                        /OC /V BDC 1 1 0 rg 160 10 40 80 re f EMC";
        let content_obj = [format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(), content, b"\nendstream"].concat();
        let pdf = pdf_of(&[
            b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R 7 0 R 8 0 R] \
              /D << /OFF [7 0 R] /Order [5 0 R 6 0 R 7 0 R 8 0 R] \
              /AS [<< /Event /Print /OCGs [5 0 R 6 0 R 7 0 R 8 0 R] /Category [/Print] >>] >> >> >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 210 100] /Contents 4 0 R \
              /Resources << /Properties << /S 5 0 R /M 6 0 R /P 7 0 R /V 8 0 R >> >> >>",
            &content_obj,
            b"<< /Type /OCG /Name (Size S) /Usage << /Print << /PrintState /ON >> >> >>",
            b"<< /Type /OCG /Name (Size M) /Usage << /Print << /PrintState /ON >> >> >>",
            b"<< /Type /OCG /Name (Print only) /Usage << /Print << /PrintState /ON >> >> >>",
            b"<< /Type /OCG /Name (View only) /Usage << /Print << /PrintState /OFF >> >> >>",
        ]);

        let mut s = Session::new();
        let id = s.open("pattern-as.pdf", None, Arc::new(pdf), None).unwrap();
        let size_s = s.get(id).unwrap().info.layers.iter().position(|l| l.name == "Size S").unwrap();
        assert!(s.set_layer_visible(id, size_s, false));

        // Print preview matches: user-hidden Size S and untouched View only are absent;
        // Size M and untouched Print only are present.
        let doc = s.get(id).unwrap();
        let (red, green, blue, yellow) = colour_counts(doc.display.clone(), display_config(&doc.config), PRINT_TAG);
        assert_eq!((red, yellow), (0, 0), "preview must hide user-hidden Size S and View only");
        assert!(green > 100 && blue > 100, "preview must show Size M ({green}) and Print only ({blue})");

        // Vector print (checked with `/OCProperties` stripped) and Print as image both agree.
        for raster in [None, Some(print::Raster { dpi: 72.0, grayscale: false, colour: None })] {
            let settings = print::Settings {
                pages: vec![0],
                paper: (210.0, 100.0),
                layout: print::Layout::Size(print::SizeMode::Actual),
                raster: raster.clone(),
                ..Default::default()
            };
            let printed = s.print_pdf(id, &settings).unwrap();
            let check_bytes = if raster.is_none() {
                assert_eq!(off_layer_names(&printed), vec!["Size S", "View only"]);
                ignoring_oc_properties(&printed)
            } else {
                printed
            };
            let (red, green, blue, yellow) = colour_counts(Arc::new(check_bytes), RenderConfig::default(), 0);
            assert_eq!(red, 0, "user-hidden Size S must stay hidden even with /PrintState /ON (raster={})", raster.is_some());
            assert_eq!(yellow, 0, "untouched View only must follow /PrintState /OFF (raster={})", raster.is_some());
            assert!(green > 100, "Size M must print (raster={}): {green}", raster.is_some());
            assert!(blue > 100, "untouched Print only must follow /PrintState /ON (raster={}): {blue}", raster.is_some());
        }

        // Resetting layers restores Size S so it prints again.
        assert!(s.reset_layers(id));
        let printed = s
            .print_pdf(
                id,
                &print::Settings {
                    pages: vec![0],
                    paper: (210.0, 100.0),
                    layout: print::Layout::Size(print::SizeMode::Actual),
                    ..Default::default()
                },
            )
            .unwrap();
        let (red, green, blue, yellow) = colour_counts(Arc::new(ignoring_oc_properties(&printed)), RenderConfig::default(), 0);
        assert!(red > 100 && green > 100 && blue > 100 && yellow == 0, "after reset: ({red}, {green}, {blue}, {yellow})");
    }

    /// Stripping hidden layers for print must not apply `/AS` `/Event /View`: an untouched layer
    /// hidden on screen only by `/ViewState /OFF` keeps its `/D` (ON) state on paper.
    #[test]
    fn view_only_auto_state_layer_is_not_stripped_when_printing() {
        let content = b"/OC /L1 BDC 1 0 0 rg 10 10 80 80 re f EMC";
        let content_obj = [format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(), content, b"\nendstream"].concat();
        let pdf = pdf_of(&[
            b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R] \
              /D << /Order [5 0 R] /AS [<< /Event /View /OCGs [5 0 R] /Category [/View] >>] >> >> >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /Properties << /L1 5 0 R >> >> >>",
            &content_obj,
            b"<< /Type /OCG /Name (Screen hidden) /Usage << /View << /ViewState /OFF >> >> >>",
        ]);
        let mut s = Session::new();
        let id = s.open("view-auto.pdf", None, Arc::new(pdf), None).unwrap();
        let printed = s
            .print_pdf(
                id,
                &print::Settings {
                    pages: vec![0],
                    paper: (100.0, 100.0),
                    layout: print::Layout::Size(print::SizeMode::Actual),
                    ..Default::default()
                },
            )
            .unwrap();
        let (red, _, _, _) = colour_counts(Arc::new(ignoring_oc_properties(&printed)), RenderConfig::default(), 0);
        assert!(red > 100, "untouched layer hidden only by /Event /View must still print: {red} red");
    }
}
