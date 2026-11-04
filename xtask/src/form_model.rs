//! Prepare (never publish) the separately distributed form detection model release.
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn package(args: &[String]) -> Result<()> {
    package_at(&crate::assets::root(), args)
}

fn package_at(root: &Path, args: &[String]) -> Result<()> {
    let input = args.first().context("usage: cargo xtask form-model-package MODEL.onnx [OUTPUT_DIR]")?;
    if args.len() > 2 {
        bail!("usage: cargo xtask form-model-package MODEL.onnx [OUTPUT_DIR]");
    }
    let manifest = crate::assets::load(root)?;
    let artifact =
        manifest.fetched.iter().find(|f| f.kind == "optional-model" && f.file == "ffdetr-576.onnx").context("missing optional model attribution")?;
    let metadata = std::fs::metadata(input)?;
    if !metadata.is_file() || metadata.len() > 256 * 1024 * 1024 {
        bail!("expected a regular ONNX file no larger than 256 MiB");
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(input)?.take(256 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 256 * 1024 * 1024 {
        bail!("model exceeds 256 MiB");
    }
    if hex(&Sha256::digest(&bytes)) != artifact.sha256 {
        bail!("model SHA-256 does not match ATTRIBUTION.toml; refusing to prepare a release");
    }
    let licence = std::fs::read(root.join(artifact.licence_file.as_deref().context("optional model must declare its local licence file")?))?;
    if hex(&Sha256::digest(&licence)) != artifact.licence_sha256 {
        bail!("licence SHA-256 does not match ATTRIBUTION.toml; refusing to prepare a release");
    }
    let out = args.get(1).map(std::path::PathBuf::from).unwrap_or_else(|| root.join("dist/models-ffdetr-576-v1"));
    std::fs::create_dir_all(&out)?;
    let notice = format!(
        "{}\nBy {}\nSource: {}\nLicence: {} (LICENSE-APACHE)\nModified: converted to self-contained 576px float32 ONNX, opset 17.\n\nRelease asset: {}\nSHA-256: {}\n",
        artifact.title, artifact.author, artifact.source, artifact.licence, artifact.url, artifact.sha256
    );
    publish(&out.join(&artifact.file), &bytes)?;
    publish(&out.join("LICENSE-APACHE"), &licence)?;
    publish(&out.join("ATTRIBUTION.txt"), notice.as_bytes())?;
    let sums = format!(
        "{}  {}\n{}  LICENSE-APACHE\n{}  ATTRIBUTION.txt\n",
        artifact.sha256,
        artifact.file,
        hex(&Sha256::digest(&licence)),
        hex(&Sha256::digest(notice.as_bytes()))
    );
    publish(&out.join("SHA256SUMS"), sums.as_bytes())?;
    println!("Verified release files prepared in {}. Nothing was uploaded or published.", out.display());
    println!(
        "Publish these on GitHub release models-ffdetr-576-v1 as pre-release, not latest (--prerelease --latest=false), before releasing the download UI."
    );
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    if path.exists() {
        use std::io::Read;
        let mut existing = Vec::new();
        std::fs::File::open(path)?.take(bytes.len() as u64 + 1).read_to_end(&mut existing)?;
        if existing == bytes {
            return Ok(());
        }
        bail!("{} already exists with different content; choose an empty output directory", path.display());
    }
    let directory = path.parent().context("release asset needs an output directory")?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaging_rejects_changed_licence_before_writing_any_release_files() {
        let root = tempfile::tempdir().unwrap();
        let model = b"synthetic contributor-original model";
        let licence = b"synthetic contributor-original licence notice";
        let model_path = root.path().join("input.onnx");
        let licence_path = root.path().join("LICENSE-APACHE");
        let output = root.path().join("release");
        std::fs::write(&model_path, model).unwrap();
        std::fs::write(&licence_path, b"changed licence").unwrap();
        let manifest = format!(
            r#"[[fetched]]
file = "ffdetr-576.onnx"
url = "https://example.invalid/ffdetr-576.onnx"
title = "Synthetic model"
author = "PdfCraft contributors"
source = "https://example.invalid/source"
licence = "Apache-2.0"
licence_url = "https://example.invalid/LICENSE-APACHE"
licence_file = "LICENSE-APACHE"
licence_sha256 = "{}"
kind = "optional-model"
usage = "Packaging regression fixture"
sha256 = "{}"
"#,
            hex(&Sha256::digest(licence)),
            hex(&Sha256::digest(model))
        );
        std::fs::write(root.path().join("ATTRIBUTION.toml"), manifest).unwrap();
        let args = [model_path.to_string_lossy().into_owned(), output.to_string_lossy().into_owned()];
        let error = package_at(root.path(), &args).unwrap_err();
        assert!(error.to_string().contains("licence SHA-256"), "{error:#}");
        assert!(!output.exists(), "no release output is created before licence verification");

        std::fs::write(&licence_path, licence).unwrap();
        package_at(root.path(), &args).unwrap();
        package_at(root.path(), &args).unwrap();
        assert_eq!(std::fs::read(output.join("ffdetr-576.onnx")).unwrap(), model);
        assert_eq!(std::fs::read(output.join("LICENSE-APACHE")).unwrap(), licence);
        let sums = std::fs::read_to_string(output.join("SHA256SUMS")).unwrap();
        for name in ["ffdetr-576.onnx", "LICENSE-APACHE", "ATTRIBUTION.txt"] {
            let hash = hex(&Sha256::digest(std::fs::read(output.join(name)).unwrap()));
            assert!(sums.contains(&format!("{hash}  {name}\n")));
        }
    }
}
