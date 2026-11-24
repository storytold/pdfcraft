//! Export form data and Export comments: the format is chosen in PdfCraft before the save panel
//! opens. The macOS save panel shows no list of file types, so a choice offered only there could
//! not be found (#5215).

use craft::ui::contract::Bind;
use craft::ui::dialog::{DialogResult, dialog_id};
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, Param, ParamDialog, ParamKind};
use craft::ui::runtime::env::Label;
use pdfcraft_engine::DataFormat;

use crate::PdfCraftApp;

/// The dialog's one row: the format, by its extension.
const FORMAT: &str = "format";

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

/// The extensions the save panel allows when saving as `format`: `format`'s first, then every
/// other format in `offered`. On Windows the first one is the panel's default extension, and a
/// name ending in one the filter doesn't list gets the default added (`data.csv.xfdf`), so every
/// offered extension is listed, letting a name changed to another format's extension keep it;
/// the macOS panel would warn about one it doesn't list.
pub(crate) fn save_extensions(format: DataFormat, offered: &[DataFormat]) -> Vec<&'static str> {
    std::iter::once(format).chain(offered.iter().copied().filter(|f| *f != format)).map(extension).collect()
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

impl DataKind {
    const ALL: [DataKind; 2] = [DataKind::Comments, DataKind::FormData];

    /// The dialog's key: the command that opens it. Each kind has its own so a result says which it answers.
    fn key(self) -> &'static str {
        match self {
            DataKind::Comments => "comment.export",
            DataKind::FormData => "form.export_data",
        }
    }
}

/// `format` as the Format list shows it: its name and extension.
fn label(format: DataFormat) -> String {
    format!("{} (.{})", name(format), extension(format))
}

/// Opens the dialog for `kind` over the active document: the Format list, then Export and Cancel.
pub(crate) fn open(app: &mut PdfCraftApp, ctx: &egui::Context, kind: DataKind) {
    let offered = formats(kind, active_is_xfa(app));
    let Some(format) = chosen(app, kind) else { return };
    let fields = DialogFields::from_pairs([(FORMAT, extension(format))]);
    let dialog = HostedParamDialog::new(ctx, kind.key(), fields, move |fields| {
        let options = offered.iter().map(|f| (extension(*f).to_string(), Label::from(label(*f)))).collect();
        let title = match kind {
            DataKind::Comments => tl!("Export comments"),
            DataKind::FormData => tl!("Export form data"),
        };
        ParamDialog::command(kind.key(), title, vec![Param::new(FORMAT, tl!("Format"), ParamKind::choice(options, None))])
            .values(Bind::bound(fields))
            .ok_label(tl!("Export").into())
    });
    crate::dialogs::host::open(app, dialog);
}

/// Takes what the dialogs answered this pass: Export remembers the format and asks where to save.
/// Only a choice exported with is remembered: a cancelled dialog keeps the format chosen before.
pub(crate) fn pass(ctx: &egui::Context, app: &mut PdfCraftApp) {
    for kind in DataKind::ALL {
        let Some(DialogResult::Ok(result)) = app.dialogs.take_by_id::<HostedParamDialog>(dialog_id(kind.key())) else { continue };
        let Some(format) = result.fields.text(FORMAT).and_then(DataFormat::from_extension) else { continue };
        app.data_format = Some(format);
        app.export_data_as(ctx, kind, format);
    }
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

    /// #5301 review: the save panel allows every offered format's extension, the chosen one
    /// first, so a name changed to another one (`data.csv` with XFDF chosen) keeps it and is
    /// written in that format, and the chosen one stays the default. Nothing else is allowed.
    #[test]
    fn the_save_panel_allows_every_offered_extension_the_chosen_one_first() {
        for (kind, xfa) in [(DataKind::Comments, false), (DataKind::FormData, false), (DataKind::FormData, true)] {
            let offered = formats(kind, xfa);
            for format in offered {
                let allowed = save_extensions(*format, offered);
                assert_eq!(allowed.first(), Some(&extension(*format)), "{kind:?} {format:?}");
                assert_eq!(allowed.len(), offered.len(), "{kind:?} {format:?}: each once: {allowed:?}");
                for f in offered {
                    assert!(allowed.contains(&extension(*f)), "{kind:?} {format:?} allows {f:?}");
                }
            }
        }
        assert_eq!(save_extensions(DataFormat::Fdf, formats(DataKind::Comments, false)), ["fdf", "xfdf"]);
    }
}
