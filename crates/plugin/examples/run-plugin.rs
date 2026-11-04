//! Run one plug-in module from the command line: print its embedded
//! manifest, then invoke one command with a JSON context.
//!
//! cargo run -p pdfcraft-plugin --example run-plugin -- plugin.wasm summarize '{"document":{"name":"a.pdf","page_count":3,"current_page":0}}'

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(wasm) = args.get(1) else {
        eprintln!("usage: run-plugin <plugin.wasm> [command] [context-json]");
        return ExitCode::from(2);
    };
    let bytes = match std::fs::read(wasm) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{wasm}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let plugin = match pdfcraft_plugin::LoadedPlugin::from_bytes(bytes) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("load: {e}");
            return ExitCode::FAILURE;
        }
    };
    match plugin.manifest() {
        Ok(m) => println!(
            "manifest: {} {:?} v{} kind={} commands={}",
            m.id,
            m.name,
            m.version,
            m.kind,
            m.commands.iter().map(|c| c.id.as_str()).collect::<Vec<_>>().join(",")
        ),
        Err(e) => {
            eprintln!("manifest: {e}");
            return ExitCode::FAILURE;
        }
    }
    if let Some(command) = args.get(2) {
        let context: serde_json::Value = args.get(3).and_then(|s| serde_json::from_str(s).ok()).unwrap_or_else(|| serde_json::json!({}));
        match plugin.invoke(command, &context) {
            Ok(response) => println!("response: message={:?} actions={}", response.message, response.actions.len()),
            Err(e) => {
                eprintln!("invoke: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}
