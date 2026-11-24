use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=MIP_SDK_ROOT");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_MIP_SDK_ROOT");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_PURVIEW_APP_ID");
    println!("cargo:rerun-if-env-changed=CXX");
    println!("cargo:rerun-if-changed=src/purview_mip.cc");

    if let Ok(app_id) = std::env::var("PDFCRAFT_PURVIEW_APP_ID") {
        println!("cargo:rustc-env=PDFCRAFT_PURVIEW_APP_ID={app_id}");
    }

    if std::env::var_os("CARGO_FEATURE_MIP_SDK").is_none() {
        println!("cargo:rustc-env=PDFCRAFT_PURVIEW_SDK_READY=0");
        return;
    }

    let out_dir = match std::env::var("OUT_DIR") {
        Ok(path) => path,
        Err(_) => {
            println!("cargo:warning=PDFCRAFT_PURVIEW: OUT_DIR is missing");
            std::process::exit(1);
        }
    };
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target = if target_os == "windows" { "pdfcraft-purview-helper.exe" } else { "pdfcraft-purview-helper" };
    let helper = PathBuf::from(out_dir).join(target);

    let compiler = std::env::var("CXX").unwrap_or_else(|_| if target_os == "windows" { "cl".into() } else { "c++".into() });
    let sdk = match std::env::var("MIP_SDK_ROOT").or_else(|_| std::env::var("PDFCRAFT_MIP_SDK_ROOT")) {
        Ok(path) => PathBuf::from(path),
        Err(_) => {
            println!("cargo:warning=PDFCRAFT_PURVIEW: MIP_SDK_ROOT or PDFCRAFT_MIP_SDK_ROOT is required with the mip-sdk feature");
            std::process::exit(1);
        }
    };
    if !sdk.join("include").is_dir() {
        println!("cargo:warning=PDFCRAFT_PURVIEW: official MIP SDK include directory is missing at {}", sdk.join("include").display());
        std::process::exit(1);
    }
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "x86_64".into());
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let sdk_arch = match (os.as_str(), arch.as_str()) {
        ("macos", "aarch64") => "arm64",
        ("macos", "x86_64") => "x86_64",
        ("macos", _) => {
            println!("cargo:warning=PDFCRAFT_PURVIEW: unsupported macOS target architecture: {arch}");
            std::process::exit(1);
        }
        (_, "x86_64") => "amd64",
        (_, "aarch64") => "arm64",
        (_, _) => arch.as_str(),
    };
    let bins = sdk.join("bins").join("release").join(sdk_arch);
    if !bins.is_dir() {
        println!("cargo:warning=PDFCRAFT_PURVIEW: official MIP SDK binary directory is missing at {}", bins.display());
        std::process::exit(1);
    }
    let mut command = Command::new(compiler);
    if os == "windows" {
        command
            .arg("/nologo")
            .arg("/std:c++17")
            .arg("/DPDFCRAFT_MIP_SDK=1")
            .arg(format!("/I{}", sdk.join("include").display()))
            .arg("src/purview_mip.cc")
            .arg("/link")
            .arg(format!("/LIBPATH:{}", bins.display()))
            .arg("mip_core.lib")
            .arg("mip_file_sdk.lib")
            .arg("mip_protection_sdk.lib")
            .arg("mip_upe_sdk.lib")
            .arg(format!("/OUT:{}", helper.display()));
    } else {
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
            command.arg("-arch").arg(sdk_arch);
            command.arg("-Wl,-rpath,@executable_path/../Frameworks");
            command.arg("-Wl,-rpath,@loader_path");
        } else {
            command.arg("-Wl,-rpath,$ORIGIN");
        }
    }
    let status = match command.status() {
        Ok(status) => status,
        Err(error) => {
            println!("cargo:warning=PDFCRAFT_PURVIEW: couldn't launch the MIP helper compiler: {error}");
            std::process::exit(1);
        }
    };
    if !status.success() {
        println!("cargo:warning=PDFCRAFT_PURVIEW: pdfcraft-purview-helper did not compile");
        std::process::exit(1);
    }

    println!("cargo:rustc-env=PDFCRAFT_PURVIEW_SDK_READY=1");
    println!("cargo:rustc-env=PDFCRAFT_PURVIEW_HELPER={}", helper.display());
}
