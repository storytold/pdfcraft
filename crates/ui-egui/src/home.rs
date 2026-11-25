//! Home tab: recent files and pinned folders (local only, never another app's list), on the
//! framework's start screen.

use craft::theme::TypeRole;
use craft::ui::contract::Show as _;
use craft::ui::display::Text;
#[cfg(not(target_arch = "wasm32"))]
use craft::ui::runtime::env::Env;
use craft::ui::shell::{RecentFile, StartAction, StartPlace, StartScreen};

use crate::{PdfCraftApp, panels::human_size};

pub mod folders_ui;

/// Draw the Home page: the welcome text, recent files and pinned folders.
pub fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    #[cfg(not(target_arch = "wasm32"))]
    app.refresh_pinned(ui.input(|i| i.time));

    let files: Vec<RecentFile> = app
        .recent
        .iter()
        .map(|r| {
            let pages = crate::i18n::trn(crate::i18n::current(), r.pages as u64, "{n} page", "{n} pages");
            RecentFile::new(r.path.as_str(), r.name.as_str()).note(format!("{pages} · {}", human_size(r.size)))
        })
        .collect();
    #[cfg(not(target_arch = "wasm32"))]
    let pinned_folders = app.pinned_folders(&Env::of(ui));

    let (mut view, mut sort) = (app.recent_view, app.recent_sort);
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut screen = StartScreen::new(craft::ui::contract::Bind::bound(&mut view), craft::ui::contract::Bind::bound(&mut sort), &files)
        .max_recent(app.recent_limit)
        // Heading-strength section titles (Clear recent beside Recent) and the pinned folders above
        // the recent files.
        .heading_sections()
        .pinned_first()
        .offers_new(false)
        // Open is in the title bar.
        .offers_open(false)
        .pinned_title(tl!("Folders").to_string())
        .drop_hint(tl!("Files you open in PdfCraft appear here").to_string())
        .section(StartPlace::BeforeRecent, |ui, _| {
            let _ = Text::new(tl!("Welcome to PdfCraft").to_string()).role(TypeRole::Display).show(ui);
        })
        .section(StartPlace::AfterSections, |ui, _| {
            let _ = Text::new(tl!("Privacy").to_uppercase()).role(TypeRole::Label).show(ui);
            let _ =
                Text::new(tl!("PdfCraft works offline. No telemetry, no account, and no cloud processing unless you add a provider.").to_string())
                    .show(ui);
        });
    #[cfg(not(target_arch = "wasm32"))]
    {
        screen = screen.pinned(&pinned_folders);
    }
    let actions = screen.show(ui).actions;
    app.recent_view = view;
    app.recent_sort = sort;

    for action in actions {
        match action {
            StartAction::Open => app.open_dialog(),
            StartAction::OpenRecent(path) | StartAction::OpenPinnedFile(path) => app.open_recent(&path.to_string_lossy()),
            StartAction::RemoveRecent(path) => {
                let path = path.to_string_lossy();
                app.recent.retain(|r| r.path != path);
            }
            StartAction::ClearRecent => {
                app.execute(ui.ctx(), "file.clear_recent");
            }
            #[cfg(not(target_arch = "wasm32"))]
            StartAction::RevealRecent(path) | StartAction::OpenPinned(path) => crate::menus::tab_context::reveal_on_jobs(app, path),
            StartAction::PinFolder => app.pin_folder_dialog(),
            StartAction::UnpinFolder(path) => app.unpin_folder(&path.to_string_lossy()),
            StartAction::FoldPinned(path) => app.pinned.toggle_collapsed(&path.to_string_lossy()),
            StartAction::ExpandPinned(path) => app.pinned.toggle_expanded(&path.to_string_lossy()),
            _ => {}
        }
    }
}
