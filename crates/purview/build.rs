use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=MIP_SDK_ROOT");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_MIP_SDK_ROOT");
    println!("cargo:rerun-if-changed=src/purview_mip.cc");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target = if target_os == "windows" { "pdfcraft-purview-helper.exe" } else { "pdfcraft-purview-helper" };
    let helper = PathBuf::from(out_dir).join(target);

    let compiler = std::env::var("CXX").unwrap_or_else(|_| if cfg!(target_os = "windows") { "cl".into() } else { "c++".into() });
    let sdk = std::env::var("MIP_SDK_ROOT").or_else(|_| std::env::var("PDFCRAFT_MIP_SDK_ROOT")).ok();

    let mip_feature = std::env::var("CARGO_FEATURE_MIP_SDK").is_ok();
    let sdk_ready = mip_feature && sdk.is_some();
    if mip_feature && let Some(sdk) = sdk {
        let sdk = PathBuf::from(sdk);
        let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "x86_64".into());
        let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
        let sdk_arch = match (os.as_str(), arch.as_str()) {
            ("macos", "aarch64") => "arm64",
            ("macos", _) => "x86_64",
            (_, "x86_64") => "amd64",
            (_, "aarch64") => "arm64",
            (_, _) => arch.as_str(),
        };
        let bins = sdk.join("bins").join("release").join(sdk_arch);
        let mut command = Command::new(compiler);
        command
            .arg("-std=c++17")
            .arg("-DPDFCRAFT_MIP_SDK=1")
            .arg(format!("-I{}", sdk.join("include").display()))
            .arg("src/purview_mip.cc")
            .arg(format!("-L{}", bins.display()))
            .arg("-lmip_core")
            .arg("-lmip_file_sdk")
            .arg("-lmip_protection_sdk")
            .arg("-lmip_upe_sdk")
            .arg("-o")
            .arg(&helper);
        if os == "macos" {
            command.arg(format!("-Wl,-rpath,{}", bins.display()));
        }
        let status = command.status().expect("couldn't compile pdfcraft-purview-helper");
        if !status.success() {
            panic!("pdfcraft-purview-helper did not compile");
        }
    } else {
        let mut command = Command::new(compiler);
        command.arg("-std=c++17").arg("src/purview_mip.cc").arg("-o").arg(&helper);
        let status = command.status().expect("couldn't compile pdfcraft-purview-helper");
        if !status.success() {
            panic!("pdfcraft-purview-helper did not compile");
        }
    }

    println!("cargo:rustc-env=PDFCRAFT_PURVIEW_SDK_READY={}", if sdk_ready { "1" } else { "0" });
    println!("cargo:rustc-env=PDFCRAFT_PURVIEW_HELPER={}", helper.display());
}
