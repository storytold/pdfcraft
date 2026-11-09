use std::{
    borrow::Cow,
    env,
    error::Error,
    ffi::OsStr,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

use flate2::read::GzDecoder;
use reqwest::blocking::Response;
use sigstore_trust_root::{TrustedRoot, SIGSTORE_PRODUCTION_TRUSTED_ROOT};
use sigstore_types::Bundle;
use sigstore_verify::{verify, VerificationPolicy};
use tar::{Archive, EntryType};

const DEFAULT_PDFIUM_BUILD_ID: &str = "7802";
const PDFIUM_ATTESTATION_NAME: &str = "pdfium-attestation.json";
const PDFIUM_CERTIFICATE_ISSUER: &str = "https://token.actions.githubusercontent.com";
const VERIFIED_MARKER_FILE: &str = ".sigstore-verified";

#[allow(dead_code)]
fn try_link_pdfium() -> Result<(), Box<dyn Error>> {
    let bin_ext = [
        OsStr::new("dll"),
        OsStr::new("lib"),
        OsStr::new("so"),
        OsStr::new("a"),
    ];
    let build_id = env::var("WINPRINT_PDFIUM_BUILD_ID")
        .unwrap_or_else(|_| DEFAULT_PDFIUM_BUILD_ID.to_string());
    let mut target_path = PathBuf::from(env::var("OUT_DIR")?);
    target_path.push(format!("pdfium_binaries_{}", build_id));
    fs::create_dir_all(target_path.as_path())?;

    let has_bin = fs::read_dir(target_path.as_path())?.any(|x| {
        bin_ext.iter().any(|s| {
            x.as_ref()
                .ok()
                .and_then(|d| {
                    if Path::new(d.file_name().as_os_str()).extension() == Some(s) {
                        Some(())
                    } else {
                        None
                    }
                })
                .is_some()
        })
    });
    let verified_marker = target_path.join(VERIFIED_MARKER_FILE);

    if !has_bin || !verified_marker.exists() {
        let platform_name = match (
            env::var("CARGO_CFG_TARGET_OS")?.as_str(),
            env::var("CARGO_CFG_TARGET_ARCH")?.as_str(),
        ) {
            ("windows", "x86") => "win-x86",
            ("windows", "x86_64") => "win-x64",
            ("windows", "aarch64") => "win-arm64",
            _ => return Err("Unsupported target arch".into()),
        };

        let binary_package_name = format!("pdfium-{}.tgz", platform_name);
        let binary_package_url = release_asset_url(&build_id, &binary_package_name);
        let binary_package = download_asset(binary_package_url.as_str())?;

        let attestation_url = release_asset_url(&build_id, PDFIUM_ATTESTATION_NAME);
        let attestation = download_asset(attestation_url.as_str())?;

        verify_pdfium_attestation(binary_package.as_slice(), attestation.as_slice())?;

        fs::remove_dir_all(target_path.as_path())?;
        fs::create_dir_all(target_path.as_path())?;
        unpack_pdfium_archive(binary_package.as_slice(), target_path.as_path(), &bin_ext)?;
        fs::write(
            verified_marker,
            format!(
                "Verified {} with {}\n",
                binary_package_name, PDFIUM_ATTESTATION_NAME
            ),
        )?;
    }

    println!(
        "cargo:rustc-link-search=native={}",
        target_path.to_str().unwrap()
    );
    println!("cargo:rustc-link-lib=dylib=pdfium");
    Ok(())
}

fn release_asset_url(build_id: &str, file_name: &str) -> String {
    format!(
        "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/{}/{}",
        build_id, file_name
    )
}

fn download_asset(url: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let resp = reqwest::blocking::get(url)?;
    ensure_success(resp, url)
}

fn ensure_success(resp: Response, url: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    if !resp.status().is_success() {
        return Err(format!("Failed to download asset from {}", url).into());
    }
    Ok(resp.bytes()?.to_vec())
}

fn verify_pdfium_attestation(
    binary_package: &[u8],
    attestation_json: &[u8],
) -> Result<(), Box<dyn Error>> {
    let bundle = Bundle::from_json(std::str::from_utf8(attestation_json)?)?;
    let trusted_root = TrustedRoot::from_json(SIGSTORE_PRODUCTION_TRUSTED_ROOT)?;
    let policy = VerificationPolicy::default().require_issuer(PDFIUM_CERTIFICATE_ISSUER);

    verify(binary_package, &bundle, &policy, &trusted_root)
        .map_err(|err| format!("Failed to verify PDFium Sigstore attestation: {err}"))?;

    Ok(())
}

fn unpack_pdfium_archive(
    archive_bytes: &[u8],
    target_path: &Path,
    bin_ext: &[&OsStr],
) -> Result<(), Box<dyn Error>> {
    let tar = GzDecoder::new(Cursor::new(archive_bytes));
    let mut archive = Archive::new(tar);

    for entry in archive.entries()? {
        let mut file = entry?;
        if file.header().entry_type() == EntryType::Regular {
            let file_path = file.path()?;
            let file_extension = file_path.extension();
            let is_bin = bin_ext.iter().any(|s| file_extension == Some(s));
            if is_bin {
                if let Some(file_name_raw) = file_path.file_name() {
                    let mut file_name_str = file_name_raw.to_string_lossy();
                    if let Some(s) = file_name_str.strip_suffix(".dll.lib") {
                        file_name_str = Cow::Owned(s.to_owned() + ".lib");
                    }
                    let output_path = target_path.join(file_name_str.as_ref());
                    file.unpack(output_path.as_path())?;
                }
            }
        }
    }

    Ok(())
}

fn main() {
    println!("cargo:rerun-if-env-changed=WINPRINT_PDFIUM_BUILD_ID");
    // Skip downloading native libraries on docs.rs
    if std::env::var("DOCS_RS").is_ok() {
        return;
    }
    let target_windows = env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_windows == "windows" {
        #[cfg(feature = "pdfium")]
        try_link_pdfium().unwrap();
    }
}
