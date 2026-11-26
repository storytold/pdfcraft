//! Print production ▸ Convert colours (Acrobat's Convert Colors): the target
//! space, what to convert, the pages. Convert counts first and says what changed.

use pdfcraft_engine::{ColourTarget, ConvertColours, DocId, Edit};

use craft::pages::PageRange;
use craft::pages::ui::{PageRangeChoices, PageRangeOptions};
use craft::ui::contract::Bind;
use craft::ui::dialog::dialog_id;
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, Param, ParamDialog, ParamKind};
use craft::ui::runtime::env::Label;

use crate::PdfCraftApp;

/// The dialog's key.
const KEY: &str = "prepress.convert_colors";

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ConvertColoursDraft {
    pub settings: ConvertColours,
    pub range: PageRange,
    /// The document `range` was chosen for.
    pub doc: Option<DocId>,
}

impl ConvertColoursDraft {
    /// A range chosen for another document (pages 4–5 of a long one) means nothing on this one.
    pub fn for_document(&mut self, id: DocId) {
        if self.doc != Some(id) {
            self.range = PageRange::default();
            self.doc = Some(id);
        }
    }

    /// The target's id in the dialog's choice row.
    fn target_id(target: ColourTarget) -> &'static str {
        match target {
            ColourTarget::Gray => "gray",
            ColourTarget::Rgb => "rgb",
            ColourTarget::Cmyk => "cmyk",
        }
    }

    /// Puts the values into the dialog's rows.
    fn write(&self, f: &mut DialogFields) {
        let s = &self.settings;
        f.set("target", Self::target_id(s.target));
        f.set("preserve_black", s.preserve_black);
        f.set("images", s.images);
        f.set("annotations", s.annotations);
        f.set("range", PageRangeOptions::field_value(&self.range));
    }

    /// Takes the dialog's values.
    fn read(&mut self, f: &DialogFields) {
        let s = &mut self.settings;
        s.target = [ColourTarget::Gray, ColourTarget::Rgb, ColourTarget::Cmyk]
            .into_iter()
            .find(|t| Some(Self::target_id(*t)) == f.text("target"))
            .unwrap_or(s.target);
        s.preserve_black = f.bool("preserve_black").unwrap_or(s.preserve_black);
        s.images = f.bool("images").unwrap_or(s.images);
        s.annotations = f.bool("annotations").unwrap_or(s.annotations);
        if let Some(range) = PageRangeOptions::value(f, "range") {
            self.range = range;
        }
    }
}

/// The rows of the dialog for a document of `count` pages with `current` on screen.
fn params(count: usize, current: usize) -> Vec<Param> {
    let target = vec![
        (ConvertColoursDraft::target_id(ColourTarget::Gray).to_string(), tl!("Grayscale").into()),
        (ConvertColoursDraft::target_id(ColourTarget::Rgb).to_string(), tl!("RGB").into()),
        (ConvertColoursDraft::target_id(ColourTarget::Cmyk).to_string(), tl!("CMYK").into()),
    ];
    vec![
        Param::new("target", tl!("Convert to"), ParamKind::radio(target, Some("gray".to_string()))),
        Param::read_only_text("target_note", "", tl!("By the standard device formulas; profiles are not applied.")).full_width(),
        Param::new("preserve_black", tl!("Preserve black"), ParamKind::bool(true)),
        Param::read_only_text("black_note", "", tl!("pure black stays 100% K or gray 0 instead of a mix")).full_width(),
        Param::new("images", tl!("Pictures too"), ParamKind::bool(true)),
        Param::read_only_text("images_note", "", tl!("re-encoded in the target space; JPEGs stay JPEG")).full_width(),
        Param::new("annotations", tl!("Comments and form fields"), ParamKind::bool(true)),
        Param::read_only_text("annotations_note", "", tl!("their appearances as well as the page")).full_width(),
        PageRangeOptions::param("range", tl!("Pages"), count, current, PageRangeChoices::CurrentFromTo),
    ]
}

impl PdfCraftApp {
    /// Opens Convert Colours over the active document.
    pub(crate) fn open_convert_colours(&mut self, ctx: &egui::Context) {
        let Some((i, id)) = self.active_ids() else { return };
        let count = self.session.get(id).map_or(0, |d| d.info.pages.len());
        let current = self.views[i].current();
        self.convert_colours_draft.for_document(id);
        let mut fields = DialogFields::new();
        self.convert_colours_draft.write(&mut fields);
        let dialog = HostedParamDialog::new(ctx, KEY, fields, move |fields| {
            let ready = PageRangeOptions::value(fields, "range").unwrap_or_default().pages(count, current, &[]);
            ParamDialog::new(KEY, tl!("Convert colours"), params(count, current))
                .values(Bind::bound(fields))
                .ok_label(Label::from(tl!("Convert")))
                .ok_enabled(!ready.is_empty())
        });
        crate::dialogs::host::open(self, dialog);
    }

    /// Converts when the dialog was answered with OK.
    pub(crate) fn convert_colours_pass(&mut self, ctx: &egui::Context) {
        let Some(answer) = self.dialogs.take_by_id::<HostedParamDialog>(dialog_id(KEY)) else { return };
        let Some(result) = answer.ok() else { return };
        self.convert_colours_draft.read(&result.fields);
        self.convert_colours(ctx);
    }

    /// Convert: count first (the edit refuses when nothing changes), then apply and say.
    pub(crate) fn convert_colours(&mut self, ctx: &egui::Context) {
        let Some((i, id)) = self.active_ids() else { return };
        let count = self.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
        let pages = self.convert_colours_draft.range.pages(count, self.views[i].current(), &[]);
        let settings = self.convert_colours_draft.settings.clone();
        let found = self.session.count_colours(id, &pages, &settings);
        if found.colours == 0 && found.images == 0 {
            self.notify_tr("Nothing on those pages needed converting");
            return;
        }
        if self.apply_edit(ctx, Edit::ConvertColours { pages, settings }) {
            self.notify_fmt(
                "Converted {c} colours and {i} pictures on {p} pages",
                &[("c", &found.colours.to_string()), ("i", &found.images.to_string()), ("p", &found.pages.to_string())],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft::pages::PageScope;

    #[test]
    fn a_page_range_does_not_follow_the_user_to_another_document() {
        let (a, b) = (DocId(1), DocId(2));
        let mut d = ConvertColoursDraft::default();
        d.for_document(a);
        let chosen = PageRange { scope: PageScope::Range, from: 4, to: Some(5), ..PageRange::default() };
        d.range = chosen;
        d.for_document(a);
        assert_eq!(d.range, chosen, "same document keeps its range");
        d.for_document(b);
        assert_eq!(d.range, PageRange::default());
        assert_eq!(d.range.pages(1, 0, &[]), vec![0]);
    }
}
