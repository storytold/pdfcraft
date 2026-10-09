//! Where PdfCraft and the ArtCraft community live on the web. One table, so the Help menu, the
//! About dialog, the home screen, the CLI and the README agree.

use icu_properties::props::Script;

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
    /// An email address asking the email app to attach a local file (`attach=`, `attachment=`)
    /// or read one into the message (`insert=`).
    MailFile,
    /// An email address containing text that some email tools decode before reading the address,
    /// so it could spell `attach`: an RFC 2047 encoded word (`=?utf-8?q?…?=`) or a backslash
    /// escape (`\0141`, `\x61`).
    MailEncodedWord,
}

impl std::fmt::Display for BlockedLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "the address is empty"),
            Self::TooLong(n) => write!(f, "the address is {n} characters long"),
            Self::Hidden => write!(f, "the address contains hidden or invisible characters"),
            Self::Malformed => write!(f, "the address isn't a complete web or email address"),
            Self::Scheme(s) => write!(f, "it is a “{s}:” address"),
            Self::MailFile => write!(f, "it asks your email app to attach or insert a file from your computer"),
            Self::MailEncodedWord => write!(f, "it contains encoded text that some email apps turn into other instructions"),
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
        _ => authority(url).is_some_and(|a| !a.contains(' ')) && display_host(url).is_some(),
    };
    // The desktop app opens addresses through the webbrowser crate, which treats one the WHATWG URL
    // parser rejects as a local file path and opens that instead (`mailto://a^b/` would become
    // `file:///…/mailto:/a^b/`). So only an address the parser accepts, as the same scheme, goes on.
    if !complete || !url::Url::parse(url).is_ok_and(|u| u.scheme() == scheme) {
        return Err(BlockedLink::Malformed);
    }
    if scheme == "mailto" {
        check_mail_parameters(rest)?;
    }
    Ok(url.to_string())
}

/// Email-address parameters that make an email app read a file from the user's computer: Evolution,
/// KMail and Claws Mail attach the file named by `attach` (Evolution and KMail also by
/// `attachment`), and Claws Mail reads an `insert` file into the message body. Names are matched as
/// prefixes, so `attachments` is refused too.
const MAIL_FILE_PARAMETERS: &[&str] = &["attach", "insert"];

/// How many layers of percent-encoding are looked through. Email apps decode once at most.
const MAIL_DECODE_LAYERS: usize = 8;

/// Refuse an email address (what follows `mailto:`) that asks for a local file. A document can't
/// need this, and a drafted message carrying the user's files is one click from being sent.
///
/// Email apps disagree on how they read the address, so this doesn't try to parse it the way any
/// one of them does. Every email app reads a parameter as `name=value`, so it refuses any word
/// that starts with one of [`MAIL_FILE_PARAMETERS`] (in any case) and is followed by `=`, wherever
/// it stands: after `?`, `&`, `#` or `;` (KMail looks for the first `?` anywhere and reads `&#38;`
/// as `&`), with no `?` at all (xdg-email reads `mailto:a@example.org&attach=…` as a parameter) or
/// after a comma (xdg-email copies the subject into Thunderbird's comma-separated `-compose`
/// argument). The check is repeated after each layer of percent-decoding (KMail decodes names, so
/// `%61ttach` is `attach`). A word without `=` (`Please attach the invoice`) is fine.
///
/// The address is refused, not stripped of the parameter: PdfCraft never rewrites what a document
/// asks to open, and a stripped address could still be read differently by some email app.
fn check_mail_parameters(rest: &str) -> Result<(), BlockedLink> {
    let mut text = rest.to_string();
    for _ in 0..MAIL_DECODE_LAYERS {
        check_mail_layer(&text)?;
        let decoded = String::from_utf8_lossy(&percent_decode(text.as_bytes())).into_owned();
        if decoded == text {
            return Ok(());
        }
        text = decoded;
    }
    check_mail_layer(&text)
}

/// One layer of [`check_mail_parameters`].
fn check_mail_layer(text: &str) -> Result<(), BlockedLink> {
    // KMail decodes RFC 2047 encoded words (`=?utf-8?q?attach?=`) across the whole address before
    // reading it, and its decoder is lenient, so any `=?` followed later by `?=` is refused.
    if text.find("=?").is_some_and(|i| text.get(i + 2..).is_some_and(|after| after.contains("?="))) {
        return Err(BlockedLink::MailEncodedWord);
    }
    // xdg-email passes the address through `echo`, which turns `\0141` (octal) or `\x61` into `a`.
    if text.split('\\').skip(1).any(|after| after.starts_with(|c: char| c.is_ascii_digit() || matches!(c, 'x' | 'X'))) {
        return Err(BlockedLink::MailEncodedWord);
    }
    let lower = text.to_ascii_lowercase();
    let is_word = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-');
    let mut previous = None;
    for (i, c) in lower.char_indices() {
        let starts_word = is_word(c) && !previous.is_some_and(is_word);
        previous = Some(c);
        if !starts_word {
            continue;
        }
        let Some(word) = lower.get(i..) else { continue };
        let name_end = word.find(|c: char| !is_word(c)).unwrap_or(word.len());
        let (name, after) = word.split_at(name_end);
        let asks = MAIL_FILE_PARAMETERS.iter().any(|p| name.starts_with(p));
        if asks && after.trim_start_matches([' ', '\t', '+', '\'', '"']).starts_with('=') {
            return Err(BlockedLink::MailFile);
        }
    }
    Ok(())
}

/// Decode `%XX` escapes. A `%` not followed by two hex digits is kept as it is.
fn percent_decode(s: &[u8]) -> Vec<u8> {
    let hex = |b: Option<&u8>| b.and_then(|b| char::from(*b).to_digit(16)).and_then(|d| u8::try_from(d).ok());
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while let Some(&b) = s.get(i) {
        if b == b'%'
            && let (Some(high), Some(low)) = (hex(s.get(i + 1)), hex(s.get(i + 2)))
        {
            out.push((high << 4) | low);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    out
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

/// How to show a web address's host so that a lookalike can't pass for a familiar site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDisplay {
    /// The host the browser connects to: lowercase ASCII, with international labels in punycode
    /// (`xn--…`) and numeric addresses in their usual form (`192.168.1.1`, `[::1]`). Show this one,
    /// not the document's spelling.
    pub ascii: String,
    /// The host has a label in another alphabet (`xn--…`), whose letters can look like familiar
    /// ones.
    pub international: bool,
    /// A label mixes writing systems, such as the Cyrillic `а` in `pаypal.com`: a common way to
    /// imitate another site's name.
    pub mixed_scripts: bool,
}

/// The host of a web address as the browser will see it (see [`HostDisplay`]). On the desktop the
/// address is handed to the operating system as the `url` crate (the WHATWG URL standard)
/// serialises it, and in the browser build the browser parses it by the same standard, so this
/// reads the host with that crate: percent-decoded, IDNA-mapped (case, fullwidth letters, `。`) and
/// with numeric hosts such as `0x7f.1` resolved. `None` for a non-web address, one without a host
/// (see [`host`]), or one the browser would reject.
pub fn display_host(url: &str) -> Option<HostDisplay> {
    host(url)?;
    let parsed = url::Url::parse(url).ok()?;
    let ascii = match parsed.host()? {
        url::Host::Domain(d) => d.to_string(),
        url::Host::Ipv4(a) => a.to_string(),
        url::Host::Ipv6(a) => format!("[{a}]"),
    };
    if !ascii.split('.').any(|label| label.starts_with("xn--")) {
        return Some(HostDisplay { ascii, international: false, mixed_scripts: false });
    }
    let (unicode, valid) = idna::domain_to_unicode(&ascii);
    let mixed_scripts = valid.is_err() || unicode.split('.').any(mixes_scripts);
    Some(HostDisplay { ascii, international: true, mixed_scripts })
}

/// A writing system for comparing the characters of a label. Han characters are written with
/// Japanese, Korean and Chinese ones, so those three combinations count as one system each
/// (Unicode TR 39's augmented script sets).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Writing {
    Script(Script),
    Japanese,
    Korean,
    Chinese,
}

/// The writing systems a character belongs to, or `None` for one shared by all (digits, `-`,
/// combining marks).
fn writing_systems(c: char) -> Option<Vec<Writing>> {
    let scripts = icu_properties::script::ScriptWithExtensions::new().get_script_extensions_val(c);
    let mut out = Vec::new();
    for s in scripts.iter() {
        if s == Script::Common || s == Script::Inherited {
            return None;
        }
        out.push(Writing::Script(s));
        match s {
            Script::Han => out.extend([Writing::Japanese, Writing::Korean, Writing::Chinese]),
            Script::Hiragana | Script::Katakana => out.push(Writing::Japanese),
            Script::Hangul => out.push(Writing::Korean),
            Script::Bopomofo => out.push(Writing::Chinese),
            _ => {}
        }
    }
    Some(out)
}

/// The writing systems every character of `label` belongs to (Unicode TR 39's resolved script
/// set), skipping characters that `skip` picks. `None` when no character narrows it down.
fn shared_writing(label: &str, skip: impl Fn(&[Writing]) -> bool) -> Option<Vec<Writing>> {
    let mut shared: Option<Vec<Writing>> = None;
    for systems in label.chars().filter_map(writing_systems) {
        if skip(&systems) {
            continue;
        }
        shared = Some(match shared {
            None => systems,
            Some(s) => s.into_iter().filter(|w| systems.contains(w)).collect(),
        });
    }
    shared
}

/// Whether a label mixes writing systems. Latin alongside Japanese, Korean or Chinese is allowed, as
/// in Unicode TR 39's "highly restrictive" level (`ソニーstore`).
fn mixes_scripts(label: &str) -> bool {
    if shared_writing(label, |_| false).is_none_or(|s| !s.is_empty()) {
        return false;
    }
    let latin = Writing::Script(Script::Latin);
    let rest = shared_writing(label, |s| s.contains(&latin)).unwrap_or_default();
    !rest.iter().any(|w| matches!(w, Writing::Japanese | Writing::Korean | Writing::Chinese))
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

    #[test]
    fn addresses_the_url_parser_rejects_are_refused() {
        use super::{BlockedLink as B, document_url};
        // The webbrowser crate would open each of these as a local file path.
        for bad in ["mailto://a^b/x", "mailto://a:99999/", "mailto://[x]/", "mailto://a\\b/", "mailto://a b/", "https://a^b/", "http://a<b/"] {
            assert_eq!(document_url(bad), Err(B::Malformed), "{bad}");
        }
        for ok in ["mailto://example.org/x", "mailto:a@example.org?subject=a b"] {
            assert_eq!(document_url(ok).as_deref(), Ok(ok), "{ok}");
        }
    }

    #[test]
    fn email_links_may_not_ask_for_a_local_file() {
        use super::{BlockedLink as B, document_url};
        for bad in [
            // Evolution, KMail and Claws Mail attach the named file.
            "mailto:a@example.org?attach=/home/me/.ssh/id_ed25519",
            "mailto:a@example.org?subject=Hi&attachment=file:///etc/passwd",
            // Claws Mail reads an `insert` file into the message body.
            "mailto:a@example.org?insert=/home/me/notes.txt",
            // Names are matched case-insensitively (Evolution, KMail) and after percent-decoding (KMail).
            "mailto:a@example.org?ATTACH=x",
            "mailto:a@example.org?Attachment=x",
            "mailto:a@example.org?%61ttach=x",
            "mailto:a@example.org?%2561ttach=x",
            "mailto:a@example.org?%49NSERT=x",
            "mailto:a@example.org?attach%3D/etc/passwd",
            // An empty value, a repeated name, spaces or quotes before `=`.
            "mailto:a@example.org?attach=",
            "mailto:a@example.org?subject=a&attach=x&attach=y",
            "mailto:a@example.org?subject=a& attach =x",
            "mailto:a@example.org?subject=a&'attachment'='x'",
            // No recipient, no `?` at all (xdg-email), a second `?`, and a `?` that only exists once decoded.
            "mailto:?attach=x",
            "mailto:a@example.org&attach=/etc/passwd",
            "mailto:a@example.org?subject=a?attach=x",
            "mailto:a@example.org%3Fattach=x",
            // KMail reads the query from the first `?` even inside a fragment, and reads `&#38;` as `&`.
            "mailto:a@example.org#?attach=x",
            "mailto:a@example.org?subject=a#&attach=x",
            "mailto:a@example.org?subject=a&#38;attach=x",
            "mailto:a@example.org?subject=a;attach=x",
            // An encoded `&`, and a comma (xdg-email copies the subject into a comma-separated list).
            "mailto:a@example.org?subject=a%26attach=x",
            "mailto:a@example.org?subject=a,attachment='/etc/passwd'",
            // A `+`, an encoded tab or an encoded space between the name and `=`.
            "mailto:a@example.org?attach+=x",
            "mailto:a@example.org?attach%09=x",
            "mailto:a@example.org?attach%20=x",
        ] {
            assert_eq!(document_url(bad), Err(B::MailFile), "{bad}");
        }
        // KMail decodes RFC 2047 encoded words across the whole address before reading it, so
        // `=?utf-8?q?attach?=` could become `attach`. Such words never belong in a link.
        for bad in [
            "mailto:a@example.org?=?utf-8?q?attach?==/etc/passwd",
            "mailto:a@example.org?subject==?UTF-8?B?YXR0YWNo?=",
            "mailto:a@example.org?subject=%3D%3Futf-8%3Fq%3Fattach%3F%3D",
            "mailto:a@example.org?subject==??q?attach?=",
            "mailto:a@example.org?subject==?utf-8?base64?YXR0YWNo?=",
            // Backslash escapes that `echo` in xdg-email decodes, raw and percent-encoded.
            "mailto:a@example.org?\\0141ttach=x",
            "mailto:x\\x61ttachment=y",
            "mailto:a@example.org?%5C0141ttach=x",
        ] {
            assert_eq!(document_url(bad), Err(B::MailEncodedWord), "{bad}");
        }
        for ok in [
            "mailto:orders@example.org?subject=Order form&body=Please attach the invoice",
            "mailto:a@example.org?subject=Attachments%20to%20follow",
            "mailto:a@example.org?body=Questions%3F%20Attach%20your%20CV",
            "mailto:a@example.org?body=Hi%3B%20insert%20your%20name",
            "mailto:a@example.org?cc=b@example.org&bcc=c@example.org&in-reply-to=%3Cid@example.org%3E",
            "mailto:attach@example.org",
            "mailto:a@example.org?attach",
            "mailto:a@example.org?x-attach=1&reattach=2",
            "mailto:a@example.org?body=Saved in C:\\Users\\me\\Documents",
        ] {
            assert_eq!(document_url(ok).as_deref(), Ok(ok), "{ok}");
        }
    }

    #[test]
    fn hosts_are_shown_as_the_browser_receives_them() {
        use super::{BlockedLink as B, HostDisplay, display_host, document_url};
        let plain = |a: &str| Some(HostDisplay { ascii: a.into(), international: false, mixed_scripts: false });
        assert_eq!(display_host("https://example.org/path"), plain("example.org"));
        assert_eq!(display_host("HTTPS://EXAMPLE.ORG/"), plain("example.org"), "lowercased, as the browser does");
        assert_eq!(display_host("https://example.org./"), plain("example.org."));
        assert_eq!(display_host("https://ex%61mple.org/"), plain("example.org"), "percent-decoded, as the browser does");
        assert_eq!(display_host("https://ｅｘａｍｐｌｅ。org/"), plain("example.org"), "fullwidth letters and `。` are mapped");
        assert_eq!(display_host("https://trusted.example@other.example/"), plain("other.example"));
        assert_eq!(display_host("https://other.example\\@trusted.example/"), plain("other.example"));
        assert_eq!(display_host("mailto:a@example.org"), None);
        // Numeric hosts in the form the browser connects to, so a local address can't hide.
        assert_eq!(display_host("https://[::1]:8443/"), plain("[::1]"));
        assert_eq!(display_host("https://[0:0:0:0:0:0:0:1]/"), plain("[::1]"));
        assert_eq!(display_host("http://3232235777/"), plain("192.168.1.1"));
        assert_eq!(display_host("http://0x7f.1/"), plain("127.0.0.1"));
        assert_eq!(display_host("http://0300.0250.01.01/"), plain("192.168.1.1"));

        // International hosts are shown in punycode and marked.
        let bucher = display_host("https://bücher.example/").unwrap();
        assert_eq!(bucher, HostDisplay { ascii: "xn--bcher-kva.example".into(), international: true, mixed_scripts: false });
        assert!(!display_host("https://ソニーstore.example/").unwrap().mixed_scripts, "Latin with Japanese is normal (UTS #39)");
        assert!(!display_host("https://example.ελ/").unwrap().mixed_scripts, "scripts are compared within a label, not across labels");
        // Cyrillic `а` (U+0430) in a Latin name: punycode, and flagged as mixed.
        let lookalike = display_host("https://p\u{0430}ypal.com/").unwrap();
        assert_eq!(lookalike, HostDisplay { ascii: "xn--pypal-4ve.com".into(), international: true, mixed_scripts: true });
        // The same lookalike written in punycode in the document, and upper-case.
        assert_eq!(display_host("https://XN--PYPAL-4VE.com/"), Some(lookalike));
        // A whole-script lookalike (all Cyrillic) isn't mixed, but is still shown in punycode, marked.
        let cyrillic = display_host("https://\u{0430}\u{0440}\u{0440}\u{04CF}\u{0435}.com/").unwrap();
        assert!(cyrillic.ascii.starts_with("xn--") && cyrillic.international && !cyrillic.mixed_scripts, "{cyrillic:?}");

        // A host the browser can't turn into an address is refused rather than shown.
        for bad in [
            "https://xn--a.example/",
            "https://exa%FFmple.org/",
            "https://\u{0300}a.example/",
            "https://%E2%80%AEexample.org/",
            "https://trusted.example%40other.example/",
            "https://[evil.example]/",
            "https://example.org:port/",
            "https://example.org:99999/",
        ] {
            assert_eq!(display_host(bad), None, "{bad}");
            assert_eq!(document_url(bad), Err(B::Malformed), "{bad}");
        }
    }
}
