//! Validate the release repository before it becomes part of the executable or web build.

use std::ffi::OsStr;

/// An unset override follows official releases; an invalid override fails the build.
pub fn repository(value: Option<&OsStr>) -> Result<&str, &'static str> {
    let Some(value) = value else { return Ok("storytold/pdfcraft") };
    let invalid = "PDFCRAFT_UPDATE_REPO must be a GitHub owner/repo (ASCII names only, no URL, credentials, whitespace or extra path); unset it for official releases";
    let value = value.to_str().ok_or(invalid)?;
    let (owner, repo) = value.split_once('/').ok_or(invalid)?;
    let owner_valid = !owner.is_empty()
        && owner.len() <= 39
        && owner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !owner.starts_with('-')
        && !owner.ends_with('-')
        && !owner.contains("--");
    let repo_valid = !repo.is_empty()
        && repo.len() <= 100
        && !matches!(repo, "." | "..")
        && repo.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if owner_valid && repo_valid { Ok(value) } else { Err(invalid) }
}

#[cfg(test)]
mod tests {
    use super::repository;
    use std::ffi::OsStr;

    #[test]
    fn an_unset_override_keeps_official_releases() {
        assert_eq!(repository(None), Ok("storytold/pdfcraft"));
    }

    #[test]
    fn a_custom_repository_is_preserved() {
        for repo in ["example/pdfcraft", "My-Org/PdfCraft_2.0", "a/.github", "a/b"] {
            assert_eq!(repository(Some(OsStr::new(repo))), Ok(repo));
        }
        let longest = format!("{}/{}", "a".repeat(39), "b".repeat(100));
        assert_eq!(repository(Some(OsStr::new(&longest))), Ok(longest.as_str()));
    }

    #[test]
    fn malformed_or_ambiguous_sources_fail_instead_of_falling_back() {
        for repo in [
            "",
            " ",
            "example",
            "/pdfcraft",
            "example/",
            "example/pdfcraft/extra",
            "https://github.com/example/pdfcraft",
            "git@github.com:example/pdfcraft",
            "example@github.com/pdfcraft",
            "example/pdfcraft?token=secret",
            "example/pdfcraft#main",
            "example/pdfcraft\ncargo::rustc-env=OTHER=value",
            "example/pdfcraft\r",
            "example/pdfcraft\0",
            "example/pdfcraft ",
            " example/pdfcraft",
            "example/..",
            "example/.",
            "../pdfcraft",
            "example/%2e%2e",
            "example\\pdfcraft",
            "example/pdƒcraft",
            "-example/pdfcraft",
            "example-/pdfcraft",
            "ex--ample/pdfcraft",
            "ex_ample/pdfcraft",
        ] {
            assert!(repository(Some(OsStr::new(repo))).is_err(), "accepted {repo:?}");
        }
        for repo in [format!("{}/pdfcraft", "a".repeat(40)), format!("example/{}", "b".repeat(101))] {
            assert!(repository(Some(OsStr::new(&repo))).is_err(), "accepted {repo:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_sources_fail_instead_of_falling_back() {
        use std::os::unix::ffi::OsStrExt;
        assert!(repository(Some(OsStr::from_bytes(b"example/pdfcraft\xff"))).is_err());
    }
}
