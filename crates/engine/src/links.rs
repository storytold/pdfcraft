//! Where PdfCraft and the ArtCraft community live on the web. One table, so the Help menu, the
//! About dialog, the home screen, the CLI and the README agree.

/// The app's name in ArtCraft URLs (`getartcraft.com/apps/{APP}`, `github.com/storytold/{APP}`).
pub const APP: &str = "pdfcraft";

pub const DISCORD: &str = "https://discord.gg/artcraft";
pub const WEBSITE: &str = "https://getartcraft.com";
pub const APP_PAGE: &str = "https://getartcraft.com/apps/pdfcraft";
pub const GITHUB: &str = "https://github.com/storytold/pdfcraft";

/// A link and the registry command that opens it.
#[derive(Clone, Copy, Debug)]
pub struct Link {
    pub command: &'static str,
    pub label: &'static str,
    pub url: &'static str,
    /// Lucide icon name.
    pub icon: &'static str,
}

/// In the order they are shown. Discord comes first: it is where people get help fastest.
pub const LINKS: &[Link] = &[
    Link { command: "help.discord", label: "Join the ArtCraft Discord", url: DISCORD, icon: "messages-square" },
    Link { command: "help.app_page", label: "PdfCraft web page", url: APP_PAGE, icon: "globe" },
    Link { command: "help.github", label: "PdfCraft on GitHub", url: GITHUB, icon: "code-xml" },
    Link { command: "help.website", label: "ArtCraft website", url: WEBSITE, icon: "external-link" },
];

pub fn for_command(id: &str) -> Option<&'static Link> {
    LINKS.iter().find(|l| l.command == id)
}

#[cfg(test)]
mod tests {
    #[test]
    fn urls_follow_the_artcraft_scheme() {
        assert_eq!(super::APP_PAGE, format!("{}/apps/{}", super::WEBSITE, super::APP));
        assert_eq!(super::GITHUB, format!("https://github.com/storytold/{}", super::APP));
        for l in super::LINKS {
            assert!(l.url.starts_with("https://"), "{}", l.url);
            assert!(crate::commands::command(l.command).is_some(), "{} is a registered command", l.command);
        }
    }
}
