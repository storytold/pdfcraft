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

/// The kinds of address a document may ask PdfCraft to open: web pages and email. Anything
/// else (`file:`, `javascript:`, `data:`, `smb:`, app handlers such as `ms-settings:`) is refused,
/// because the operating system would hand it to whichever program claims it (#90, #91).
pub const DOCUMENT_SCHEMES: &[&str] = &["https", "http", "mailto"];

/// The longest address a document may ask to open. A longer one is refused rather than shown
/// cut short in the confirmation, where the hidden tail could carry the document's form data.
pub const MAX_DOCUMENT_URL: usize = 2048;

/// Why an address that came from a document was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockedLink {
    Empty,
    /// Longer than [`MAX_DOCUMENT_URL`] characters.
    TooLong(usize),
    /// Control, zero-width or bidirectional-override characters, or whitespace other than a plain
    /// space, which can hide part of the address or make it read as something else.
    Hidden,
    /// No scheme, or a web address without a host or with a space before its path.
    Malformed,
    /// A scheme other than [`DOCUMENT_SCHEMES`] (lowercased).
    Scheme(String),
}

impl std::fmt::Display for BlockedLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "the address is empty"),
            Self::TooLong(n) => write!(f, "the address is {n} characters long"),
            Self::Hidden => write!(f, "the address contains hidden or invisible characters"),
            Self::Malformed => write!(f, "the address isn't a complete web or email address"),
            Self::Scheme(s) => write!(f, "it is a “{s}:” address"),
        }
    }
}

/// Characters that can hide or disguise part of an address in the confirmation dialog. A plain
/// space is visible and turns up in real links (`mailto:…?subject=Order form`), so it is allowed.
fn hides_text(c: char) -> bool {
    (c.is_whitespace() && c != ' ')
        || c.is_control()
        || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// Check an address that came from a document (a link, a button's URI action or a script's
/// `app.launchURL`) before PdfCraft offers to open it. Returns the address trimmed of
/// surrounding whitespace. PdfCraft's own links (Help, About, updates) don't come through here.
pub fn document_url(raw: &str) -> Result<String, BlockedLink> {
    let url = raw.trim();
    if url.is_empty() {
        return Err(BlockedLink::Empty);
    }
    let len = url.chars().count();
    if len > MAX_DOCUMENT_URL {
        return Err(BlockedLink::TooLong(len));
    }
    if url.chars().any(hides_text) {
        return Err(BlockedLink::Hidden);
    }
    let (scheme, rest) = url.split_once(':').ok_or(BlockedLink::Malformed)?;
    // RFC 3986: ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ).
    let well_formed = scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !well_formed {
        return Err(BlockedLink::Malformed);
    }
    let scheme = scheme.to_ascii_lowercase();
    if !DOCUMENT_SCHEMES.contains(&scheme.as_str()) {
        return Err(BlockedLink::Scheme(scheme));
    }
    let complete = match scheme.as_str() {
        "mailto" => !rest.is_empty(),
        _ => authority(url).is_some_and(|a| !a.contains(' ')) && host(url).is_some(),
    };
    if !complete {
        return Err(BlockedLink::Malformed);
    }
    Ok(url.to_string())
}

/// The authority of a web address: what follows `//`, up to the first `/`, `\`, `?` or `#`.
/// Browsers read `\` as `/` in web addresses (the WHATWG URL standard), so it ends the authority
/// too; otherwise `https://other.example\@trusted.example/` would seem to go to
/// `trusted.example`. `None` for a non-web address or one without `//`.
fn authority(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once(':')?;
    if !(scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http")) {
        return None;
    }
    rest.strip_prefix("//")?.split(['/', '\\', '?', '#']).next()
}

/// The host a web address really goes to, without any `user:password@` prefix, so
/// `https://trusted.example@other.example/` reports `other.example`. `None` for a non-web
/// address or one without a host.
pub fn host(url: &str) -> Option<&str> {
    let authority = authority(url)?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if host_port.starts_with('[') {
        // An IPv6 literal keeps its brackets; only a port after them is dropped.
        host_port.find(']').and_then(|i| host_port.get(..=i)).unwrap_or_default()
    } else {
        host_port.split(':').next().unwrap_or_default()
    };
    (!host.is_empty()).then_some(host)
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

    #[test]
    fn documents_may_offer_web_and_email_addresses() {
        use super::document_url;
        for ok in [
            "https://example.org",
            "HTTPS://Example.org/a?b=c#d",
            "http://example.org:8080/x",
            "mailto:someone@example.org",
            "https://[::1]:8443/",
            // Plain spaces turn up in real documents' links and are visible in the dialog.
            "mailto:orders@example.org?subject=Order form",
            "https://example.org/My Report.pdf",
        ] {
            assert_eq!(document_url(ok).as_deref(), Ok(ok), "{ok}");
        }
        let multibyte = format!("https://example.org/{}", "é".repeat(super::MAX_DOCUMENT_URL - 20));
        assert!(document_url(&multibyte).is_ok(), "the limit counts characters, not bytes");
        assert_eq!(document_url("  https://example.org/ \n").as_deref(), Ok("https://example.org/"), "surrounding whitespace is trimmed");
    }

    #[test]
    fn documents_may_not_offer_other_kinds_of_address() {
        use super::{BlockedLink as B, MAX_DOCUMENT_URL, document_url};
        let scheme = |s: &str| Err(B::Scheme(s.into()));
        assert_eq!(document_url("file:///C:/Windows/System32/calc.exe"), scheme("file"));
        assert_eq!(document_url("FILE://server/share/x.exe"), scheme("file"));
        assert_eq!(document_url("javascript:alert(1)"), scheme("javascript"));
        assert_eq!(document_url("data:text/html,<script>x</script>"), scheme("data"));
        assert_eq!(document_url("ms-settings:privacy"), scheme("ms-settings"));
        assert_eq!(document_url("smb://server/share"), scheme("smb"));
        assert_eq!(document_url(""), Err(B::Empty));
        assert_eq!(document_url("   "), Err(B::Empty));
        for bad in [
            "example.org",
            "//example.org",
            "1http://x",
            "ht tp://x",
            "https:",
            "https://",
            "https:///path",
            "https://user@/x",
            "mailto:",
            "https://trusted.example /x",
        ] {
            assert_eq!(document_url(bad), Err(B::Malformed), "{bad}");
        }
        for hidden in [
            "https://exa\u{202E}gro.elpmaxe",
            "https://example.org\u{200B}.evil",
            "https://a.org/\u{0007}",
            "https://a.org/x\ty",
            "https://a.org/x\ny",
            "https://example.org/a\u{00A0}b",
        ] {
            assert_eq!(document_url(hidden), Err(B::Hidden), "{hidden:?}");
        }
        let long = format!("https://example.org/?q={}", "a".repeat(MAX_DOCUMENT_URL));
        assert!(matches!(document_url(&long), Err(B::TooLong(_))));
    }

    #[test]
    fn the_host_shown_is_the_one_the_address_goes_to() {
        use super::host;
        assert_eq!(host("https://example.org/path"), Some("example.org"));
        assert_eq!(host("https://trusted.example@other.example/"), Some("other.example"));
        assert_eq!(host("http://user:pw@other.example:8080?x"), Some("other.example"));
        assert_eq!(host("https://[::1]:8443/"), Some("[::1]"));
        // Browsers read `\` as `/`, so this goes to other.example, not trusted.example.
        assert_eq!(host("https://other.example\\@trusted.example/"), Some("other.example"));
        assert_eq!(host("https://other.example\\x@trusted.example/"), Some("other.example"));
        assert_eq!(host("mailto:a@example.org"), None);
        assert_eq!(host("https:///nohost"), None);
    }
}
