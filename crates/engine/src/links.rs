//! Where PdfCraft and the ArtCraft community live on the web. One table, so the Help menu, the
//! About dialog, the home screen, the CLI and the README agree.

use icu_properties::props::Script;
use std::path::{Path, PathBuf};

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

/// Maximum decoded GoToR file-spec size retained by inspection and accepted by the resolver.
pub const MAX_REMOTE_DOCUMENT_PATH_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteDocumentPathError {
    Empty,
    TooLong,
    Hidden,
    /// A URL, a network or device path, or a path that climbs above its root.
    NotLocal(craft_docpath::Refused),
    MissingSourcePath,
    /// The target doesn't end in `.pdf`.
    NotPdf,
    /// The target is a directory, device, FIFO or socket rather than a regular file.
    NotAFile,
    /// The target is larger than [`MAX_REMOTE_DOCUMENT_BYTES`].
    TooLarge,
    /// The target couldn't be read.
    Read(String),
}

impl std::fmt::Display for RemoteDocumentPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("the file specification is empty"),
            Self::TooLong => f.write_str("the file specification is too long"),
            Self::Hidden => f.write_str("the file specification contains hidden or control characters"),
            Self::NotLocal(r) => write!(f, "only local document paths are supported: {r}"),
            Self::MissingSourcePath => f.write_str("a relative link needs a source document path"),
            Self::NotPdf => f.write_str("the linked file is not a .pdf file"),
            Self::NotAFile => f.write_str("the linked path is not a regular file"),
            Self::TooLarge => f.write_str("the linked file is too large to open"),
            Self::Read(e) => write!(f, "the linked file couldn't be read: {e}"),
        }
    }
}

/// Whether the original GoToR file specification is absolute on this platform.
/// Check this before resolving relative paths against the source document. This does no I/O;
/// the resolver must still reject URL/network paths and malformed file specifications.
pub fn remote_document_path_is_absolute(raw: &str) -> bool {
    Path::new(raw).is_absolute()
}

/// Resolve a PDF remote-Go-To file specification to a local path without opening it.
/// Relative paths are based on the source document, which should be an absolute path; an
/// in-memory source has no fallback to CWD. URLs (including `file:`), network and device paths are
/// refused ([`craft_docpath::resolve_absolute`]).
pub fn resolve_remote_document_path(source: Option<&Path>, raw: &str) -> Result<PathBuf, RemoteDocumentPathError> {
    if raw.is_empty() || raw.trim().is_empty() {
        return Err(RemoteDocumentPathError::Empty);
    }
    if raw.len() > MAX_REMOTE_DOCUMENT_PATH_BYTES {
        return Err(RemoteDocumentPathError::TooLong);
    }
    if raw.chars().any(hides_text) {
        return Err(RemoteDocumentPathError::Hidden);
    }
    // A GoToR file specification is a path, not a URL: any scheme of two or more characters is
    // refused (one letter is a drive), `file:` and names such as `custom:target.pdf` included.
    if let Some((scheme, _)) = raw.split_once(':') {
        let is_scheme = scheme.len() > 1
            && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if is_scheme {
            return Err(RemoteDocumentPathError::NotLocal(craft_docpath::Refused::Url));
        }
    }
    craft_docpath::resolve_absolute(raw, source).map_err(|r| match r {
        // A path that resolves but not to an absolute one needed a source with an absolute path. With
        // such a source, it is a path this platform cannot open (`C:\x.pdf` on Unix).
        craft_docpath::Refused::Invalid if !source.is_some_and(Path::is_absolute) && craft_docpath::resolve(raw, source).is_ok() => {
            RemoteDocumentPathError::MissingSourcePath
        }
        r => RemoteDocumentPathError::NotLocal(r),
    })
}

/// Largest linked PDF a GoToR link may open (1 GiB, the largest size the large-file work measures).
pub const MAX_REMOTE_DOCUMENT_BYTES: u64 = 1 << 30;

/// Read the PDF a GoToR link resolved to. The path comes from an untrusted document, so only a
/// regular `.pdf` file of at most [`MAX_REMOTE_DOCUMENT_BYTES`] is read: a directory, a device such
/// as `/dev/zero` or a FIFO is refused before it is opened, and a file that grows past the cap
/// while it is read is refused too.
pub fn read_remote_document(path: &Path) -> Result<Vec<u8>, RemoteDocumentPathError> {
    use std::io::Read;
    let is_pdf = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if !is_pdf {
        return Err(RemoteDocumentPathError::NotPdf);
    }
    let meta = std::fs::metadata(path).map_err(|e| RemoteDocumentPathError::Read(e.to_string()))?;
    if !meta.is_file() {
        return Err(RemoteDocumentPathError::NotAFile);
    }
    if meta.len() > MAX_REMOTE_DOCUMENT_BYTES {
        return Err(RemoteDocumentPathError::TooLarge);
    }
    let file = std::fs::File::open(path).map_err(|e| RemoteDocumentPathError::Read(e.to_string()))?;
    let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.take(MAX_REMOTE_DOCUMENT_BYTES.saturating_add(1)).read_to_end(&mut bytes).map_err(|e| RemoteDocumentPathError::Read(e.to_string()))?;
    if u64::try_from(bytes.len()).map_or(true, |n| n > MAX_REMOTE_DOCUMENT_BYTES) {
        return Err(RemoteDocumentPathError::TooLarge);
    }
    Ok(bytes)
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
mod tests;
