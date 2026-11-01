//! `pdfcraft-cli.exe` carries a Windows version resource (`build.rs`), so Explorer, PowerShell's
//! `VersionInfo` and inventory tools see its version and publisher, as they do for `pdfcraft.exe`.

#![cfg(windows)]

/// `s` as UTF-16LE, the encoding of every string in a version resource.
fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A little-endian 32-bit DWORD at `at`.
fn dword(bytes: &[u8], at: usize) -> u32 {
    let b: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
    u32::from_le_bytes(b)
}

/// The value of string-table entry `key`: the key and its terminator, zero padding to the next
/// 32-bit boundary, then the value and its terminator.
fn string_value(resource: &[u8], key: &str) -> Option<String> {
    let after_key = find(resource, &utf16(&format!("{key}\0")))? + utf16(key).len() + 2;
    let start = after_key + resource[after_key..].iter().take_while(|&&b| b == 0).count();
    let units: Vec<u16> = resource[start..].as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).take_while(|&u| u != 0).collect();
    String::from_utf16(&units).ok()
}

#[test]
fn the_exe_has_version_info_with_the_package_version() {
    let exe = std::fs::read(env!("CARGO_BIN_EXE_pdfcraft-cli")).unwrap();
    let start = find(&exe, &utf16("VS_VERSION_INFO")).expect("no version resource");
    let resource = &exe[start..];
    for (key, value) in [
        ("ProductName", "PdfCraft"),
        ("FileDescription", "PdfCraft command-line tool"),
        ("LegalCopyright", "Copyright (c) the PdfCraft contributors. MIT OR Apache-2.0."),
        ("OriginalFilename", "pdfcraft-cli.exe"),
        ("InternalName", "pdfcraft-cli"),
        ("FileVersion", env!("CARGO_PKG_VERSION")),
        ("ProductVersion", env!("CARGO_PKG_VERSION")),
    ] {
        assert_eq!(string_value(resource, key).as_deref(), Some(value), "{key}");
    }
    // VS_FIXEDFILEINFO: signature, struct version, then file and product versions as
    // major << 16 | minor and patch << 16 | build.
    let fixed = start + find(resource, &0xFEEF_04BDu32.to_le_bytes()).expect("no VS_FIXEDFILEINFO");
    let number = |s: &str| -> u32 { s.parse().unwrap() };
    let ms = (number(env!("CARGO_PKG_VERSION_MAJOR")) << 16) | number(env!("CARGO_PKG_VERSION_MINOR"));
    let ls = number(env!("CARGO_PKG_VERSION_PATCH")) << 16;
    assert_eq!([dword(&exe, fixed + 8), dword(&exe, fixed + 12)], [ms, ls], "file version");
    assert_eq!([dword(&exe, fixed + 16), dword(&exe, fixed + 20)], [ms, ls], "product version");
}
