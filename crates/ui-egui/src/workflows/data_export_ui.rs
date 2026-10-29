//! Export form data and Export comments: the format is chosen in PdfCraft before the save panel
//! opens. The macOS save panel shows no list of file types, so a choice offered only there could
//! not be found (#5215).

use egui::{Align, Layout};
use pdfcraft_engine::DataFormat;

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// What a data export writes out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataKind {
    /// Export comments: every comment.
    Comments,
    /// Export form data: every field's value.
    FormData,
}

/// The formats offered for `kind`, the default first. An XFA form's data comes first as XML, as
/// Acrobat exports it.
pub(crate) fn formats(kind: DataKind, xfa: bool) -> &'static [DataFormat] {
    use DataFormat::{Csv, Fdf, Txt, Xdp, Xfdf, Xml};
    match (kind, xfa) {
        (DataKind::Comments, _) => &[Xfdf, Fdf],
        (DataKind::FormData, true) => &[Xml, Xdp, Xfdf, Fdf, Csv, Txt],
        (DataKind::FormData, false) => &[Xfdf, Fdf, Xml, Csv, Txt],
    }
}

/// The file extension `format` is saved with, without the dot.
pub(crate) fn extension(format: DataFormat) -> &'static str {
    match format {
        DataFormat::Xfdf => "xfdf",
        DataFormat::Fdf => "fdf",
        DataFormat::Xml => "xml",
        DataFormat::Xdp => "xdp",
        DataFormat::Csv => "csv",
        DataFormat::Txt => "txt",
    }
}

/// `format`'s name as the save panel's filter and the Format list show it.
pub(crate) fn name(format: DataFormat) -> String {
    match format {
        DataFormat::Xfdf => "XFDF".into(),
        DataFormat::Fdf => "FDF".into(),
        DataFormat::Xml => "XML".into(),
        DataFormat::Xdp => "XDP".into(),
        DataFormat::Csv => "CSV".into(),
        DataFormat::Txt => tl!("Text").to_string(),
    }
}

/// The format the dialog shows for `kind`: the last one chosen when it is offered here, else the
/// default.
pub(crate) fn chosen(app: &PdfCraftApp, kind: DataKind) -> Option<DataFormat> {
    let offered = formats(kind, active_is_xfa(app));
    app.data_format.filter(|f| offered.contains(f)).or_else(|| offered.first().copied())
}

fn active_is_xfa(app: &PdfCraftApp) -> bool {
    app.active_ids().and_then(|(_, id)| app.session.get(id)).is_some_and(|d| d.info.xfa.is_some())
}

/// The dialog's body: the Format list, then Export and Cancel.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, _t: &Tokens, kind: DataKind) -> (bool, bool) {
    let offered = formats(kind, active_is_xfa(app));
    let shown = chosen(app, kind);
    let mut current = shown;
    ui.label(
        egui::RichText::new(match kind {
            DataKind::Comments => tl!("Export comments"),
            DataKind::FormData => tl!("Export form data"),
        })
        .font(theme::semibold(18.0)),
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let title = ui.label(tl!("Format"));
        let label = |f: DataFormat| format!("{} (.{})", name(f), extension(f));
        let combo = craft_ui::ComboBox::from_id_salt("export-data-format").selected_text(current.map(label).unwrap_or_default()).show_ui(ui, |ui| {
            for f in offered {
                ui.selectable_value(&mut current, Some(*f), label(*f));
            }
        });
        // The visible label names the list for assistive technology and tests.
        combo.response.labelled_by(title.id);
    });
    // Only a choice the user makes is remembered: drawing another kind's default, then
    // cancelling, keeps the format chosen before.
    if current != shown {
        app.data_format = current;
    }
    ui.add_space(12.0);
    let (mut apply, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.add_enabled_ui(current.is_some(), |ui| widgets::pill_button(ui, tl!("Export"), true)).inner.clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (apply, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every offered format has its own extension, and the extension reads back as that format,
    /// so the file the save panel names is written in the format chosen.
    #[test]
    fn every_offered_format_round_trips_through_its_extension() {
        for (kind, xfa) in [(DataKind::Comments, false), (DataKind::FormData, false), (DataKind::FormData, true)] {
            let offered = formats(kind, xfa);
            assert!(!offered.is_empty(), "{kind:?} {xfa}");
            for f in offered {
                assert_eq!(DataFormat::from_extension(extension(*f)), Some(*f), "{f:?}");
            }
        }
        assert_eq!(formats(DataKind::FormData, true).first(), Some(&DataFormat::Xml), "an XFA form's data is XML first");
        assert_eq!(formats(DataKind::FormData, false).first(), Some(&DataFormat::Xfdf));
        assert!(!formats(DataKind::Comments, false).contains(&DataFormat::Csv), "comments are XFDF or FDF only");
    }
}
