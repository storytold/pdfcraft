//! Try the assistant on a PDF without the window:
//!
//! `cargo run -p pdfcraft-assistant --example ask -- <file.pdf> <model> [question]`
//!
//! Without a question it summarises. Uses Ollama on 127.0.0.1:11434
//! as the app does. Prints the page origins, the verified answer and what was refused.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use pdfcraft_assistant::{Endpoint, Settings, Task, pages, run};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(path), Some(model)) = (args.first(), args.get(1)) else {
        eprintln!("usage: ask <file.pdf> <model> [question]");
        std::process::exit(2);
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };
    let doc = match pages::read_document(Arc::new(bytes), None, &|| false) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    for p in &doc.pages {
        println!("p.{}: {} ({} caracteres)", p.page + 1, p.origin.label(), p.text.chars().count());
    }
    let task = match args.get(2) {
        Some(q) => Task::Question(q.clone()),
        None => Task::Summary,
    };
    let ep = Endpoint { model: model.clone(), ..Endpoint::default() };
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    // ASK_CANCEL_AFTER=<seconds>: cancel from another thread, as the panel's Cancel button does.
    if let Some(secs) = std::env::var("ASK_CANCEL_AFTER").ok().and_then(|v| v.parse::<u64>().ok()) {
        let c = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            eprintln!("… cancelling");
            c.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }
    let started = std::time::Instant::now();
    let result = run(&ep, &doc, &task, &Settings::default(), &cancel, |s| eprintln!("… {s:?}"));
    eprintln!("… returned after {:.1}s", started.elapsed().as_secs_f64());
    match result {
        Err(e) => {
            println!("ERROR: {e}");
            std::process::exit(1);
        }
        Ok(o) => {
            println!(
                "\nenviadas: {:?} parciales: {:?} omitidas: {:?} ilegibles: {:?} | espera {:.1}s, respuesta {:.1}s, local={}",
                o.prompt.included.iter().map(|p| p + 1).collect::<Vec<_>>(),
                o.prompt.partial.iter().map(|p| p + 1).collect::<Vec<_>>(),
                o.prompt.omitted.iter().map(|p| p + 1).collect::<Vec<_>>(),
                o.prompt.unreadable.iter().map(|p| p + 1).collect::<Vec<_>>(),
                o.queue_seconds,
                o.answer_seconds,
                o.local
            );
            if o.answer.not_found {
                println!("El modelo dice que el texto enviado no contiene la respuesta.");
            }
            if let Some(u) = &o.answer.unparsed {
                println!("Respuesta ilegible: {u}");
            }
            for (i, pt) in o.answer.points.iter().enumerate() {
                let tag = if pt.supported() { "respaldado" } else { "SIN CITA VERIFICADA" };
                println!("\n{}. [{tag}] {}", i + 1, pt.statement);
                for q in &pt.quotes {
                    let moved = q.claimed_page.map(|c| format!(" (el modelo indicó p. {c})")).unwrap_or_default();
                    println!("   p. {} · {}{moved}: «{}»", q.page + 1, q.origin.label(), q.text);
                }
            }
            for r in &o.answer.rejected {
                println!("\nDESCARTADA (p. {:?}): «{}» — {}", r.claimed_page, r.text, r.reason);
            }
        }
    }
}
