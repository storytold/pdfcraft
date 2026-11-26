use super::*;
use std::cell::Cell;
use std::rc::Rc;

/// The desktop app's exit hook runs in `on_exit`, the one exit path macOS has (its quit ends the
/// process before `run_native` returns), and only once.
#[test]
fn leaving_runs_the_exit_hook_once() {
    let runs = Rc::new(Cell::new(0));
    let counted = Rc::clone(&runs);
    let mut app = PdfCraftApp::for_unit_tests();
    app.set_exit_hook(Box::new(move || counted.set(counted.get() + 1)));
    eframe::App::on_exit(&mut app, None);
    assert_eq!(runs.get(), 1);
    eframe::App::on_exit(&mut app, None);
    assert_eq!(runs.get(), 1, "a second exit must not run the hook again");
}

/// Without a hook the exit path still does its own work and returns: a stable build installs none.
#[test]
fn an_app_without_an_exit_hook_leaves_normally() {
    let mut app = PdfCraftApp::for_unit_tests();
    eframe::App::on_exit(&mut app, None);
}
