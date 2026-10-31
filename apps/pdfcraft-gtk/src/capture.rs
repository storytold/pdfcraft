use adw::{glib, gtk, prelude::*};
use libadwaita as adw;
use std::path::Path;

pub fn window_png<W>(window: &W, path: &Path) -> Result<(), String>
where
    W: IsA<gtk::Window>,
{
    let up = window.upcast_ref::<gtk::Window>();
    capture_once(up, path)
        .map_err(|error| format!("{error} [mapped={} visible={} w={} h={}]", up.is_mapped(), up.is_visible(), up.width(), up.height()))
}

pub fn window_png_when_ready<W>(window: W, path: &Path, attempts: u32, done: impl FnOnce(Result<(), String>) + 'static)
where
    W: IsA<gtk::Window> + IsA<gtk::Widget> + 'static,
{
    window.upcast_ref::<gtk::Window>().present();
    let path = path.to_path_buf();
    let remaining = std::cell::Cell::new(attempts);
    let first = std::cell::Cell::new(true);
    let done = std::cell::RefCell::new(Some(Box::new(done) as Box<dyn FnOnce(Result<(), String>)>));
    let widget = window.upcast_ref::<gtk::Widget>().clone();
    widget.add_tick_callback(move |_widget, _clock| {
        // Alineado con el frame clock. Si el reloj está suspendido (ventana ocluida,
        // típico en Wayland), pedimos expresamente la fase PAINT para despertarlo:
        // sin fase en curso, GTK ni siquiera dispara este tick callback.
        if first.get() {
            first.set(false);
            if let Some(surface) = window.upcast_ref::<gtk::Window>().surface() {
                surface.frame_clock().request_phase(gtk::gdk::FrameClockPhase::PAINT);
            }
            return glib::ControlFlow::Continue;
        }
        let result = capture_once(window.upcast_ref::<gtk::Window>(), &path);
        let finished = match result {
            Ok(()) => {
                if let Some(done) = done.borrow_mut().take() {
                    done(Ok(()));
                }
                true
            }
            Err(error) if remaining.get() == 0 => {
                if let Some(done) = done.borrow_mut().take() {
                    done(Err(error));
                }
                true
            }
            Err(_) => {
                remaining.set(remaining.get() - 1);
                if let Some(surface) = window.upcast_ref::<gtk::Window>().surface() {
                    surface.frame_clock().request_phase(gtk::gdk::FrameClockPhase::PAINT);
                }
                false
            }
        };
        if finished { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
    });
}

fn capture_once(window: &gtk::Window, path: &Path) -> Result<(), String> {
    let width = window.width().max(1);
    let height = window.height().max(1);
    // El snapshot de la ventana completa pasa por AdwDialogHost y puede fallar sin asignación.
    // El chrome real (header bar, contenido, barra inferior) vive en la AdwToolbarView: la
    // localizamos y la pintamos sobre un fondo blanco del tamaño de la ventana.
    let mut walker: Option<gtk::Widget> = window.child();
    let mut content = None::<gtk::Widget>;
    for _ in 0..8 {
        let Some(widget) = walker else { break };
        if widget.type_().name() == "AdwToolbarView" {
            content = Some(widget);
            break;
        }
        walker = widget.first_child();
    }
    let content = match content {
        Some(widget) => widget,
        // Ventanas sin ToolbarView (diálogos simples): el hijo directo hace de contenido.
        None => window.child().ok_or_else(|| "la ventana no tiene contenido".to_string())?,
    };
    let fallback = gtk::Snapshot::new();
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    fallback.append_color(&gtk::gdk::RGBA::WHITE, &bounds);
    let debug = std::env::var_os("PDFCRAFT_CAPTURE_DEBUG").is_some();
    let mut content_node = None;
    // Estrategia 1: WidgetPaintable de la ventana completa (incluye su fondo).
    if content_node.is_none() {
        let paintable = gtk::WidgetPaintable::new(Some(window.upcast_ref::<gtk::Widget>()));
        let snapshot = gtk::Snapshot::new();
        gtk::gdk::prelude::PaintableExt::snapshot(&paintable, &snapshot, f64::from(width), f64::from(height));
        let node = snapshot.to_node();
        if debug {
            eprintln!("[DEBUG-capture] strategy=window-paintable node={:?}", node.is_some());
        }
        content_node = node;
    }
    // Estrategia 2: snapshot desde el padre directo (la vía "oficial" de GTK).
    if content_node.is_none()
        && let Some(parent) = content.parent()
    {
        let snapshot = gtk::Snapshot::new();
        parent.snapshot_child(&content, &snapshot);
        let node = snapshot.to_node();
        if debug {
            eprintln!("[DEBUG-capture] strategy=parent-child parent={:?} node={:?}", parent.type_().name(), node.is_some());
        }
        content_node = node;
    }
    // Estrategia 3: WidgetPaintable del contenido.
    if content_node.is_none() {
        let paintable = gtk::WidgetPaintable::new(Some(&content));
        let snapshot = gtk::Snapshot::new();
        gtk::gdk::prelude::PaintableExt::snapshot(&paintable, &snapshot, f64::from(width), f64::from(height));
        let node = snapshot.to_node();
        if debug {
            eprintln!("[DEBUG-capture] strategy=content-paintable node={:?}", node.is_some());
        }
        content_node = node;
    }
    let content_node = content_node.ok_or_else(|| "GTK no produjo render node para el contenido".to_string())?;
    fallback.append_node(&content_node);
    if std::env::var_os("PDFCRAFT_CAPTURE_DEBUG").is_some() {
        eprintln!("[DEBUG-capture] content={} parent={:?}", content.type_().name(), content.parent().map(|parent| parent.type_().name().to_string()));
    }
    let node = fallback.to_node().ok_or_else(|| "GTK no produjo render node del fondo".to_string())?;
    let surface = window.surface().ok_or_else(|| "la ventana no está mapeada".to_string())?;
    let renderer = gtk::gsk::Renderer::for_surface(&surface).ok_or_else(|| "GSK no tiene renderer para la superficie".to_string())?;
    if !renderer.is_realized() {
        renderer.realize(Some(&surface)).map_err(|e| format!("no se pudo realizar el renderer: {e}"))?;
    }
    let texture = renderer.render_texture(&node, None);
    // Patrón verificado en la app vieja: SIEMPRE desconectar el renderer tras
    // render_texture; dejarlo realizado aborta en gsk_renderer_dispose al salir.
    renderer.unrealize();
    texture.save_to_png(path).map_err(|e| format!("no se pudo guardar {}: {e}", path.display()))
}

pub fn mark(ok: bool, name: &str, application: &adw::Application) {
    use std::io::Write as _;
    if ok {
        println!("GTK_QA_OK {name}");
        let _ = std::io::stdout().flush();
        application.quit();
    } else {
        eprintln!("GTK_QA_FAIL {name}");
        let _ = std::io::stderr().flush();
        std::process::exit(1);
    }
}

pub fn path(name: &str) -> std::path::PathBuf {
    artifact(&format!("{name}.png"))
}

pub fn artifact(name: &str) -> std::path::PathBuf {
    let dir = std::env::var_os("PDFCRAFT_GTK_QA_DIR").unwrap_or_else(|| "/tmp/opencode/pdfcraft-gtk-qa".into());
    std::path::PathBuf::from(dir).join(name)
}
