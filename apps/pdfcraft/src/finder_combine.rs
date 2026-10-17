//! macOS: "Combine files in PdfCraft" in Finder's Quick Actions (right-click ▸ Quick Actions),
//! the Mac half of the file manager's "Combine files" entry (#585; Explorer and Linux file
//! managers start `pdfcraft --combine` with the files).
//!
//! Finder hands a service its files through Apple events, not arguments, and a Services
//! provider needs Objective-C classes (`unsafe`, which this workspace forbids); `fmv_macos_events`
//! delivers only Open and Quit. So the app installs an Automator Quick Action in
//! `~/Library/Services` whose shell step writes the selected paths, one a line, to
//! `files.pdfcraft-combine` in a fresh `pdfcraft-combine.*` folder in the user's temporary folder
//! and opens that file with this app (`open -b`). The open arrives as an ordinary Apple event,
//! whether or not the app is running, and `from_open` turns it into the same `Combine` event a
//! Windows or Linux launch makes. Nothing is combined until Combine is pressed.
//!
//! The list is trusted only where the Quick Action writes it: that exact file name, in a
//! `pdfcraft-combine.*` folder directly inside the temporary folder (canonical paths compared), at
//! most a megabyte; it is removed once read. Any other file opens as a document, as before.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::path::{Path, PathBuf};

use pdfcraft_ui_egui::OsEvent;

/// The list file's name, and the start of its folder's name.
const LIST: &str = "files.pdfcraft-combine";
const DIR: &str = "pdfcraft-combine.";
/// The longest list read: far more than any selection's paths.
const MAX_LIST: u64 = 1 << 20;
/// The Quick Action's name in Finder's menu and on disk.
pub const NAME: &str = "Combine files in PdfCraft";

/// An opened Finder list (in the temporary folder) becomes a `Combine` of its files, and the
/// list is removed; anything else opens as before.
pub fn from_open(event: OsEvent) -> Vec<OsEvent> {
    let OsEvent::Open(paths) = event else { return vec![event] };
    let mut out: Vec<OsEvent> = Vec::new();
    for path in paths {
        match listed(Path::new(&path), &std::env::temp_dir()) {
            Some(files) if !files.is_empty() => match out.last_mut() {
                Some(OsEvent::Combine(list)) => list.extend(files),
                _ => out.push(OsEvent::Combine(files)),
            },
            // An empty or unreadable list combines nothing and opens nothing.
            Some(_) => {}
            None => match out.last_mut() {
                Some(OsEvent::Open(list)) => list.push(path),
                _ => out.push(OsEvent::Open(vec![path])),
            },
        }
    }
    out
}

/// The files a Quick Action list names, when `path` is one: called `files.pdfcraft-combine`,
/// in a `pdfcraft-combine.*` folder directly in `temp`. The list and its folder are removed.
/// `None` for any other file (it opens as usual).
fn listed(path: &Path, temp: &Path) -> Option<Vec<String>> {
    if path.file_name()? != LIST {
        return None;
    }
    let dir = path.parent()?;
    if !dir.file_name()?.to_str()?.starts_with(DIR) {
        return None;
    }
    let canon = |p: &Path| std::fs::canonicalize(p).ok();
    if canon(dir.parent()?)? != canon(temp)? {
        return None;
    }
    // Neither the folder nor the list may be a link: what the Quick Action writes is a real
    // folder holding a real file, so a link cannot stand in for either.
    if std::fs::symlink_metadata(dir).ok()?.file_type().is_symlink() {
        return None;
    }
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() {
        return None;
    }
    let text = if meta.is_file() && meta.len() <= MAX_LIST { std::fs::read_to_string(path).unwrap_or_default() } else { String::new() };
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir(dir);
    Some(text.lines().map(str::trim_end).filter(|l| !l.is_empty()).map(str::to_string).collect())
}

/// The Quick Action's folder: `~/Library/Services/<NAME>.workflow`.
fn workflow_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Services").join(format!("{NAME}.workflow")))
}

/// Write the Quick Action (when it is missing or differs) for the app `bundle_id`, and let
/// the services list know. Failures are logged; the app works without it.
///
/// macOS lists a newly installed Quick Action only under Quick Actions ▸ Customize until the
/// person ticks it once; the app does not tick it for them (Finder keeps that choice as a
/// boolean in the `pbs` defaults, which `defaults write` can't nest, and rewriting the whole
/// services record for one checkbox would be too much). The Combine files tab says where it is.
pub fn install(bundle_id: &str) {
    let Some(dir) = workflow_dir() else { return };
    match install_in(&dir, bundle_id) {
        Ok(false) => {}
        Ok(true) => {
            // Finder's menu picks the new service up on its own too; this only hurries it.
            let _ = std::process::Command::new("/System/Library/CoreServices/pbs").arg("-update").spawn();
            log::info!("Finder Quick Action installed: {}", dir.display());
        }
        Err(e) => log::warn!("Finder Quick Action not installed ({}): {e}", dir.display()),
    }
}

/// Write the workflow's two files into `dir` (a `….workflow` folder) unless they are already
/// these; `Ok(true)` when something was written.
fn install_in(dir: &Path, bundle_id: &str) -> std::io::Result<bool> {
    let contents = dir.join("Contents");
    let files = [(contents.join("Info.plist"), info_plist()), (contents.join("document.wflow"), document(bundle_id))];
    if files.iter().all(|(p, text)| std::fs::read_to_string(p).ok().as_deref() == Some(text.as_str())) {
        return Ok(false);
    }
    std::fs::create_dir_all(&contents)?;
    for (p, text) in &files {
        std::fs::write(p, text)?;
    }
    Ok(true)
}

/// The file types the Quick Action is offered for: what Combine files reads (PDFs and
/// `pdfcraft_engine::CONVERTIBLE`), as Uniform Type Identifiers. Not `public.image`, which
/// would offer it for HEIC and other pictures Combine can't read.
const FILE_TYPES: [(&str, &[&str]); 8] = [
    ("com.adobe.pdf", &["pdf"]),
    ("public.png", &["png"]),
    ("public.jpeg", &["jpg", "jpeg"]),
    ("public.tiff", &["tif", "tiff"]),
    ("com.compuserve.gif", &["gif"]),
    ("com.microsoft.bmp", &["bmp"]),
    ("public.jpeg-2000", &["jp2", "j2k", "jpx"]),
    ("public.plain-text", &["txt", "text"]),
];

/// The shell step: the selected paths into a fresh list, opened with the app.
fn script(bundle_id: &str) -> String {
    format!(
        "d=$(mktemp -d \"${{TMPDIR:-/tmp}}/{DIR}XXXXXX\") || exit 1\nfor p in \"$@\"; do printf '%s\\n' \"$p\"; done > \"$d/{LIST}\"\nopen -b {bundle_id} \"$d/{LIST}\"\n"
    )
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>NSServices</key>
	<array>
		<dict>
			<key>NSBackgroundColorName</key>
			<string>background</string>
			<key>NSIconName</key>
			<string>NSActionTemplate</string>
			<key>NSMenuItem</key>
			<dict>
				<key>default</key>
				<string>{}</string>
			</dict>
			<key>NSMessage</key>
			<string>runWorkflowAsService</string>
			<key>NSRequiredContext</key>
			<dict>
				<key>NSApplicationIdentifier</key>
				<string>com.apple.finder</string>
			</dict>
			<key>NSSendFileTypes</key>
			<array>
{}
			</array>
		</dict>
	</array>
</dict>
</plist>
"#,
        esc(NAME),
        FILE_TYPES.iter().map(|(uti, _)| format!("\t\t\t\t<string>{uti}</string>")).collect::<Vec<_>>().join("\n")
    )
}

/// Automator's document: one Run Shell Script step taking the files as arguments, a Quick
/// Action for files in Finder.
fn document(bundle_id: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AMApplicationBuild</key>
	<string>534</string>
	<key>AMApplicationVersion</key>
	<string>2.10</string>
	<key>AMDocumentVersion</key>
	<string>2</string>
	<key>actions</key>
	<array>
		<dict>
			<key>action</key>
			<dict>
				<key>AMAccepts</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Optional</key>
					<true/>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>AMActionVersion</key>
				<string>2.0.3</string>
				<key>AMApplication</key>
				<array>
					<string>Automator</string>
				</array>
				<key>AMParameterProperties</key>
				<dict>
					<key>COMMAND_STRING</key>
					<dict/>
					<key>CheckedForUserDefaultShell</key>
					<dict/>
					<key>inputMethod</key>
					<dict/>
					<key>shell</key>
					<dict/>
					<key>source</key>
					<dict/>
				</dict>
				<key>AMProvides</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>ActionBundlePath</key>
				<string>/System/Library/Automator/Run Shell Script.action</string>
				<key>ActionName</key>
				<string>Run Shell Script</string>
				<key>ActionParameters</key>
				<dict>
					<key>COMMAND_STRING</key>
					<string>{}</string>
					<key>CheckedForUserDefaultShell</key>
					<true/>
					<key>inputMethod</key>
					<integer>1</integer>
					<key>shell</key>
					<string>/bin/zsh</string>
					<key>source</key>
					<string></string>
				</dict>
				<key>BundleIdentifier</key>
				<string>com.apple.RunShellScript</string>
				<key>CFBundleVersion</key>
				<string>2.0.3</string>
				<key>CanShowSelectedItemsWhenRun</key>
				<false/>
				<key>CanShowWhenRun</key>
				<true/>
				<key>Category</key>
				<array>
					<string>AMCategoryUtilities</string>
				</array>
				<key>Class Name</key>
				<string>RunShellScriptAction</string>
				<key>InputUUID</key>
				<string>5B1E3C1A-7F0E-4E4B-9C3B-2D6A7F1C0A01</string>
				<key>OutputUUID</key>
				<string>5B1E3C1A-7F0E-4E4B-9C3B-2D6A7F1C0A02</string>
				<key>UUID</key>
				<string>5B1E3C1A-7F0E-4E4B-9C3B-2D6A7F1C0A03</string>
				<key>UnlocalizedApplications</key>
				<array>
					<string>Automator</string>
				</array>
				<key>isViewVisible</key>
				<integer>1</integer>
			</dict>
			<key>isViewVisible</key>
			<integer>1</integer>
		</dict>
	</array>
	<key>connectors</key>
	<dict/>
	<key>workflowMetaData</key>
	<dict>
		<key>applicationBundleID</key>
		<string>com.apple.finder</string>
		<key>applicationPath</key>
		<string>/System/Library/CoreServices/Finder.app</string>
		<key>inputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject</string>
		<key>outputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>presentationMode</key>
		<integer>15</integer>
		<key>processesInput</key>
		<false/>
		<key>serviceApplicationBundleID</key>
		<string>com.apple.finder</string>
		<key>serviceApplicationPath</key>
		<string>/System/Library/CoreServices/Finder.app</string>
		<key>serviceInputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject</string>
		<key>serviceOutputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>serviceProcessesInput</key>
		<false/>
		<key>systemImageName</key>
		<string>NSActionTemplate</string>
		<key>useAutomaticInputType</key>
		<false/>
		<key>workflowTypeIdentifier</key>
		<string>com.apple.Automator.servicesMenu</string>
	</dict>
</dict>
</plist>
"#,
        esc(&script(bundle_id))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list_in(temp: &Path, dir: &str, name: &str, text: &str) -> PathBuf {
        let d = temp.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join(name);
        std::fs::write(&f, text).unwrap();
        f
    }

    #[test]
    fn a_finder_list_in_the_temporary_folder_names_the_files_to_combine_and_is_removed() {
        let temp = std::env::temp_dir().join(format!("pdfcraft-finder-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let f = list_in(&temp, "pdfcraft-combine.AbC123", LIST, "/a/b.pdf\n/c/photo.png\n\n/d/a b.pdf\n");
        assert_eq!(listed(&f, &temp), Some(vec!["/a/b.pdf".into(), "/c/photo.png".into(), "/d/a b.pdf".into()]));
        assert!(!f.exists() && !f.parent().unwrap().exists(), "the list and its folder are removed");
        // Anything else is an ordinary file to open, and is left alone.
        for (dir, name) in [("pdfcraft-combine.x", "other.pdf"), ("elsewhere", LIST)] {
            let g = list_in(&temp, dir, name, "/a.pdf\n");
            assert_eq!(listed(&g, &temp), None, "{dir}/{name}");
            assert!(g.exists());
        }
        // Outside the temporary folder: not a list.
        let other = temp.join("not-temp");
        let h = list_in(&other, "pdfcraft-combine.x", LIST, "/a.pdf\n");
        assert_eq!(listed(&h, &temp), None);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn opened_lists_become_one_combine_and_other_files_still_open() {
        let temp = std::env::temp_dir();
        let d = tempdir_in(&temp);
        let list = d.join(LIST);
        std::fs::write(&list, "/x/1.pdf\n/x/2.jpg\n").unwrap();
        let events = from_open(OsEvent::Open(vec!["/y/report.pdf".into(), list.to_string_lossy().into_owned()]));
        assert_eq!(events, vec![OsEvent::Open(vec!["/y/report.pdf".into()]), OsEvent::Combine(vec!["/x/1.pdf".into(), "/x/2.jpg".into()])]);
        assert_eq!(from_open(OsEvent::Quit), vec![OsEvent::Quit]);
    }

    fn tempdir_in(temp: &Path) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = temp.join(format!("{DIR}{}-{n}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The Quick Action is offered for exactly what Combine files reads: PDFs and every
    /// extension Create converts, each under one type.
    #[test]
    fn the_quick_action_offers_exactly_the_files_combine_reads() {
        let mut offered: Vec<&str> = FILE_TYPES.iter().flat_map(|(_, exts)| exts.iter().copied()).collect();
        offered.sort_unstable();
        let mut reads: Vec<&str> = std::iter::once("pdf").chain(pdfcraft_engine::CONVERTIBLE.iter().copied()).collect();
        reads.sort_unstable();
        assert_eq!(offered, reads);
        let info = info_plist();
        for (uti, _) in FILE_TYPES {
            assert!(info.contains(&format!("<string>{uti}</string>")), "{uti}");
        }
    }

    /// A list longer than `MAX_LIST`, or a folder where the list should be, combines nothing
    /// (an oversized list is still cleared away).
    #[test]
    fn an_oversized_list_or_a_folder_in_its_place_combines_nothing() {
        let temp = std::env::temp_dir();
        let big = tempdir_in(&temp).join(LIST);
        std::fs::write(&big, "/a.pdf\n".repeat(MAX_LIST as usize / 7 + 1)).unwrap();
        assert!(std::fs::metadata(&big).unwrap().len() > MAX_LIST);
        assert_eq!(listed(&big, &temp), Some(vec![]));
        assert!(!big.exists(), "the oversized list is removed");
        let d = tempdir_in(&temp);
        std::fs::create_dir_all(d.join(LIST)).unwrap();
        assert_eq!(listed(&d.join(LIST), &temp), Some(vec![]));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A link to a folder or to a list is not a list: the Quick Action writes neither, and
    /// nothing a link points at is touched.
    #[cfg(unix)]
    #[test]
    fn a_linked_folder_or_list_is_not_a_list() {
        let temp = std::env::temp_dir();
        let real = tempdir_in(&temp);
        std::fs::write(real.join(LIST), "/a.pdf\n").unwrap();
        let folder_link = temp.join(format!("{DIR}{}-folder-link", std::process::id()));
        let _ = std::fs::remove_file(&folder_link);
        std::os::unix::fs::symlink(&real, &folder_link).unwrap();
        assert_eq!(listed(&folder_link.join(LIST), &temp), None);
        let elsewhere = tempdir_in(&temp);
        let other = elsewhere.join("other.txt");
        std::fs::write(&other, "/b.pdf\n").unwrap();
        let file_dir = tempdir_in(&temp);
        let file_link = file_dir.join(LIST);
        std::os::unix::fs::symlink(&other, &file_link).unwrap();
        assert_eq!(listed(&file_link, &temp), None);
        assert!(real.join(LIST).exists() && other.exists(), "nothing the links point at is touched");
        let _ = std::fs::remove_file(&folder_link);
        for d in [&real, &elsewhere, &file_dir] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// Installing writes the workflow once, rewrites it when it differs, and leaves it alone
    /// when it is already this version.
    #[test]
    fn installing_writes_the_workflow_only_when_it_differs() {
        let root = std::env::temp_dir().join(format!("pdfcraft-install-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join(format!("{NAME}.workflow"));
        assert!(install_in(&dir, "com.example.app").unwrap(), "a first install writes");
        assert!(dir.join("Contents/Info.plist").is_file() && dir.join("Contents/document.wflow").is_file());
        assert!(!install_in(&dir, "com.example.app").unwrap(), "the same version writes nothing");
        assert!(install_in(&dir, "com.example.other").unwrap(), "another bundle id rewrites it");
        assert!(std::fs::read_to_string(dir.join("Contents/document.wflow")).unwrap().contains("open -b com.example.other"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The Quick Action's two files are property lists macOS reads, and its step writes a
    /// list `from_open` takes (run with zsh, as Automator does).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_quick_action_is_a_valid_workflow_whose_step_writes_a_list_this_app_reads() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-workflow-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in [("Info.plist", info_plist()), ("document.wflow", document("com.example.app"))] {
            let p = dir.join(name);
            std::fs::write(&p, text).unwrap();
            let lint = std::process::Command::new("plutil").arg("-lint").arg(&p).output().unwrap();
            assert!(lint.status.success(), "{name}: {}", String::from_utf8_lossy(&lint.stdout));
        }
        let info = std::fs::read_to_string(dir.join("Info.plist")).unwrap();
        assert!(info.contains("<string>Combine files in PdfCraft</string>") && info.contains("com.adobe.pdf") && !info.contains("public.image"));
        // The step, with `open` replaced by printing the list's path.
        let step = script("com.example.app").replace("open -b com.example.app", "printf %s");
        let out = std::process::Command::new("/bin/zsh").arg("-c").arg(&step).arg("zsh").arg("/p/one.pdf").arg("/p/two words.png").output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let list = PathBuf::from(String::from_utf8(out.stdout).unwrap());
        assert_eq!(listed(&list, &std::env::temp_dir()), Some(vec!["/p/one.pdf".into(), "/p/two words.png".into()]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
