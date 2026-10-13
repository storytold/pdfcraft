//! Mobile build, baseline, and packaging tasks for PdfCraft.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::gates::{cargo, release_cli, root, run};

/// Entry point for `cargo xtask mobile-baseline`.
pub fn baseline(args: &[String]) -> anyhow::Result<()> {
    let root_dir = root();
    let out_json = flag(args, "--out").map(PathBuf::from).unwrap_or_else(|| root_dir.join("mobile/benchmarks/baseline/host.json"));
    let target = flag(args, "--target");
    let device = flag(args, "--device");
    let tile_sizes = flag(args, "--tile-sizes").unwrap_or("256,512,1024");

    // Ensure mobile/fixtures directory exists and generate synthetic fixture if needed
    let fixtures_dir = root_dir.join("mobile/fixtures");
    std::fs::create_dir_all(&fixtures_dir).context("creating mobile/fixtures")?;
    let synthetic_path = fixtures_dir.join("typical-10p.pdf");
    if !synthetic_path.exists() {
        let pdf_bytes = synthetic_pdf(10);
        std::fs::write(&synthetic_path, pdf_bytes).context("writing synthetic fixture")?;
    }

    if let Some(target_triple) = target {
        if target_triple.contains("android") {
            return run_android_baseline(&root_dir, target_triple, device, tile_sizes, &out_json);
        } else if target_triple.contains("ios") && target_triple.contains("sim") {
            return run_ios_sim_baseline(&root_dir, target_triple, tile_sizes, &out_json);
        } else {
            bail!("unsupported target for mobile-baseline: {target_triple}");
        }
    }

    // Default: run on host using release pdfcraft-cli
    println!("Building release pdfcraft-cli on host...");
    let mut build = cargo();
    build.args(["build", "--release", "-p", "pdfcraft-cli"]);
    run(build, "cargo build --release -p pdfcraft-cli")?;

    let cli = release_cli();
    if !cli.exists() {
        bail!("expected CLI binary at {}", cli.display());
    }

    let mut cmd = Command::new(cli);
    cmd.arg("bench").arg("--tile-sizes").arg(tile_sizes).arg("--json").arg(&out_json).arg(&synthetic_path);

    // If corpus is present, add a selection of representative PDFs
    let corpus_dir = root_dir.join("corpus/pdfjs/test/pdfs");
    if corpus_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&corpus_dir)
    {
        for entry in entries.flatten().take(5) {
            let p = entry.path();
            if p.extension().is_some_and(|e| e == "pdf") {
                cmd.arg(p);
            }
        }
    }

    println!("Running host baseline benchmark...");
    run(cmd, "pdfcraft-cli bench")?;

    println!("\nBaseline saved to: {}", out_json.display());
    Ok(())
}

fn run_android_baseline(root_dir: &Path, target: &str, device: Option<&str>, tile_sizes: &str, out_json: &Path) -> anyhow::Result<()> {
    let abi = if target.contains("x86_64") { "x86_64" } else { "arm64-v8a" };
    println!("Cross-compiling pdfcraft-cli for Android ({abi})...");

    let mut build = Command::new("cargo");
    build.current_dir(root_dir);
    build.args(["ndk", "-t", abi, "build", "--release", "-p", "pdfcraft-cli"]);
    if let Ok(ndk) = std::env::var("ANDROID_NDK_HOME") {
        build.env("ANDROID_NDK_HOME", ndk);
    }
    run(build, "cargo ndk build --release -p pdfcraft-cli")?;

    let target_bin = root_dir.join("target").join(target).join("release").join("pdfcraft-cli");

    if !target_bin.exists() {
        bail!("expected Android binary at {}", target_bin.display());
    }

    // Check if adb is available
    let adb = Command::new("adb").arg("version").output();
    if adb.is_err() {
        bail!("adb not found; install Android platform-tools to run on device");
    }

    let mut adb_base = Command::new("adb");
    if let Some(dev) = device {
        adb_base.args(["-s", dev]);
    }

    println!("Pushing binary and fixture to device via adb...");
    let remote_dir = "/data/local/tmp/pdfcraft";
    let mut mkdir = clone_cmd(&adb_base);
    mkdir.args(["shell", "mkdir", "-p", remote_dir]);
    run(mkdir, "adb shell mkdir")?;

    let mut push_bin = clone_cmd(&adb_base);
    push_bin.args(["push", target_bin.to_str().unwrap_or(""), remote_dir]);
    run(push_bin, "adb push pdfcraft-cli")?;

    let mut chmod = clone_cmd(&adb_base);
    chmod.args(["shell", "chmod", "755", &format!("{remote_dir}/pdfcraft-cli")]);
    run(chmod, "adb shell chmod")?;

    let remote_fixture = format!("{remote_dir}/typical-10p.pdf");
    let local_fixture = root_dir.join("mobile/fixtures/typical-10p.pdf");
    let mut push_fix = clone_cmd(&adb_base);
    push_fix.args(["push", local_fixture.to_str().unwrap_or(""), &remote_fixture]);
    run(push_fix, "adb push fixture")?;

    println!("Executing benchmark on Android device...");
    let remote_json = format!("{remote_dir}/out.json");
    let mut bench_run = clone_cmd(&adb_base);
    bench_run.args(["shell", &format!("{remote_dir}/pdfcraft-cli"), "bench", &remote_fixture, "--tile-sizes", tile_sizes, "--json", &remote_json]);
    run(bench_run, "adb shell pdfcraft-cli bench")?;

    println!("Pulling baseline results...");
    if let Some(parent) = out_json.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut pull_json = clone_cmd(&adb_base);
    pull_json.args(["pull", &remote_json, out_json.to_str().unwrap_or("")]);
    run(pull_json, "adb pull out.json")?;

    println!("Android baseline saved to: {}", out_json.display());
    Ok(())
}

fn run_ios_sim_baseline(root_dir: &Path, target: &str, tile_sizes: &str, out_json: &Path) -> anyhow::Result<()> {
    println!("Cross-compiling pdfcraft-cli for iOS Simulator ({target})...");
    let mut build = cargo();
    build.args(["build", "--release", "-p", "pdfcraft-cli", "--target", target]);
    run(build, &format!("cargo build --release -p pdfcraft-cli --target {target}"))?;

    let target_bin = root_dir.join("target").join(target).join("release").join("pdfcraft-cli");

    if !target_bin.exists() {
        bail!("expected iOS Simulator binary at {}", target_bin.display());
    }

    let fixture = root_dir.join("mobile/fixtures/typical-10p.pdf");

    println!("Executing benchmark on booted iOS simulator via xcrun simctl spawn...");
    let mut cmd = Command::new("xcrun");
    cmd.args([
        "simctl",
        "spawn",
        "booted",
        target_bin.to_str().unwrap_or(""),
        "bench",
        fixture.to_str().unwrap_or(""),
        "--tile-sizes",
        tile_sizes,
        "--json",
        out_json.to_str().unwrap_or(""),
    ]);
    run(cmd, "xcrun simctl spawn booted pdfcraft-cli bench")?;

    println!("iOS Simulator baseline saved to: {}", out_json.display());
    Ok(())
}

fn clone_cmd(cmd: &Command) -> Command {
    let mut new_cmd = Command::new(cmd.get_program());
    for arg in cmd.get_args() {
        new_cmd.arg(arg);
    }
    new_cmd
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn synthetic_pdf(n: usize) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 595 842] >>", kids.join(" ")).into_bytes());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i).into_bytes());
        let body = format!("BT /F1 14 Tf 50 800 Td (PdfCraft Mobile Baseline Document - Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Entry point for `cargo xtask mobile-bindings`.
pub fn bindings(args: &[String]) -> anyhow::Result<()> {
    let check = args.iter().any(|a| a == "--check");
    let root_dir = root();

    println!("Building pdfcraft-ffi library...");
    let mut build = cargo();
    build.args(["build", "-p", "pdfcraft-ffi"]);
    run(build, "cargo build -p pdfcraft-ffi")?;

    let lib_name = if cfg!(target_os = "macos") {
        "libpdfcraft_ffi.dylib"
    } else if cfg!(target_os = "windows") {
        "pdfcraft_ffi.dll"
    } else {
        "libpdfcraft_ffi.so"
    };

    let lib_path = root_dir.join("target/debug").join(lib_name);
    if !lib_path.exists() {
        bail!("expected compiled library at {}", lib_path.display());
    }

    let out_dir = root_dir.join("target/mobile-bindings");
    let swift_out = out_dir.join("swift");
    let kotlin_out = out_dir.join("kotlin");
    std::fs::create_dir_all(&swift_out).context("creating swift out dir")?;
    std::fs::create_dir_all(&kotlin_out).context("creating kotlin out dir")?;

    println!("Generating Swift bindings...");
    let mut swift_cmd = cargo();
    swift_cmd.args(["run", "-p", "pdfcraft-ffi", "--features", "cli", "--bin", "uniffi-bindgen", "--", "generate", "--library"]);
    swift_cmd.arg(&lib_path);
    swift_cmd.args(["--language", "swift", "--out-dir"]);
    swift_cmd.arg(&swift_out);
    run(swift_cmd, "generate swift bindings")?;

    println!("Generating Kotlin bindings...");
    let mut kt_cmd = cargo();
    kt_cmd.args(["run", "-p", "pdfcraft-ffi", "--features", "cli", "--bin", "uniffi-bindgen", "--", "generate", "--library"]);
    kt_cmd.arg(&lib_path);
    kt_cmd.args(["--language", "kotlin", "--no-format", "--out-dir"]);
    kt_cmd.arg(&kotlin_out);
    run(kt_cmd, "generate kotlin bindings")?;

    let swift_file = swift_out.join("pdfcraft_ffi.swift");
    let kt_file = kotlin_out.join("uniffi/pdfcraft_ffi/pdfcraft_ffi.kt");

    if !swift_file.exists() {
        bail!("Swift bindings file missing at {}", swift_file.display());
    }
    if !kt_file.exists() {
        bail!("Kotlin bindings file missing at {}", kt_file.display());
    }

    let swift_src = std::fs::read_to_string(&swift_file)?;
    let kt_src = std::fs::read_to_string(&kt_file)?;

    let swift_api = extract_swift_api(&swift_src);
    let kt_api = extract_kotlin_api(&kt_src);

    let api_dir = root_dir.join("crates/ffi/api");
    std::fs::create_dir_all(&api_dir).context("creating api dir")?;
    let swift_api_file = api_dir.join("swift.api");
    let kt_api_file = api_dir.join("kotlin.api");

    if check {
        println!("Checking API surface snapshots against committed api/*.api...");
        if !swift_api_file.exists() || !kt_api_file.exists() {
            bail!("Committed API surface files missing in crates/ffi/api/! Run `cargo xtask mobile-bindings` to generate them.");
        }
        let committed_swift = std::fs::read_to_string(&swift_api_file)?;
        let committed_kt = std::fs::read_to_string(&kt_api_file)?;

        if committed_swift.trim() != swift_api.trim() {
            eprintln!("--- committed crates/ffi/api/swift.api");
            eprintln!("+++ generated Swift API surface");
            bail!("Swift public API surface changed! Run `cargo xtask mobile-bindings` to update snapshots and review changes.");
        }

        if committed_kt.trim() != kt_api.trim() {
            eprintln!("--- committed crates/ffi/api/kotlin.api");
            eprintln!("+++ generated Kotlin API surface");
            bail!("Kotlin public API surface changed! Run `cargo xtask mobile-bindings` to update snapshots and review changes.");
        }

        println!("mobile-bindings: ok (Swift and Kotlin API surface matches committed snapshots)");
    } else {
        std::fs::write(&swift_api_file, format!("{}\n", swift_api.trim()))?;
        std::fs::write(&kt_api_file, format!("{}\n", kt_api.trim()))?;
        println!("Wrote API surface snapshots to:");
        println!("  {}", swift_api_file.display());
        println!("  {}", kt_api_file.display());
        println!("Generated bindings to:");
        println!("  {}", swift_out.display());
        println!("  {}", kotlin_out.display());
    }

    Ok(())
}

fn extract_swift_api(swift_src: &str) -> String {
    let mut out = Vec::new();
    let mut in_public_block = false;
    let mut block_depth: i32 = 0;

    for line in swift_src.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.is_empty() || trimmed.starts_with("import ") {
            continue;
        }
        if trimmed.contains("FfiConverter") || trimmed.contains("uniffi") {
            continue;
        }

        if trimmed.starts_with("public protocol ")
            || trimmed.starts_with("public class Engine")
            || trimmed.starts_with("public struct Ffi")
            || trimmed.starts_with("public struct MobileProfile")
            || trimmed.starts_with("public enum FfiError")
        {
            in_public_block = true;
            block_depth = 1;
            out.push(trimmed.to_string());
            continue;
        }

        if in_public_block {
            let opens = line.chars().filter(|c| *c == '{').count() as i32;
            let closes = line.chars().filter(|c| *c == '}').count() as i32;
            block_depth = block_depth + opens - closes;
            if block_depth <= 0 {
                in_public_block = false;
                out.push("}".to_string());
                out.push("".to_string());
                continue;
            }
            if trimmed.starts_with("public var ")
                || trimmed.starts_with("public init(")
                || trimmed.starts_with("func ")
                || trimmed.starts_with("case ")
            {
                out.push(format!("    {trimmed}"));
            }
        } else if trimmed.starts_with("public func ") {
            out.push(trimmed.to_string());
        }
    }
    out.join("\n")
}

fn extract_kotlin_api(kt_src: &str) -> String {
    let mut out = Vec::new();
    let mut in_public_block = false;
    let mut block_depth: i32 = 0;

    for line in kt_src.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.is_empty() || trimmed.starts_with("package ") || trimmed.starts_with("import ") {
            continue;
        }
        if trimmed.contains("FfiConverter") || trimmed.contains("Uniffi") || trimmed.contains("uniffi") {
            continue;
        }

        if trimmed.starts_with("public interface Engine")
            || trimmed.starts_with("public interface RenderCallback")
            || trimmed.starts_with("public open class Engine")
            || trimmed.starts_with("public data class Ffi")
            || trimmed.starts_with("public data class MobileProfile")
            || trimmed.starts_with("public sealed class FfiError")
            || trimmed.starts_with("public enum class ")
        {
            in_public_block = true;
            block_depth = 1;
            out.push(trimmed.to_string());
            continue;
        }

        if in_public_block {
            let opens = line.chars().filter(|c| *c == '{').count() as i32;
            let closes = line.chars().filter(|c| *c == '}').count() as i32;
            block_depth = block_depth + opens - closes;
            if block_depth <= 0 {
                in_public_block = false;
                out.push("}".to_string());
                out.push("".to_string());
                continue;
            }
            if trimmed.starts_with("fun ")
                || trimmed.starts_with("val ")
                || trimmed.starts_with("var ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("data class ")
            {
                out.push(format!("    {trimmed}"));
            }
        } else if (trimmed.starts_with("fun ") || trimmed.starts_with("public fun "))
            && (trimmed.contains("MobileProfile") || trimmed.contains("Tools") || trimmed.contains("Tool"))
        {
            out.push(trimmed.to_string());
        }
    }
    out.join("\n")
}

/// Entry point for `cargo xtask mobile-xcframework`.
pub fn xcframework(_args: &[String]) -> anyhow::Result<()> {
    let root_dir = root();
    println!("Step 1/3: Generating Swift bindings & headers...");
    bindings(&[])?;

    let swift_bindings_dir = root_dir.join("target/mobile-bindings/swift");
    let header_file = swift_bindings_dir.join("pdfcraft_ffiFFI.h");
    let modulemap_file = swift_bindings_dir.join("pdfcraft_ffiFFI.modulemap");
    if !header_file.exists() || !modulemap_file.exists() {
        bail!("expected UniFFI C headers in {}", swift_bindings_dir.display());
    }
    // Apple XCFramework expects module.modulemap in Headers
    std::fs::copy(&modulemap_file, swift_bindings_dir.join("module.modulemap")).context("copying module.modulemap")?;

    println!("Step 2/3: Compiling pdfcraft-ffi for iOS (device + simulator)...");
    let mut build_device = cargo();
    build_device.args(["build", "--release", "-p", "pdfcraft-ffi", "--target", "aarch64-apple-ios"]);
    build_device.env("IPHONEOS_DEPLOYMENT_TARGET", "17.0");
    build_device.env("RUSTFLAGS", "-C link-arg=-miphoneos-version-min=17.0");
    run(build_device, "cargo build --release --target aarch64-apple-ios")?;

    let mut build_sim = cargo();
    build_sim.args(["build", "--release", "-p", "pdfcraft-ffi", "--target", "aarch64-apple-ios-sim"]);
    build_sim.env("IPHONEOS_DEPLOYMENT_TARGET", "17.0");
    build_sim.env("RUSTFLAGS", "-C link-arg=-mios-simulator-version-min=17.0");
    run(build_sim, "cargo build --release --target aarch64-apple-ios-sim")?;

    let mut build_mac = cargo();
    build_mac.args(["build", "--release", "-p", "pdfcraft-ffi"]);
    run(build_mac, "cargo build --release -p pdfcraft-ffi")?;

    let device_lib = root_dir.join("target/aarch64-apple-ios/release/libpdfcraft_ffi.a");
    let sim_lib = root_dir.join("target/aarch64-apple-ios-sim/release/libpdfcraft_ffi.a");
    let mac_lib = root_dir.join("target/release/libpdfcraft_ffi.a");

    if !device_lib.exists() {
        bail!("expected device library at {}", device_lib.display());
    }
    if !sim_lib.exists() {
        bail!("expected simulator library at {}", sim_lib.display());
    }
    if !mac_lib.exists() {
        bail!("expected macOS host library at {}", mac_lib.display());
    }

    println!("Step 3/3: Assembling XCFramework...");
    let kit_frameworks_dir = root_dir.join("mobile/ios/PdfCraftKit/Frameworks");
    let xcframework_out = kit_frameworks_dir.join("pdfcraft_ffiFFI.xcframework");
    std::fs::create_dir_all(&kit_frameworks_dir).context("creating PdfCraftKit/Frameworks")?;
    if xcframework_out.exists() {
        std::fs::remove_dir_all(&xcframework_out).context("removing existing xcframework")?;
    }

    let mut xc_cmd = Command::new("xcodebuild");
    xc_cmd.args(["-create-xcframework", "-library"]);
    xc_cmd.arg(&device_lib);
    xc_cmd.arg("-headers");
    xc_cmd.arg(&swift_bindings_dir);
    xc_cmd.arg("-library");
    xc_cmd.arg(&sim_lib);
    xc_cmd.arg("-headers");
    xc_cmd.arg(&swift_bindings_dir);
    xc_cmd.arg("-library");
    xc_cmd.arg(&mac_lib);
    xc_cmd.arg("-headers");
    xc_cmd.arg(&swift_bindings_dir);
    xc_cmd.arg("-output");
    xc_cmd.arg(&xcframework_out);
    run(xc_cmd, "xcodebuild -create-xcframework")?;

    // Copy generated Swift bindings to PdfCraftKit
    let generated_swift_dir = root_dir.join("mobile/ios/PdfCraftKit/Sources/PdfCraftKit/Generated");
    std::fs::create_dir_all(&generated_swift_dir).context("creating Generated swift dir")?;
    let src_swift = swift_bindings_dir.join("pdfcraft_ffi.swift");
    let dst_swift = generated_swift_dir.join("pdfcraft_ffi.swift");
    std::fs::copy(&src_swift, &dst_swift).context("copying generated Swift binding")?;

    println!("\nXCFramework packaged successfully at:");
    println!("  {}", xcframework_out.display());
    println!("Swift bindings placed at:");
    println!("  {}", dst_swift.display());

    Ok(())
}

fn find_android_ndk() -> Option<PathBuf> {
    if let Ok(ndk) = std::env::var("ANDROID_NDK_HOME") {
        let p = PathBuf::from(ndk);
        if p.exists() {
            return Some(p);
        }
    }
    // Check Homebrew cask location
    let candidates = ["/opt/homebrew/Caskroom/android-ndk/30/AndroidNDK16248370.app/Contents/NDK", "/opt/homebrew/share/android-ndk"];
    for c in &candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Entry point for `cargo xtask mobile-android`.
pub fn android(_args: &[String]) -> anyhow::Result<()> {
    let root_dir = root();
    println!("Step 1/3: Generating Kotlin bindings...");
    bindings(&[])?;

    let kt_bindings_file = root_dir.join("target/mobile-bindings/kotlin/uniffi/pdfcraft_ffi/pdfcraft_ffi.kt");
    if !kt_bindings_file.exists() {
        bail!("expected generated Kotlin bindings at {}", kt_bindings_file.display());
    }

    println!("Step 2/3: Compiling pdfcraft-ffi for Android (arm64-v8a + x86_64, 16KB page-aligned)...");
    let mut build = Command::new("cargo");
    build.current_dir(&root_dir);
    build.args(["ndk", "-t", "arm64-v8a", "-t", "x86_64", "build", "--release", "-p", "pdfcraft-ffi"]);

    if let Some(ndk) = find_android_ndk() {
        build.env("ANDROID_NDK_HOME", &ndk);
    }
    // 16 KB page-size alignment
    build.env("RUSTFLAGS", "-C link-arg=-z -C link-arg=max-page-size=16384");
    run(build, "cargo ndk build --release -p pdfcraft-ffi (arm64-v8a + x86_64)")?;

    println!("Step 3/3: Copying libraries and bindings into pdfcraft-kit...");
    let kit_jni_dir = root_dir.join("mobile/android/pdfcraft-kit/src/main/jniLibs");
    let arm64_dst = kit_jni_dir.join("arm64-v8a");
    let x86_64_dst = kit_jni_dir.join("x86_64");
    std::fs::create_dir_all(&arm64_dst).context("creating jniLibs/arm64-v8a")?;
    std::fs::create_dir_all(&x86_64_dst).context("creating jniLibs/x86_64")?;

    let arm64_src = root_dir.join("target/aarch64-linux-android/release/libpdfcraft_ffi.so");
    let x86_64_src = root_dir.join("target/x86_64-linux-android/release/libpdfcraft_ffi.so");

    if !arm64_src.exists() {
        bail!("expected arm64-v8a library at {}", arm64_src.display());
    }
    if !x86_64_src.exists() {
        bail!("expected x86_64 library at {}", x86_64_src.display());
    }

    std::fs::copy(&arm64_src, arm64_dst.join("libpdfcraft_ffi.so")).context("copying arm64 .so")?;
    std::fs::copy(&x86_64_src, x86_64_dst.join("libpdfcraft_ffi.so")).context("copying x86_64 .so")?;

    // Copy Kotlin bindings
    let kit_java_dir = root_dir.join("mobile/android/pdfcraft-kit/src/main/java/uniffi/pdfcraft_ffi");
    std::fs::create_dir_all(&kit_java_dir).context("creating Java package dir")?;
    std::fs::copy(&kt_bindings_file, kit_java_dir.join("pdfcraft_ffi.kt")).context("copying Kotlin bindings")?;

    println!("\nAndroid native libraries packaged successfully into:");
    println!("  {}", kit_jni_dir.display());
    println!("Kotlin bindings placed at:");
    println!("  {}", kit_java_dir.join("pdfcraft_ffi.kt").display());

    Ok(())
}

/// Entry point for `cargo xtask mobile-size`.
/// Reports stripped binary footprint and per-feature cost of mobile libraries against budget B10.
pub fn size(args: &[String]) -> anyhow::Result<()> {
    let root_dir = root();
    let check_budget = args.iter().any(|a| a == "--check");
    let json_out = flag(args, "--json");

    println!("Measuring mobile binary footprints and feature costs against budget B10...");
    println!("Step 1/2: Building pdfcraft-ffi under mobile-release profile...");

    let mut build = cargo();
    build.args(["build", "--profile", "mobile-release", "-p", "pdfcraft-ffi"]);
    run(build, "cargo build --profile mobile-release -p pdfcraft-ffi")?;

    let release_dir = root_dir.join("target/mobile-release");
    let archive_path = release_dir.join("libpdfcraft_ffi.a");
    let dylib_path = release_dir.join("libpdfcraft_ffi.dylib");
    let so_path = release_dir.join("libpdfcraft_ffi.so");

    let binary_path = if dylib_path.exists() {
        dylib_path
    } else if so_path.exists() {
        so_path
    } else {
        bail!("neither libpdfcraft_ffi.dylib nor libpdfcraft_ffi.so found in {}", release_dir.display());
    };

    if !archive_path.exists() {
        bail!("expected static archive at {}", archive_path.display());
    }

    // Measure raw shared library size
    let raw_bytes = std::fs::metadata(&binary_path)?.len();

    // Measure stripped shared library size
    let temp_stripped = std::env::temp_dir().join("libpdfcraft_ffi_stripped_check");
    std::fs::copy(&binary_path, &temp_stripped)?;
    let _ = Command::new("strip").arg("-x").arg(&temp_stripped).status();
    let stripped_bytes = std::fs::metadata(&temp_stripped)?.len();
    let _ = std::fs::remove_file(&temp_stripped);

    let stripped_mb = stripped_bytes as f64 / (1024.0 * 1024.0);
    let raw_mb = raw_bytes as f64 / (1024.0 * 1024.0);

    println!("\nBinary footprint ({})", binary_path.file_name().unwrap_or_default().to_string_lossy());
    println!("  Raw size:      {raw_mb:.2} MB ({raw_bytes} bytes)");
    println!("  Stripped size: {stripped_mb:.2} MB ({stripped_bytes} bytes)");
    println!("  Budget B10:    ≤ 25.00 MB (All features)");

    // Per-feature cost analysis from archive
    println!("\nStep 2/2: Inspecting static archive symbols for feature breakdown...");
    let ar_output = Command::new("ar").arg("-tv").arg(&archive_path).output()?;
    let ar_text = String::from_utf8_lossy(&ar_output.stdout);

    let mut feature_sizes: std::collections::BTreeMap<&str, u64> = std::collections::BTreeMap::new();
    let mut total_archive_bytes: u64 = 0;

    for line in ar_text.lines() {
        // format: rw-r--r-- 0/0 <size> <date> <name>
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 6
            && let Ok(sz) = parts[2].parse::<u64>()
        {
            total_archive_bytes += sz;
            let obj_name = parts[parts.len() - 1];
            let feature = if obj_name.contains("pdfcraft_sign")
                || obj_name.contains("aws_lc")
                || obj_name.contains("rsa")
                || obj_name.contains("p256")
                || obj_name.contains("p384")
                || obj_name.contains("p521")
                || obj_name.contains("ed25519")
                || obj_name.contains("curve25519")
                || obj_name.contains("pbkdf2")
                || obj_name.contains("des")
                || obj_name.contains("rc2")
            {
                "sign"
            } else if obj_name.contains("pdfcraft_js") || obj_name.contains("boa_") {
                "js"
            } else if obj_name.contains("pdfcraft_ocr") || obj_name.contains("ocrs") || obj_name.contains("rten") {
                "ocr"
            } else if obj_name.contains("pdfcraft_xfa") {
                "xfa"
            } else if obj_name.contains("pdfcraft_export") {
                "export"
            } else if obj_name.contains("pdfcraft_preflight") {
                "preflight"
            } else if obj_name.contains("uniffi") {
                "ffi_bindings"
            } else {
                "viewer_core"
            };
            *feature_sizes.entry(feature).or_insert(0) += sz;
        }
    }

    println!("\nPer-feature cost breakdown (archive objects):");
    println!("{:<15} {:>12} {:>10}", "Feature", "Size", "Share");
    println!("{:-<39}", "");
    for (feat, bytes) in &feature_sizes {
        let mb = *bytes as f64 / (1024.0 * 1024.0);
        let pct = if total_archive_bytes > 0 { (*bytes as f64 / total_archive_bytes as f64) * 100.0 } else { 0.0 };
        println!("{:<15} {:>9.2} MB {:>9.1}%", feat, mb, pct);
    }
    println!("{:-<39}", "");
    let total_archive_mb = total_archive_bytes as f64 / (1024.0 * 1024.0);
    println!("{:<15} {:>9.2} MB {:>9.1}%", "Total archive", total_archive_mb, 100.0);

    // Budget B10 enforcement
    let b10_all_features_pass = stripped_mb <= 25.0;
    println!("\nBudget B10 Status: {}", if b10_all_features_pass { "PASSED" } else { "FAILED" });
    println!("  All features limit: 25.0 MB | Current: {stripped_mb:.2} MB");

    if let Some(json_path) = json_out {
        let json_data = serde_json::json!({
            "stripped_mb": stripped_mb,
            "raw_mb": raw_mb,
            "archive_mb": total_archive_mb,
            "budget_limit_mb": 25.0,
            "budget_passed": b10_all_features_pass,
            "features": feature_sizes,
        });
        std::fs::write(json_path, serde_json::to_string_pretty(&json_data)?)?;
        println!("JSON report written to: {json_path}");
    }

    if check_budget && !b10_all_features_pass {
        bail!("Budget B10 VIOLATION: stripped library ({stripped_mb:.2} MB) exceeds 25 MB limit!");
    }

    Ok(())
}
