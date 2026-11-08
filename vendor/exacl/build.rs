use std::env;
use std::path::Path;

#[cfg(feature = "buildtime_bindgen")]
const BINDGEN_FAILURE_MSG: &str = r"Could not generate bindings.

On Linux, the 'sys/acl.h' file is installed by the `libacl1-dev` package. To 
install this package, please use `apt-get install libacl1-dev`.

If you still have problems, please create a GitHub issue at:
https://github.com/byllyfish/exacl/issues

";

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir).join("bindings.rs");
    let wrapper = "bindgen/wrapper.h";

    // Tell cargo to tell rustc to link libacl.so, only on Linux.
    #[cfg(target_os = "linux")]
    println!("cargo:rustc-link-lib=acl");

    // Tell cargo to invalidate the built crate whenever the wrapper changes
    println!("cargo:rerun-if-changed={wrapper}");

    #[cfg(feature = "buildtime_bindgen")]
    bindgen_bindings(wrapper, &out_path);

    #[cfg(not(feature = "buildtime_bindgen"))]
    prebuilt_bindings(&out_path);
}

#[cfg(feature = "buildtime_bindgen")]
fn bindgen_bindings(wrapper: &str, out_path: &Path) {
    // Build bindings for "wrapper.h". Tell cargo to invalidate the built
    // crate when any included header file changes.
    let mut builder = bindgen::Builder::default()
        .header(wrapper)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .disable_header_comment()
        .layout_tests(false); // no layout tests for passwd/group structs.

    if cfg!(target_os = "macos") {
        // Pass output of `xcrun --sdk macosx --show-sdk-path`.
        builder = builder.clang_arg("-isysroot/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk");
    }

    // Specify the types, functions, and constants we want to include.
    let types = ["acl_.*", "uid_t", "gid_t"];
    let funcs = [
        "acl_.*",
        "getpw(nam|uid)_r",
        "getgr(nam|gid)_r",
        "mbr_uid_to_uuid",
        "mbr_gid_to_uuid",
        "mbr_uuid_to_id",
        #[cfg(target_os = "macos")]
        "open",
        #[cfg(target_os = "macos")]
        "close",
        #[cfg(target_os = "freebsd")]
        "[lf]?pathconf",
    ];
    let vars = [
        "ACL_.*",
        ".*_ACL_NFS4",
        "ENOENT",
        "ENOTSUP",
        "EINVAL",
        "ENOMEM",
        "ERANGE",
        #[cfg(target_os = "macos")]
        "O_SYMLINK",
        "ID_TYPE_UID",
        "ID_TYPE_GID",
    ];

    // FreeBSD: Specify acl functions by exact name that we want to exclude.
    // The reason is that FreeBSD 14 & 15 include additional `_np` functions
    // that we don't use.
    #[cfg(not(target_os = "freebsd"))]
    let blocked_funcs: [&str; 0] = [];
    #[cfg(target_os = "freebsd")]
    let blocked_funcs = [
        "acl_cmp_np",
        "acl_equiv_mode_np",
        "acl_extended_(file|file_nofollow|link)_np",
        "acl_from_mode_np",
    ];

    for type_ in &types {
        builder = builder.allowlist_type(type_);
    }

    for func_ in &funcs {
        builder = builder.allowlist_function(func_);
    }

    for var_ in &vars {
        builder = builder.allowlist_var(var_);
    }

    for blocked_func in &blocked_funcs {
        builder = builder.blocklist_function(blocked_func);
    }

    // Generate the bindings.
    let bindings = builder.generate().expect(BINDGEN_FAILURE_MSG);

    // Write the bindings.
    bindings
        .write_to_file(out_path)
        .expect("Couldn't write bindings!");
}

#[cfg(not(feature = "buildtime_bindgen"))]
fn prebuilt_bindings(out_path: &Path) {
    let target = env::var("CARGO_CFG_TARGET_OS").unwrap();

    // Untrusted input check.
    match target.as_str() {
        "macos" | "linux" | "freebsd" => (),
        s => panic!("Unsupported target OS: {}", s),
    };

    let bindings_path = format!("bindgen/bindings_{target}.rs");
    if let Err(err) = std::fs::copy(&bindings_path, out_path) {
        panic!("Can't copy {:?} to {:?}: {}", bindings_path, out_path, err);
    }
}
