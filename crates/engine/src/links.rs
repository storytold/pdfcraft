//! Web link helpers and validation for Linkco PDF Editor.

/// The repository URL used for release checks and server metadata.
pub const APP_PAGE: &str = "https://github.com/b-lincko/linkco-pdf";
pub const GITHUB: &str = "https://github.com/b-lincko/linkco-pdf";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    /// Registry command id (`help.*`), so the Help menu and the palette use the same entry.
    pub command: &'static str,
    pub label: &'static str,
    pub url: &'static str,
    pub icon: &'static str,
}

/// External community links (empty in Linkco PDF Editor).
pub const LINKS: &[Link] = &[];

pub fn for_command(command: &str) -> Option<&'static Link> {
    LINKS.iter().find(|l| l.command == command)
}

/// The kinds of address a document may ask Linkco PDF Editor to open: web pages and email. Anything
/// else (`file:`, `javascript:`, custom protocol handlers, relative paths, control characters)
/// is refused so a PDF cannot launch local programs or scripts through the system URL handler.
pub fn document_url(raw: &str) -> Result<&str, &'static str> {
    let u = raw.trim();
    if u.is_empty() {
        return Err("the link address is empty");
    }
    if u.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err("the link address contains control characters");
    }
    let Some((scheme, rest)) = u.split_once(':') else {
        return Err("only web (http, https) and email (mailto) links can be opened");
    };
    if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
        let host = rest.strip_prefix("//").unwrap_or("").split(['/', '?', '#']).next().unwrap_or("");
        if host.is_empty() || host.contains(char::is_whitespace) {
            return Err("the web address has no host name");
        }
        Ok(u)
    } else if scheme.eq_ignore_ascii_case("mailto") {
        let addr = rest.trim_start_matches('/').split('?').next().unwrap_or("");
        if addr.is_empty() || addr.contains(char::is_whitespace) {
            return Err("the email link has no address");
        }
        Ok(u)
    } else {
        Err("only web (http, https) and email (mailto) links can be opened")
    }
}

/// The host name (or email address, for `mailto:`) of a validated [`document_url`], shown so the
/// user can spot a misleading link before opening it.
pub fn host(url: &str) -> Option<&str> {
    let (scheme, rest) = url.trim().split_once(':')?;
    if scheme.eq_ignore_ascii_case("mailto") {
        rest.trim_start_matches('/').split('?').next().filter(|s| !s.is_empty())
    } else {
        rest.strip_prefix("//")?.split(['/', '?', '#']).next().filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands;

    #[test]
    fn every_link_has_a_matching_help_command() {
        for l in LINKS {
            let cmd = commands::command(l.command).unwrap_or_else(|| panic!("missing command {}", l.command));
            assert_eq!(cmd.label, l.label);
            assert_eq!(cmd.menu, Some("Help"));
            assert_eq!(for_command(l.command), Some(l));
        }
    }

    #[test]
    fn urls_use_https() {
        assert!(APP_PAGE.starts_with("https://"));
        assert!(GITHUB.starts_with("https://"));
        assert!(LINKS.is_empty());
    }

    #[test]
    fn document_url_accepts_http_https_and_mailto() {
        assert_eq!(document_url("https://example.org/path?a=1#s"), Ok("https://example.org/path?a=1#s"));
        assert_eq!(document_url("  HTTP://Example.org  "), Ok("HTTP://Example.org"));
        assert_eq!(document_url("mailto:ada@example.org?subject=Hi"), Ok("mailto:ada@example.org?subject=Hi"));
    }

    #[test]
    fn document_url_rejects_local_files_scripts_and_custom_schemes() {
        for bad in [
            "",
            "   ",
            "example.org",
            "/etc/passwd",
            "\\\\server\\share\\a.exe",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script>",
            "ms-word:ofe|u|https://example.org/a.docx",
            "custom://open",
            "https://",
            "https:///path",
            "https://exam ple.org",
            "https://example.org/\n",
            "mailto:",
            "mailto:?subject=hi",
        ] {
            assert!(document_url(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn host_extracts_the_domain() {
        assert_eq!(host("https://sub.example.org:8080/a/b?c=1"), Some("sub.example.org:8080"));
        assert_eq!(host("mailto:ada@example.org?subject=Hi"), Some("ada@example.org"));
    }
}
