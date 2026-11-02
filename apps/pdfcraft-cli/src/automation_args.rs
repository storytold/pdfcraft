//! Parse `--automation-read-root` / `--automation-write-root` before the server starts. Each flag may
//! be given more than once, and every folder counts. A malformed flag must not silently remove a
//! root and leave the server unconfined.

use std::path::PathBuf;

pub struct AutomationArgs {
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
}

impl AutomationArgs {
    pub fn new() -> Self {
        Self { read_roots: Vec::new(), write_roots: Vec::new() }
    }

    /// Returns whether this argument is a root flag. A flag may be repeated, and each folder is
    /// added, in either spelling (`--flag DIR`, `--flag=DIR`).
    pub fn consume(&mut self, arg: &str, args: &mut impl Iterator<Item = String>) -> Result<bool, String> {
        let (flag, inline) = arg.split_once('=').map_or((arg, None), |(flag, value)| (flag, Some(value)));
        let roots = match flag {
            "--automation-read-root" => &mut self.read_roots,
            "--automation-write-root" => &mut self.write_roots,
            _ => return Ok(false),
        };
        let value = match inline {
            Some(value) => value.to_owned(),
            None => {
                let value = args.next().ok_or_else(|| format!("{flag} needs a folder"))?;
                if value.starts_with('-') {
                    return Err(format!("{flag} needs a folder; use {flag}=<dir> for a folder starting with '-'"));
                }
                value
            }
        };
        if value.is_empty() {
            return Err(format!("{flag} needs a non-empty folder"));
        }
        roots.push(value.into());
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn roots_are_collected_and_other_arguments_are_left() {
        let mut roots = AutomationArgs::new();
        let mut args = ["a folder".to_owned(), "report.pdf".to_owned()].into_iter();
        assert!(roots.consume("--automation-read-root", &mut args).unwrap());
        assert!(roots.consume("--automation-write-root=out", &mut args).unwrap());
        assert!(!roots.consume("--compact", &mut args).unwrap());
        assert_eq!(roots.read_roots, paths(&["a folder"]));
        assert_eq!(roots.write_roots, paths(&["out"]));
        assert_eq!(args.next().as_deref(), Some("report.pdf"));
    }

    #[test]
    fn a_flag_may_be_repeated_in_either_spelling() {
        let mut roots = AutomationArgs::new();
        let mut args = ["b".to_owned(), "d".to_owned()].into_iter();
        assert!(roots.consume("--automation-read-root=a", &mut args).unwrap());
        assert!(roots.consume("--automation-read-root", &mut args).unwrap());
        assert!(roots.consume("--automation-write-root", &mut args).unwrap());
        assert!(roots.consume("--automation-write-root=e", &mut args).unwrap());
        assert_eq!(roots.read_roots, paths(&["a", "b"]));
        assert_eq!(roots.write_roots, paths(&["d", "e"]));
    }

    #[test]
    fn invalid_operands_add_no_root() {
        for flag in ["--automation-read-root", "--automation-write-root"] {
            for operand in [None, Some(""), Some("--version"), Some("--root"), Some("-folder")] {
                let mut roots = AutomationArgs::new();
                roots.read_roots.push("read".into());
                roots.write_roots.push("write".into());
                let mut args = operand.map(str::to_owned).into_iter();
                assert!(roots.consume(flag, &mut args).is_err(), "{flag} {operand:?}");
                assert_eq!(roots.read_roots, paths(&["read"]));
                assert_eq!(roots.write_roots, paths(&["write"]));
            }
            let mut roots = AutomationArgs::new();
            assert!(roots.consume(&format!("{flag}="), &mut std::iter::empty()).is_err());
        }
    }

    #[test]
    fn explicit_dash_folder_and_relative_dash_folder_are_valid() {
        let mut roots = AutomationArgs::new();
        assert!(roots.consume("--automation-read-root=-folder", &mut std::iter::empty()).unwrap());
        assert!(roots.consume("--automation-write-root", &mut ["./-folder".to_owned()].into_iter()).unwrap());
        assert_eq!(roots.read_roots, paths(&["-folder"]));
        assert_eq!(roots.write_roots, paths(&["./-folder"]));
    }
}
