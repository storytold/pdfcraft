use super::*;

pub(crate) fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

#[derive(Clone, Copy)]
pub(crate) struct OptionSpec {
    pub(crate) name: &'static str,
    pub(crate) takes_value: bool,
}

/// The options whose value is a file or folder. Only these refuse a value that is another
/// option: a file named `--password` is never what was meant, while a password can be anything
/// (`--password --page` is a password of `--page`, which tests/text.rs pins), and so can a title.
/// The way to name a file that does start with `-` is `./--name`, as `--root` already offered.
const PATH_VALUE_OPTIONS: [&str; 6] = ["--out", "--out-dir", "--json", "--root", "--control", "--script"];

/// Whether a token can be the value of a path option. Another option cannot: `--out --password`
/// used to write the PDF to a file called `--password` and exit 0 (photocraft#6944).
pub(crate) fn can_be_path_value(name: &str, value: Option<&str>) -> bool {
    !PATH_VALUE_OPTIONS.contains(&name) || value.is_some_and(|v| !v.is_empty() && !v.starts_with("--"))
}

/// The complaint about a path option whose value is missing or is another option.
pub(crate) fn needs_value(command: &str, name: &str, value: Option<&str>) -> String {
    match value {
        Some(v) if !v.is_empty() => {
            format!("{command}: {name} needs a value and `{v}` is another option; write `./{v}` for a file of that name")
        }
        _ => format!("{command}: {name} needs a value"),
    }
}

/// Refuse unknown options and malformed confinement flags before any file is read.
pub(crate) fn validate_options(command: &str, args: &[String], specs: &[OptionSpec]) -> Result<(), CliError> {
    if specs.iter().any(|spec| spec.name == "--root") {
        // `flag` searches every token, including another option's value. Match that lookup so
        // a malformed confinement flag cannot be hidden there and fall back to no root.
        let mut root_seen = false;
        for (i, _) in args.iter().enumerate().filter(|(_, arg)| arg.as_str() == "--root") {
            if root_seen {
                return Err(format!("{command}: --root may only be supplied once").into());
            }
            root_seen = true;
            if args.get(i + 1).is_none_or(|value| value.is_empty() || value.starts_with("--")) {
                return Err(format!("{command}: --root needs a directory value").into());
            }
        }
    }
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if let Some(name) = arg.strip_prefix("--") {
            let Some(spec) = specs.iter().find(|spec| spec.name == arg) else {
                return Err(format!("{command}: unknown option --{name}").into());
            };
            // A file's name is not another option, which also closes the gap the walk left for
            // those: an unknown option in that value position was skipped along with it. Other
            // values, missing or malformed, are left to the command, which reports them in its
            // own terms (`text` and `extract` explain what a page value must be).
            i += if spec.takes_value {
                let value = args.get(i + 1).map(String::as_str);
                if !can_be_path_value(spec.name, value) {
                    return Err(needs_value(command, spec.name, value).into());
                }
                2
            } else {
                1
            };
        } else {
            i += 1;
        }
    }
    Ok(())
}

pub(crate) const INFO_OPTIONS: &[OptionSpec] = &[OptionSpec { name: "--password", takes_value: true }];

pub(crate) const AUDIT_OPTIONS: &[OptionSpec] = &[OptionSpec { name: "--password", takes_value: true }];

pub(crate) const TEXT_OPTIONS: &[OptionSpec] =
    &[OptionSpec { name: "--page", takes_value: true }, OptionSpec { name: "--password", takes_value: true }];

pub(crate) const COMBINE_OPTIONS: &[OptionSpec] = &[OptionSpec { name: "--out", takes_value: true }];

pub(crate) const EXTRACT_OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--out", takes_value: true },
    OptionSpec { name: "--pages", takes_value: true },
    OptionSpec { name: "--password", takes_value: true },
];

pub(crate) const SPLIT_OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--every", takes_value: true },
    OptionSpec { name: "--before", takes_value: true },
    OptionSpec { name: "--out-dir", takes_value: true },
    OptionSpec { name: "--password", takes_value: true },
];

pub(crate) const RENDER_OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--page", takes_value: true },
    OptionSpec { name: "--dpi", takes_value: true },
    OptionSpec { name: "--out", takes_value: true },
    OptionSpec { name: "--password", takes_value: true },
];

pub(crate) const CHECK_ONE_OPTIONS: &[OptionSpec] =
    &[OptionSpec { name: "--dpi", takes_value: true }, OptionSpec { name: "--edit", takes_value: false }];

pub(crate) const CHECK_OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--timeout", takes_value: true },
    OptionSpec { name: "--dpi", takes_value: true },
    OptionSpec { name: "--json", takes_value: true },
];

pub(crate) const RUN_OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--root", takes_value: true },
    OptionSpec { name: "--script", takes_value: true },
    OptionSpec { name: "--out", takes_value: true },
    OptionSpec { name: "--report", takes_value: false },
];

pub(crate) const UI_OPTIONS: &[OptionSpec] = &[OptionSpec { name: "--control", takes_value: true }, OptionSpec { name: "--out", takes_value: true }];

#[cfg(feature = "mcp")]
pub(crate) const MCP_OPTIONS: &[OptionSpec] =
    &[OptionSpec { name: "--root", takes_value: true }, OptionSpec { name: "--compact", takes_value: false }];

/// The arguments that are not options or their values. Which tokens are values is the spec's
/// answer: skipping the one after every option made `check-one --edit file.pdf` report "missing
/// file", the file having been read as `--edit`'s value.
pub(crate) fn positional<'a>(args: &'a [String], specs: &[OptionSpec]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(a) = args.get(i) {
        match specs.iter().find(|spec| spec.name == a.as_str()) {
            Some(spec) if spec.takes_value => i += 2,
            // An option that takes no value, and an unknown one, which `validate_options` has
            // already refused by the time this runs.
            Some(_) => i += 1,
            None if a.starts_with("--") => i += 1,
            None => {
                out.push(a.as_str());
                i += 1;
            }
        }
    }
    out
}
