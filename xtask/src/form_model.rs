//! Prepare (never publish) the separately distributed form detection model release.
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn package(args: &[String]) -> Result<()> {
    let input = args.first().context("usage: cargo xtask form-model-package MODEL.onnx [OUTPUT_DIR]")?;
    if args.len() > 2 {
        bail!("usage: cargo xtask form-model-package MODEL.onnx [OUTPUT_DIR]");
    }
    let root = crate::assets::root();
    let manifest = crate::assets::load(&root)?;
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
    let out = args.get(1).map(std::path::PathBuf::from).unwrap_or_else(|| root.join("dist/models-ffdetr-576-v1"));
    std::fs::create_dir_all(&out)?;
    let notice = format!(
        "{}\nBy {}\nSource: {}\nLicence: {} (LICENSE-APACHE)\nModified: converted to self-contained 576px float32 ONNX, opset 17.\n\nRelease asset: {}\nSHA-256: {}\n",
        artifact.title, artifact.author, artifact.source, artifact.licence, artifact.url, artifact.sha256
    );
    publish(&out.join(&artifact.file), &bytes)?;
    let licence = std::fs::read(root.join(artifact.licence_file.as_deref().context("optional model must declare its local licence file")?))?;
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
