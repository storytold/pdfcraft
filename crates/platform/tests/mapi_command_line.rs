//! The Windows Simple MAPI route runs PowerShell with the script on its command line, and Windows
//! refuses a command line over 32 767 characters. The message's values (the body can be long) must
//! therefore travel on the helper's standard input, never on the command line.

use std::path::PathBuf;

use pdfcraft_platform::mail::{Compose, DefaultMail, Route, plan_for};

const WINDOWS_LIMIT: usize = 32_767;

fn run_plan(body: String) -> (usize, Option<String>) {
    let compose =
        Compose { path: PathBuf::from(r"C:\Temp\Invoice 26340.pdf"), subject: "Invoice 26340".into(), body, to: vec!["ap@example.org".into()] };
    let plan = plan_for("windows", &DefaultMail::Mapi("Mozilla Thunderbird".into()), &compose);
    match plan.map(|p| p.route) {
        // The whole line: the program, then each argument and the space before it (quotes aside).
        Ok(Route::Run { command, args, stdin, .. }) => {
            let line = command.to_string_lossy().chars().count() + args.iter().map(|a| a.chars().count() + 1).sum::<usize>();
            (line, stdin)
        }
        other => panic!("not a run: {other:?}"),
    }
}

#[test]
fn a_long_body_stays_under_the_windows_limit() {
    for (name, body) in [("ASCII", "a".repeat(50_000)), ("CJK", "漢".repeat(50_000))] {
        let (line, stdin) = run_plan(body.clone());
        assert!(line < WINDOWS_LIMIT, "{name}: the command line is {line} characters");

        // The values arrive on standard input: five base64 lines (path, name, subject, body,
        // recipients), the body's among them whole.
        let input = stdin.unwrap_or_default();
        let lines: Vec<&str> = input.lines().collect();
        assert_eq!(lines.len(), 5, "{name}: {} lines on standard input", lines.len());
        assert!(
            input.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '\n')),
            "{name}: standard input is base64, one value to a line"
        );
        // Base64 grows 3 bytes to 4 characters: the 4th line holds the whole body.
        let bytes = body.len();
        assert_eq!(lines[3].len(), bytes.div_ceil(3) * 4, "{name}: the body is whole on its line");
    }
}

#[test]
fn the_command_line_does_not_depend_on_the_message() {
    let (short, _) = run_plan(String::new());
    let (long, _) = run_plan("x".repeat(50_000));
    assert_eq!(short, long);
}
