//! Preserve notices from the exact Gradle runtime graph, separately from Cargo notices.
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAX_ENTRY: usize = 4 * 1024 * 1024;
const MAX_ARCHIVE: usize = 64 * 1024 * 1024;
const MAX_BUNDLE: usize = 32 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    apache_license: PathBuf,
    material_notice: PathBuf,
    artifacts: Vec<Artifact>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    coordinate: String,
    file: PathBuf,
    pom: PathBuf,
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn read_limited(reader: impl Read, max: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(max.saturating_add(1) as u64).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= max, "Android notice input exceeds its size limit");
    Ok(bytes)
}

fn read_file(path: &Path, max: usize) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).with_context(|| format!("read Android notice input {}", path.display()))?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file() && metadata.len() <= max as u64, "Invalid or oversized Android notice input {}", path.display());
    read_limited(file, max)
}

#[derive(Default)]
struct Budget {
    entries: usize,
    expanded: usize,
}

fn collect(archive: &[u8], prefix: &str, depth: usize, budget: &mut Budget, notices: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
    ensure!(depth <= 2 && archive.len() <= MAX_ARCHIVE, "Android notice archive exceeds its bounds");
    let mut zip = zip::ZipArchive::new(Cursor::new(archive)).context("Invalid Android notice archive")?;
    budget.entries = budget.entries.saturating_add(zip.len());
    ensure!(budget.entries <= 50_000, "Too many Android archive entries");
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).context("Read Android archive entry")?;
        if entry.is_dir() {
            continue;
        }
        let entry_name = entry.name().context("Invalid Android archive entry name")?.into_owned();
        ensure!(entry_name.len() <= 4096, "Android archive entry name is too long");
        let leaf = entry_name.rsplit('/').next().unwrap_or_default().to_ascii_uppercase();
        let notice = ["LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE", "UNLICENSE"].iter().any(|p| leaf.starts_with(p));
        let nested = leaf.ends_with(".JAR");
        if !notice && !nested {
            continue;
        }
        let max = if nested && !notice { MAX_ARCHIVE } else { MAX_ENTRY };
        ensure!(entry.size() <= max as u64, "Oversized Android archive entry");
        // Entry names remain descriptive labels only. No archive path is extracted.
        let name = format!("{prefix}{entry_name}");
        let bytes = read_limited(&mut entry, max)?;
        budget.expanded = budget.expanded.saturating_add(bytes.len()).saturating_add(name.len());
        ensure!(budget.expanded <= 2 * MAX_ARCHIVE, "Android nested notices exceed their size limit");
        if notice {
            ensure!(notices.insert(name, bytes).is_none(), "Duplicate Android notice name");
        } else {
            collect(&bytes, &format!("{name}!/"), depth + 1, budget, notices)?;
        }
    }
    Ok(())
}

fn insert_file(files: &mut BTreeMap<String, Vec<u8>>, name: String, bytes: Vec<u8>) -> Result<()> {
    files.insert(name, bytes);
    let size = files.values().fold(0usize, |sum, bytes| sum.saturating_add(bytes.len()));
    ensure!(size <= MAX_BUNDLE, "Android notice bundle is too large");
    Ok(())
}

fn label(value: &str) -> String {
    value.replace('\r', "\\r").replace('\n', "\\n")
}

fn bundle(manifest: Manifest) -> Result<(String, BTreeMap<String, Vec<u8>>)> {
    ensure!(manifest.schema == 1, "Unsupported Android notice manifest version");
    ensure!(!manifest.artifacts.is_empty() && manifest.artifacts.len() <= 500, "Missing or excessive Android runtime artifact list");
    let mut files = BTreeMap::new();
    insert_file(&mut files, "LICENSE-APACHE-2.0.txt".into(), read_file(&manifest.apache_license, MAX_ENTRY)?)?;
    insert_file(&mut files, "Material-NOTICE.txt".into(), read_file(&manifest.material_notice, MAX_ENTRY)?)?;
    let mut report = String::from(
        "PdfCraft Android runtime dependencies\n\nResolved per build variant by Gradle. Original embedded notices and Maven POMs\n\
         are retained byte-for-byte under pdfcraft-android-licenses/. SHA-256 identifies\n\
         source archives and retained files. Apache-2.0 text is included here; Rust,\n\
         font and other frontend notices remain in THIRD-PARTY-NOTICES.txt.\n",
    );
    let mut artifacts = manifest.artifacts;
    artifacts.sort_by(|a, b| a.coordinate.cmp(&b.coordinate).then_with(|| a.file.cmp(&b.file)));
    for artifact in artifacts {
        ensure!(!artifact.coordinate.is_empty() && artifact.coordinate.len() <= 512, "Invalid Android artifact identity");
        if artifact.coordinate.starts_with("com.google.android.material:material:") {
            ensure!(
                artifact.coordinate == "com.google.android.material:material:1.13.0",
                "Update the verified Material notice for the selected version"
            );
        }
        let archive = read_file(&artifact.file, MAX_ARCHIVE)?;
        let name = artifact.file.file_name().and_then(|s| s.to_str()).context("Invalid Android archive filename")?;
        report.push_str(&format!("\n{}\nArchive: {}\nSHA-256: {}\n", label(&artifact.coordinate), label(name), sha256(&archive)));
        let pom = read_file(&artifact.pom, MAX_ENTRY)?;
        let pom_name = format!("{}.pom", sha256(&pom));
        report.push_str(&format!("Maven metadata: pdfcraft-android-licenses/{pom_name}\n"));
        insert_file(&mut files, pom_name, pom)?;
        let mut notices = BTreeMap::new();
        collect(&archive, "", 0, &mut Budget::default(), &mut notices)?;
        if notices.is_empty() {
            report.push_str("No separate license/notice file in this archive; see Maven license declarations and accompanying license texts.\n");
        }
        for (path, bytes) in notices {
            let retained = format!("{}.txt", sha256(&bytes));
            report.push_str(&format!("Original {}: pdfcraft-android-licenses/{retained}\n", label(&path)));
            ensure!(report.len() <= MAX_BUNDLE, "Android notice index is too large");
            insert_file(&mut files, retained, bytes)?;
        }
        ensure!(report.len() <= MAX_BUNDLE, "Android notice index is too large");
    }
    Ok((report, files))
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    // A generated output may be replaced, but never follow a link outside it.
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        ensure!(metadata.is_file() && !metadata.file_type().is_symlink(), "Android notice output is not a regular file");
    }
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub fn run(manifest: &Path, output: &Path) -> Result<()> {
    let manifest: Manifest = serde_json::from_slice(&read_file(manifest, MAX_ENTRY)?).context("Read Android runtime notice manifest")?;
    let (report, files) = bundle(manifest)?;
    // All inputs are verified before output writes. Only our named files are
    // replaced; no recursive deletion of a caller-selected directory occurs.
    for dir in [output.to_path_buf(), output.join("pdfcraft-android-licenses")] {
        if let Ok(metadata) = std::fs::symlink_metadata(&dir) {
            ensure!(metadata.is_dir() && !metadata.file_type().is_symlink(), "Android notice output must be a real directory");
        }
        std::fs::create_dir_all(dir)?;
    }
    for (name, bytes) in files {
        write_file(&output.join("pdfcraft-android-licenses").join(name), &bytes)?;
    }
    write_file(&output.join("ANDROID-THIRD-PARTY-NOTICES.txt"), report.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            zip.start_file(*name, SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn preserves_original_notices_and_nested_jars_without_extracting_paths() {
        let original = b"Copyright test author\r\nUnmodified notice\r\n";
        let nested = archive(&[("META-INF/NOTICE.txt", original)]);
        let bytes = archive(&[("classes.jar", &nested), ("../LICENSE", original), ("ignored.class", &[0])]);
        let mut notices = BTreeMap::new();
        collect(&bytes, "", 0, &mut Budget::default(), &mut notices).unwrap();
        assert_eq!(notices.keys().collect::<Vec<_>>(), ["../LICENSE", "classes.jar!/META-INF/NOTICE.txt"]);
        assert!(notices.values().all(|bytes| bytes == original));
    }

    #[test]
    fn rejects_invalid_oversized_and_deeply_nested_archives() {
        let mut notices = BTreeMap::new();
        assert!(collect(&[0, 1, 2, 3], "", 0, &mut Budget::default(), &mut notices).is_err());
        assert!(read_limited(&[1, 2, 3][..], 2).is_err());
        let mut bytes = archive(&[("LICENSE", b"test")]);
        for _ in 0..3 {
            bytes = archive(&[("classes.jar", &bytes)]);
        }
        assert!(collect(&bytes, "", 0, &mut Budget::default(), &mut notices).is_err());
        let oversized = archive(&[("LICENSE", &vec![0; MAX_ENTRY + 1])]);
        assert!(collect(&oversized, "", 0, &mut Budget::default(), &mut notices).is_err());
    }
}
