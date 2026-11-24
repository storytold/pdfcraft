use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::policy::{Checked, MAX_PATH_BYTES, MAX_PATHS};
use crate::{Canary, PROBE_CHILD_FLAG, Policy, SandboxError, handle_args};

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn bad_policy<T: std::fmt::Debug>(r: crate::Result<T>) -> String {
    match r {
        Err(SandboxError::BadPolicy(why)) => why,
        other => panic!("expected a bad-policy error, got {other:?}"),
    }
}

/// A directory of its own with a file in it, outside the system's temporary directory's canary.
fn scratch() -> (Canary, PathBuf) {
    let canary = Canary::create().unwrap();
    let dir =
        std::env::temp_dir().canonicalize().unwrap().join(format!("gridcraft-sandbox-test-{}-{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("data.txt"), "data").unwrap();
    (canary, dir)
}

#[test]
fn a_checked_policy_is_canonical_and_executes_the_launcher_and_the_program() {
    let (canary, dir) = scratch();
    let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
    let file = dir.join("data.txt");
    let checked = Policy::new().read(&dir).read(&dir).exec(&file).checked(&exe, &file, &canary).unwrap();
    assert_eq!(checked.read, vec![dir.clone()], "duplicates collapse");
    assert_eq!(checked.exec, vec![file.clone(), exe.clone()], "the launcher joins the program");
    assert_eq!(checked.program(), file);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn what_a_policy_refuses() {
    let (canary, dir) = scratch();
    let exe = std::env::current_exe().unwrap();
    let ok = dir.join("data.txt");
    let check = |p: Policy| bad_policy(p.checked(&exe, &ok, &canary));
    assert!(check(Policy::new().read("relative/path")).contains("absolute"));
    assert!(check(Policy::new().read("")).contains("absolute"));
    assert!(check(Policy::new().read(dir.join("missing"))).contains("missing"));
    assert!(check(Policy::new().read(PathBuf::from(format!("/{}", "a".repeat(MAX_PATH_BYTES))))).contains("longer"));
    let mut many = Policy::new();
    for _ in 0..=MAX_PATHS {
        many = many.read(&dir);
    }
    assert!(check(many).contains("more than"));
    // The canary must stay outside every allowed path, or the check would read it.
    let around = canary.file().parent().unwrap().to_path_buf();
    assert!(check(Policy::new().read(around)).contains("canary"));
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let line_break = PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/a\nb"));
        assert!(check(Policy::new().read(line_break)).contains("line break"));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_launcher_reads_its_arguments_back() {
    let (canary, dir) = scratch();
    let exe = std::env::current_exe().unwrap();
    let program = dir.join("data.txt");
    let checked = Policy::new().read(&dir).checked(&exe, &program, &canary).unwrap();
    let mut args = checked.to_args(canary.file());
    args.push("--".into());
    args.push(program.clone().into());
    args.extend(os(&["-c", "print(1)"]));
    let (back, probe) = Checked::from_args(&args).unwrap();
    assert_eq!(probe, canary.file().canonicalize().unwrap());
    assert_eq!((back.read.clone(), back.exec.clone()), (checked.read.clone(), checked.exec.clone()));
    assert_eq!(back.args(), os(&["-c", "print(1)"]).as_slice());

    let p = program.to_string_lossy().into_owned();
    let c = canary.file().to_string_lossy().into_owned();
    // No canary, a program the policy doesn't execute, an unknown option, a missing program.
    assert!(bad_policy(Checked::from_args(&os(&["--exec", &p, "--", &p]))).contains("canary"));
    assert!(bad_policy(Checked::from_args(&os(&["--probe", &c, "--", &p]))).contains("not an executable path"));
    assert!(bad_policy(Checked::from_args(&os(&["--write", &p, "--probe", &c, "--", &p]))).contains("unknown"));
    assert!(bad_policy(Checked::from_args(&os(&["--exec", &p, "--probe", &c, "--"]))).contains("no program"));
    assert!(bad_policy(Checked::from_args(&os(&["--exec", &p, "--probe", &c]))).contains("no program"));
    assert!(bad_policy(Checked::from_args(&os(&["--exec"]))).contains("needs a path"));
    // A second canary isn't taken.
    assert!(bad_policy(Checked::from_args(&os(&["--exec", &p, "--probe", &c, "--probe", &c, "--", &p]))).contains("unknown"));
    // The enforcement check starts the current executable, so a hand-built list must let it.
    assert!(bad_policy(Checked::from_args(&os(&["--exec", &p, "--probe", &c, "--", &p]))).contains("current executable"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn only_the_sandbox_flags_are_handled() {
    assert_eq!(handle_args(&[]), None);
    assert_eq!(handle_args(&os(&["--control", "7979"])), None);
    assert_eq!(handle_args(&os(&[PROBE_CHILD_FLAG])), Some(0));
}

/// A launch whose arguments are refused runs nothing and leaves the process's descriptors alone:
/// closing them here would pull files out from under the tests running beside this one.
#[test]
fn a_launch_refused_for_its_arguments_closes_nothing() {
    use std::io::Read as _;
    let (_canary, dir) = scratch();
    let mut held = std::fs::File::open(dir.join("data.txt")).unwrap();
    assert_eq!(handle_args(&os(&[crate::LAUNCH_FLAG, "--", "/bin/sh"])), Some(crate::REFUSED_EXIT_CODE));
    let mut text = String::new();
    held.read_to_string(&mut text).unwrap();
    assert_eq!(text, "data");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_canary_is_a_fresh_file_removed_on_drop() {
    let a = Canary::create().unwrap();
    let b = Canary::create().unwrap();
    assert_ne!(a.file(), b.file());
    assert!(a.file().is_file());
    let dir = a.file().parent().unwrap().to_path_buf();
    drop(a);
    assert!(!dir.exists());
    assert!(Path::new(b.file()).exists());
}

#[cfg(target_os = "macos")]
#[test]
fn a_directory_of_programs_is_a_subpath_in_both_of_its_rules() {
    let (canary, dir) = scratch();
    let exe = std::env::current_exe().unwrap();
    let checked = Policy::new().exec(&dir).checked(&exe, &dir.join("data.txt"), &canary).unwrap();
    let profile = crate::macos::profile(&checked).unwrap();
    let rule = format!("(subpath \"{}\")", dir.display());
    assert_eq!(profile.matches(&rule).count(), 2, "{profile}");
    assert!(!profile.contains(&format!("(literal \"{}\")", dir.display())), "{profile}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn profile_strings_are_escaped() {
    assert_eq!(crate::macos::quoted(Path::new("/a/b c")).unwrap(), "\"/a/b c\"");
    assert_eq!(crate::macos::quoted(Path::new("/a\"b\\c")).unwrap(), "\"/a\\\"b\\\\c\"");
    assert!(crate::macos::quoted(Path::new("/a\tb")).is_err(), "a control character is refused");
    let (canary, dir) = scratch();
    let exe = std::env::current_exe().unwrap();
    let checked = Policy::new().read(&dir).checked(&exe, &dir.join("data.txt"), &canary).unwrap();
    let profile = crate::macos::profile(&checked).unwrap();
    assert!(profile.starts_with("(version 1)\n(deny default)\n"));
    assert!(profile.contains(&format!("(subpath \"{}\")", dir.display())));
    for absent in ["process-fork", "network", "file-write*"] {
        assert!(!profile.contains(absent), "no rule allows {absent}");
    }
    std::fs::remove_dir_all(dir).unwrap();
}
