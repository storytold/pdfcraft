//! Native launch arguments can carry navigation; filesystem-path APIs stay literal.

use pdfcraft_render::{DestView, RemoteDestination};

const MAX_ARGUMENT_BYTES: usize = 1 << 20;
const MAX_PATH_BYTES: usize = 128 << 10;
const MAX_FRAGMENT_BYTES: usize = 64 << 10;
const MAX_DESTINATIONS: usize = 64;
// Every semantic byte may expand to three URI bytes, plus bounded parameter headers.
const MAX_ENCODED_FRAGMENT_BYTES: usize = 3 * MAX_FRAGMENT_BYTES + 32 * MAX_DESTINATIONS;
const MAX_NAME_BYTES: usize = 1024;

/// A local file and its bounded, ordered navigation requests from a native launcher.
#[derive(Clone, Debug)]
pub struct NativeOpenTarget {
    pub(crate) path: String,
    pub(crate) destinations: Vec<RemoteDestination>,
    has_parameters: bool,
}

impl NativeOpenTarget {
    /// Parse one launch argument, keeping URI escaping distinct from literal path characters.
    /// Existing plain filenames win over fragment syntax. Only local file URIs are decoded.
    pub fn from_arg(argument: &str) -> Result<Self, String> {
        if argument.len() > MAX_ARGUMENT_BYTES {
            return Err("The file launch argument is too long.".into());
        }
        if argument.contains('\0') {
            return Err("The file launch argument contains a NUL character.".into());
        }
        let (path, fragment) = if let Some(rest) =
            argument.get(..7).filter(|prefix| prefix.eq_ignore_ascii_case("file://")).and_then(|_| argument.get(7..))
        {
            let (path, fragment) = rest.split_once('#').map_or((rest, None), |(p, f)| (p, Some(f)));
            let path = if path.starts_with('/') {
                path
            } else {
                let (host, _) = path.split_once('/').ok_or("The file URI has no local path.")?;
                if !host.eq_ignore_ascii_case("localhost") {
                    return Err("Only local file URIs are supported.".into());
                }
                // The slash was the ASCII delimiter of split_once, hence a character boundary.
                path.get(host.len()..).ok_or("The file URI has no local path.")?
            };
            let decoded = percent_decode(path)?;
            let decoded = if cfg!(windows)
                && decoded.starts_with('/')
                && (decoded.as_bytes().get(2) == Some(&b':') || decoded.get(1..).is_some_and(|path| path.starts_with("\\\\")))
            {
                decoded.get(1..).ok_or("The file URI has no local path.")?.to_string()
            } else {
                decoded
            };
            (decoded, fragment)
        } else if std::path::Path::new(argument).is_file() {
            (argument.to_string(), None)
        } else {
            // A hash may belong to a directory or filename. Only a real file prefix can
            // introduce launch navigation, and at most 64 prefixes are probed.
            let boundaries: Vec<_> = argument.match_indices('#').take(MAX_DESTINATIONS + 1).map(|(at, _)| at).collect();
            if boundaries.len() > MAX_DESTINATIONS {
                return Err("The file launch argument has too many fragment boundaries.".into());
            }
            let candidate = |at: usize| Some((argument.get(..at)?, argument.get(at.checked_add(1)?..)?)).filter(|(_, fragment)| recognized(fragment));
            let existing = boundaries.iter().rev().filter_map(|&at| candidate(at)).find(|(path, _)| std::path::Path::new(path).is_file());
            // Missing files still keep their fragment out of filesystem normalization.
            let syntactic = || {
                boundaries
                    .iter()
                    .filter_map(|&at| candidate(at))
                    .find(|(path, _)| std::path::Path::new(path).extension().is_some_and(|extension| extension.eq_ignore_ascii_case("pdf")))
            };
            existing.or_else(syntactic).map_or_else(|| (argument.to_string(), None), |(path, fragment)| (path.to_string(), Some(fragment)))
        };
        let has_parameters = fragment.is_some();
        let mut destinations = Vec::new();
        if let Some(fragment) = fragment {
            if fragment.len() > MAX_ENCODED_FRAGMENT_BYTES {
                return Err("The PDF launch parameters are too long.".into());
            }
            let mut value_bytes = 0usize;
            for (index, token) in fragment.split(['&', '#']).enumerate() {
                if index >= MAX_DESTINATIONS {
                    return Err("There are too many PDF launch parameters.".into());
                }
                let Some((key, value)) = token.split_once('=') else { continue };
                let value = percent_decode(value)?;
                value_bytes = value_bytes
                    .checked_add(value.len())
                    .filter(|size| *size <= MAX_FRAGMENT_BYTES)
                    .ok_or("The decoded PDF launch parameters are too long.")?;
                match key {
                    "page" => {
                        let page = value
                            .parse::<usize>()
                            .ok()
                            .and_then(|n| n.checked_sub(1))
                            .ok_or("The PDF launch page must be a positive physical page number.")?;
                        destinations.push(RemoteDestination::Page { page, view: DestView::Top });
                    }
                    "nameddest" => {
                        let name = value;
                        if name.is_empty() || name.len() > MAX_NAME_BYTES || name.contains('\0') {
                            return Err("The PDF launch destination name is empty or too long.".into());
                        }
                        destinations.push(RemoteDestination::Named(name));
                    }
                    // Unsupported view parameters never become filesystem paths or actions.
                    _ => {}
                }
            }
        }
        // Normalize only the filesystem path, never slashes or dot segments in a name.
        let absolute = std::path::absolute(&path).map_err(|error| error.to_string())?;
        let path = absolute.to_str().ok_or("The absolute launch path is not valid UTF-8.")?.to_string();
        if path.len() > MAX_PATH_BYTES {
            return Err("The absolute launch path is too long.".into());
        }
        Ok(Self { path, destinations, has_parameters })
    }

    /// Whether the argument carried launch parameters, including unsupported view parameters.
    pub fn has_launch_parameters(&self) -> bool {
        self.has_parameters
    }

    /// Keep parameters out of platform path normalization when forwarding to another process.
    /// Every target uses an escaped file URI so literal hashes stay unambiguous even if the
    /// file disappears before the receiving process opens it.
    pub fn absolute_argument(&self) -> Result<String, String> {
        let path = &self.path;
        let mut encoded = String::from("file://");
        if !path.starts_with('/') {
            encoded.push('/');
        }
        encode_into(path, true, &mut encoded)?;
        for (index, destination) in self.destinations.iter().enumerate() {
            encoded.push(if index == 0 { '#' } else { '&' });
            match destination {
                RemoteDestination::Page { page, .. } => {
                    let page = page.checked_add(1).ok_or("The PDF launch page is too large.")?;
                    use std::fmt::Write;
                    write!(&mut encoded, "page={page}").map_err(|e| e.to_string())?;
                }
                RemoteDestination::Named(name) => {
                    encoded.push_str("nameddest=");
                    encode_into(name, false, &mut encoded)?;
                }
                RemoteDestination::Default => {}
            }
        }
        if encoded.len() > MAX_ARGUMENT_BYTES {
            return Err("The encoded file launch argument is too long.".into());
        }
        Ok(encoded)
    }

    /// The ordered navigation requests, bounded by the launch parser.
    pub fn destinations(&self) -> &[RemoteDestination] {
        &self.destinations
    }

    /// The literal filesystem path, without launch navigation.
    pub fn path(&self) -> &str {
        &self.path
    }
}

fn recognized(fragment: &str) -> bool {
    let mut navigation = false;
    for token in fragment.split(['&', '#']).take(MAX_DESTINATIONS) {
        if token.is_empty() {
            continue;
        }
        let Some((key, _)) = token.split_once('=') else { return false };
        navigation |= matches!(key, "page" | "nameddest");
    }
    navigation
}

fn encode_into(value: &str, path: bool, output: &mut String) -> Result<(), String> {
    use std::fmt::Write;
    for byte in value.bytes() {
        let literal = byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) || (path && b"/:".contains(&byte));
        let required = if literal { 1 } else { 3 };
        if output.len().checked_add(required).is_none_or(|size| size > MAX_ARGUMENT_BYTES) {
            return Err("The encoded file launch argument is too long.".into());
        }
        if literal {
            output.push(char::from(byte));
        } else {
            write!(output, "%{byte:02X}").map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Decode once, after separating delimiters. Plus is a literal PDF name character.
fn percent_decode(value: &str) -> Result<String, String> {
    let mut out = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let hex: Vec<_> = bytes.clone().take(2).collect();
            // The numeric parser also accepts a leading '+', which is not a percent escape.
            if let Some(decoded) = std::str::from_utf8(&hex)
                .ok()
                .filter(|s| s.len() == 2 && s.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .and_then(|s| u8::from_str_radix(s, 16).ok())
            {
                out.push(decoded);
                bytes.nth(1);
                continue;
            }
        }
        out.push(byte);
    }
    let decoded = String::from_utf8(out).map_err(|_| "The file launch argument has invalid UTF-8 escapes.".to_string())?;
    if decoded.contains('\0') {
        return Err("The file launch argument contains a NUL character.".into());
    }
    Ok(decoded)
}

impl crate::PdfCraftApp {
    /// Open a prepared launch target through the ordinary document lifecycle.
    pub fn open_target(&mut self, target: NativeOpenTarget) -> crate::state::OpenOutcome {
        self.open_path_with_destinations(&target.path, None, target.destinations)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            for _ in 0..100 {
                let path = std::env::temp_dir().join(format!("pdfcraft-launch-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
                if std::fs::create_dir(&path).is_ok() {
                    return Self(path);
                }
            }
            panic!("could not create an isolated fixture directory");
        }
        fn file(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, b"original parser fixture").unwrap();
            path
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn uri(path: &std::path::Path) -> String {
        let path = path.to_string_lossy().replace('\\', "/").replace('%', "%25").replace('#', "%23").replace(' ', "%20");
        format!("file://{}{path}", if path.starts_with('/') { "" } else { "/" })
    }

    #[test]
    fn literal_hash_and_percent_filenames_stay_distinct_from_uri_fragments() {
        let dir = Directory::new();
        for name in ["literal#page=2.pdf", "literal%20.pdf", "draft#part/manual with spaces.pdf"] {
            let file = dir.file(name);
            let plain = NativeOpenTarget::from_arg(&file.to_string_lossy()).unwrap();
            assert_eq!(std::path::Path::new(plain.path()), file.as_path());
            assert!(plain.destinations.is_empty());
            let encoded = NativeOpenTarget::from_arg(&uri(&file)).unwrap();
            assert_eq!(std::path::Path::new(encoded.path()), file.as_path());
            assert!(encoded.destinations.is_empty(), "encoded hashes are filename characters");
            let target = NativeOpenTarget::from_arg(&format!("{}#page=2", file.display())).unwrap();
            assert_eq!(std::path::Path::new(target.path()), file.as_path());
            assert_eq!(target.destinations, [RemoteDestination::Page { page: 1, view: DestView::Top }]);
        }
    }

    #[test]
    fn fragment_delimiters_are_split_before_decoding_and_order_is_retained() {
        let dir = Directory::new();
        let file = dir.file("manual with spaces.pdf");
        let suffix = "#page=2&nameddest=chapter%26two%23x+z#page=3&zoom=100";
        for argument in [format!("{}{suffix}", file.display()), format!("{}{suffix}", uri(&file))] {
            let target = NativeOpenTarget::from_arg(&argument).unwrap();
            assert_eq!(std::path::Path::new(target.path()), file.as_path());
            assert_eq!(
                target.destinations,
                [
                    RemoteDestination::Page { page: 1, view: DestView::Top },
                    RemoteDestination::Named("chapter&two#x+z".into()),
                    RemoteDestination::Page { page: 2, view: DestView::Top },
                ]
            );
        }
        let target = NativeOpenTarget::from_arg(&format!("{}#nameddest=caf%C3%A9%2520", uri(&file))).unwrap();
        assert_eq!(target.destinations, [RemoteDestination::Named("café%20".into())], "decode exactly once");
        let localhost = uri(&file).replacen("file:///", "file://LOCALHOST/", 1);
        assert_eq!(std::path::Path::new(NativeOpenTarget::from_arg(&localhost).unwrap().path()), file.as_path());
    }

    #[test]
    fn native_launch_keeps_malformed_percent_plus_names_literal() {
        let dir = Directory::new();
        let file = dir.file("manual.pdf");
        for escape in ["%+5", "%+F", "%+0"] {
            let name = format!("50{escape}off");
            let target = NativeOpenTarget::from_arg(&format!("{}#nameddest={name}", uri(&file))).unwrap();
            assert_eq!(target.destinations(), [RemoteDestination::Named(name)], "only two ASCII hex digits form an escape");
        }
        for escape in ["%2B", "%2b"] {
            let target = NativeOpenTarget::from_arg(&format!("{}#nameddest=50{escape}5off", uri(&file))).unwrap();
            assert_eq!(target.destinations(), [RemoteDestination::Named("50+5off".into())], "uppercase and lowercase hex remain valid");
        }
    }

    #[test]
    fn native_file_uris_keep_malformed_percent_plus_filenames_literal() {
        let dir = Directory::new();
        for escape in ["%+1", "%+F", "%+0"] {
            let file = dir.file(&format!("manual{escape}.pdf"));
            let malformed_uri = uri(&file).replace("%25", "%");
            let target = NativeOpenTarget::from_arg(&malformed_uri).unwrap();
            assert_eq!(std::path::Path::new(target.path()), file.as_path());
            assert!(target.destinations().is_empty());
        }
    }

    #[test]
    fn invalid_and_oversized_navigation_is_refused_without_remote_file_access() {
        let dir = Directory::new();
        let file = dir.file("manual.pdf");
        for fragment in
            ["page=0", "page=-1", "page=1.5", "page=NaN", "page=99999999999999999999999999999", "nameddest=", "nameddest=%FF", "nameddest=%00"]
        {
            assert!(NativeOpenTarget::from_arg(&format!("{}#{fragment}", uri(&file))).is_err(), "{fragment}");
        }
        assert!(NativeOpenTarget::from_arg("file://example.invalid/manual.pdf#page=2").is_err());
        assert!(NativeOpenTarget::from_arg(&format!("{}#nameddest={}", uri(&file), "x".repeat(MAX_NAME_BYTES + 1))).is_err());
        assert!(NativeOpenTarget::from_arg(&format!("{}#{}", uri(&file), vec!["page=1"; MAX_DESTINATIONS + 1].join("&"))).is_err());
        assert!(NativeOpenTarget::from_arg(&"x".repeat(MAX_ARGUMENT_BYTES + 1)).is_err());
        assert!(NativeOpenTarget::from_arg("bad\0path.pdf").is_err());
        let escaped = NativeOpenTarget::from_arg("file:///tmp/%zz.pdf").unwrap();
        assert_eq!(std::path::Path::new(escaped.path()), std::path::absolute("/tmp/%zz.pdf").unwrap().as_path());
    }
    #[test]
    fn native_hash_filenames_choose_the_longest_existing_file_before_navigation() {
        let dir = Directory::new();
        let short = dir.file("manual.pdf");
        for name in ["manual.pdf#draft.pdf", "manual.pdf#nameddest=literal.pdf"] {
            let long = dir.file(name);
            let target = NativeOpenTarget::from_arg(&format!("{}#page=2", long.display())).unwrap();
            assert_eq!(std::path::Path::new(target.path()), long.as_path());
            assert_ne!(std::path::Path::new(target.path()), short.as_path());
        }
    }
    #[test]
    fn native_navigation_absolute_round_trips_preserve_filename_and_value_escaping() {
        let dir = Directory::new();
        let file = dir.file("manual#draft%20.pdf");
        let original = NativeOpenTarget::from_arg(&format!("{}#nameddest=section//part/../caf%C3%A9+%2520", file.display())).unwrap();
        let received = NativeOpenTarget::from_arg(&original.absolute_argument().unwrap()).unwrap();
        assert_eq!(std::path::Path::new(received.path()), file.as_path());
        assert_eq!(received.destinations(), original.destinations());
    }

    #[test]
    fn native_canonical_literals_stay_literal_after_the_file_disappears() {
        let dir = Directory::new();
        dir.file("manual.pdf");
        let literal = dir.file("manual.pdf#nameddest=chapterTwo.pdf");
        let target = NativeOpenTarget::from_arg(literal.to_str().unwrap()).unwrap();
        std::fs::remove_file(&literal).unwrap();
        let received = NativeOpenTarget::from_arg(&target.absolute_argument().unwrap()).unwrap();
        assert_eq!(received.path(), literal.to_str().unwrap());
        assert!(received.destinations().is_empty());
    }

    #[test]
    fn native_semantic_and_wire_limits_accept_the_maximum_canonical_target() {
        let names = vec![format!("nameddest={}", "+".repeat(1024)); MAX_DESTINATIONS].join("&");
        let target = NativeOpenTarget::from_arg(&format!("manual.pdf#{names}")).unwrap();
        let received = NativeOpenTarget::from_arg(&target.absolute_argument().unwrap()).unwrap();
        assert_eq!(received.destinations(), target.destinations());
        assert!(std::path::Path::new(received.path()).is_absolute());
        let oversized = format!("file:///manual.pdf#ignored={}", "x".repeat(MAX_FRAGMENT_BYTES + 1));
        assert!(NativeOpenTarget::from_arg(&oversized).is_err());
        let oversized_wire = format!("file:///manual.pdf#ignored={}", "%".repeat(MAX_ENCODED_FRAGMENT_BYTES));
        assert!(NativeOpenTarget::from_arg(&oversized_wire).is_err());
        assert!(NativeOpenTarget::from_arg(&format!("file:///{}.pdf", "x".repeat(MAX_PATH_BYTES))).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn native_navigation_absolute_round_trips_windows_drive_unc_and_verbatim_paths() {
        for path in [r"C:\docs\manual.pdf", r"\\server\share\manual.pdf", r"\\?\C:\docs\manual.pdf", r"\\?\UNC\server\share\manual.pdf"] {
            let original = NativeOpenTarget {
                path: path.to_string(),
                destinations: vec![RemoteDestination::Named("section/../target+%20".into())],
                has_parameters: true,
            };
            let received = NativeOpenTarget::from_arg(&original.absolute_argument().unwrap()).unwrap();
            assert_eq!(std::path::Path::new(received.path()), std::path::Path::new(path));
            assert_eq!(received.destinations(), original.destinations());
        }
    }
    #[test]
    fn native_canonical_navigation_keeps_accepted_semantic_fragment_limits() {
        let dir = Directory::new();
        let file = dir.file("manual.pdf");
        let parameters = vec![format!("nameddest={}", "+".repeat(1000)); 32].join("&");
        assert!(parameters.len() < 64 * 1024);
        let original = NativeOpenTarget::from_arg(&format!("{}#{parameters}", file.display())).unwrap();
        let encoded = original.absolute_argument().unwrap();
        let received = NativeOpenTarget::from_arg(&encoded).expect("canonical encoding must remain accepted by the receiver");
        assert_eq!(received.path(), original.path());
        assert_eq!(received.destinations(), original.destinations());
    }
}
