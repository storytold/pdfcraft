//! The app's own icon set: the Lucide documents (ISC licence, see assets/icons/LICENSE-lucide.txt) that the
//! built-in catalogue has no icon for. Names the catalogue serves are drawn by the catalogue, so none of them is
//! listed here (`register_app_icons` refuses a clashing name and only logs it).

use craft::ui::primitives::{AppIcons, register_app_icons};

// PLAN: the writer round's file deletion removed every assets/icons/*.svg; the 83 files listed in `ICONS` and
// circle-check, triangle-alert and circle-x (the source of crates/sign/src/status_layers.rs's path data, entries kept
// in ATTRIBUTION.toml) must be restored from the base (`git checkout <base> -- assets/icons/<name>.svg`), byte for
// byte, so `include_bytes!` and the SHA-256 entries hold. The other 105 stay deleted.

/// One icon document of `assets/icons/`, named as its file.
macro_rules! icon {
    ($name:literal) => {
        ($name, include_bytes!(concat!("../../../assets/icons/", $name, ".svg")) as &[u8])
    };
}

/// The documents, named as the engine's icon strings and `CommentTool::icon` name them.
const ICONS: &[(&str, &[u8])] = &[
    icon!("a-large-small"),
    icon!("accessibility"),
    icon!("arrow-up-right"),
    icon!("badge-check"),
    icon!("bell"),
    icon!("book-open"),
    icon!("bookmark-plus"),
    icon!("braces"),
    icon!("camera"),
    icon!("check-circle-2"),
    icon!("circle-check"),
    icon!("circle-help"),
    icon!("circle-x"),
    icon!("clipboard-paste"),
    icon!("clock-3"),
    icon!("cloud-upload"),
    icon!("code-xml"),
    icon!("columns-2"),
    icon!("columns-3"),
    icon!("copy-plus"),
    icon!("ellipsis-vertical"),
    icon!("file-check"),
    icon!("file-code"),
    icon!("file-down"),
    icon!("file-input"),
    icon!("file-lock-2"),
    icon!("file-minus"),
    icon!("file-output"),
    icon!("file-pen-line"),
    icon!("file-plus-2"),
    icon!("file-scan"),
    icon!("file-search"),
    icon!("file-spreadsheet"),
    icon!("file-symlink"),
    icon!("file-text"),
    icon!("file-x"),
    icon!("files"),
    icon!("film"),
    icon!("filter"),
    icon!("flip-horizontal-2"),
    icon!("flip-vertical-2"),
    icon!("folder-plus"),
    icon!("git-compare"),
    icon!("hash"),
    icon!("heading"),
    icon!("highlighter"),
    icon!("image-plus"),
    icon!("languages"),
    icon!("list-checks"),
    icon!("maximize"),
    icon!("message-circle-reply"),
    icon!("message-square-plus"),
    icon!("message-square-quote"),
    icon!("message-square-text"),
    icon!("messages-square"),
    icon!("mic"),
    icon!("minimize"),
    icon!("minimize-2"),
    icon!("move-horizontal"),
    icon!("move-right"),
    icon!("panel-left"),
    icon!("panel-right"),
    icon!("paperclip"),
    icon!("pen-line"),
    icon!("presentation"),
    icon!("printer"),
    icon!("replace"),
    icon!("reply"),
    icon!("rows-3"),
    icon!("scan-text"),
    icon!("send"),
    icon!("settings-2"),
    icon!("share-2"),
    icon!("shield-check"),
    icon!("signature"),
    icon!("square-dashed-mouse-pointer"),
    icon!("square-terminal"),
    icon!("squares-subtract"),
    icon!("sticky-note"),
    icon!("tag"),
    icon!("text-cursor-input"),
    icon!("text-select"),
    icon!("trash-2"),
    icon!("triangle-alert"),
    icon!("type"),
    icon!("user-round"),
    icon!("wrench"),
];

/// The app's set, registered beside the catalogue by [`register`]. Its documents are drawn as drawn: they carry
/// Lucide's own stroke width.
pub(crate) static PDFCRAFT: AppIcons = AppIcons::new("pdfcraft", None, ICONS);

/// Makes the app's icons known to every icon lookup; runs before the first pass.
pub(crate) fn register() {
    register_app_icons(&PDFCRAFT);
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft::behavior::IconId;
    use craft::ui::primitives::{has_icon, icon_names};
    use pdfcraft_engine::{catalog, commands, links};

    /// Every icon string the engine carries: groups, items, commands and links.
    fn engine_icons() -> Vec<(String, &'static str)> {
        let mut names = Vec::new();
        for group in catalog::TOOL_GROUPS {
            names.push((format!("group {}", group.id), group.icon));
            for section in group.sections {
                for item in section.items {
                    names.push((format!("item {}", item.label), item.icon));
                }
            }
        }
        for command in commands::COMMANDS {
            names.push((format!("command {}", command.id), command.icon));
        }
        for link in links::LINKS {
            names.push((format!("link {}", link.command), link.icon));
        }
        names
    }

    /// A name neither the catalogue nor the app set has draws nothing, silently.
    #[test]
    fn every_icon_the_app_names_resolves() {
        register();
        let missing: Vec<String> =
            engine_icons().into_iter().filter(|(_, name)| !has_icon(&IconId::new(name))).map(|(owner, name)| format!("{owner}: {name}")).collect();
        assert!(missing.is_empty(), "icons nobody has: {missing:?}");
        assert!(has_icon(&IconId::new(crate::chrome::HOME_ICON)), "the title bar Home icon resolves");
        for name in ["circle-check", "circle-x", "triangle-alert"] {
            assert!(has_icon(&IconId::new(name)), "signature notice icon {name}");
        }
        // The eight documents the comment tools name stay served.
        for name in
            ["message-square-plus", "type", "highlighter", "move-right", "message-square-quote", "message-square-text", "replace", "paperclip"]
        {
            assert!(has_icon(&IconId::new(name)), "comment tool icon {name}");
        }
    }

    /// `register_app_icons` keeps the catalogue's glyph and only logs a clash, so a shadowing name would draw the
    /// wrong icon without a failure.
    #[test]
    fn the_app_set_never_shadows_a_catalogue_icon() {
        let catalogue = icon_names();
        let shadowing: Vec<&str> = PDFCRAFT.icons.iter().map(|(name, _)| *name).filter(|name| catalogue.contains(name)).collect();
        assert!(shadowing.is_empty(), "the catalogue already has {shadowing:?}");
    }
}
