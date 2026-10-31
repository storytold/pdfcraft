# Port GTK4 + libadwaita

## Objetivo y alcance

`pdfcraft-gtk` es un frontend nativo GNOME del mismo workspace que PdfCraft. El objetivo es ofrecer la funcionalidad del frontend egui mediante GTK4 y libadwaita, reutilizando el motor y la automatización existentes.

El frontend egui y la CLI se conservan durante la migración. La paridad solo se declara cuando cada acción tiene una equivalencia accesible, una prueba de comportamiento y evidencia visual cuando corresponde.

## Arquitectura

- `apps/pdfcraft-gtk`: aplicación `ai.storyteller.pdfcraft.gtk`.
- `crates/engine`: única entrada al motor para el documento interactivo. El frontend no depende de crates de render, organización, edición, firma o exportación.
- `crates/automation`: una única instancia compartida es dueña de la `Session`; la GTK y `command_run` trabajan sobre los mismos documentos.
- `Automation::session_mut`: punto de mutación nativa que invalida renderers y cachés antes de devolver la sesión.
- `RenderPool`: render incremental de páginas mediante `RenderRequest` y resultados RGBA convertidos a `gdk::MemoryTexture`.
- `gio::spawn_blocking`: lectura y escritura de ficheros fuera del hilo de GTK.
- `glib::timeout_add_local`: consumo de resultados del render en el hilo principal.
- `COMMANDS`: genera menús, acciones, atajos y la paleta; no hay un segundo catálogo visual que pueda derivar.
- `command_run`: ejecuta las herramientas Automation asociadas a comandos; sus `input_schema` generan formularios libadwaita.
- Reconciliación: después de cada llamada, las pestañas se comparan con `Session::docs`, se cierran las ausentes y se crean las nuevas.
- Handlers nativos: acciones de vista, layouts, historial, paneles, tema, recientes, preferencias y unlock en `app/native.rs`.
- Edición nativa: clipboard de páginas y texto, snapshot, organizer y preferencias de redacción en `app/editing.rs`.
- Anotaciones: `document/annotations.rs` instala `GestureClick` y `GestureDrag` en cada `GtkPicture`, convierte coordenadas locales a usuario con `PageInfo` y aplica `AddAnnotation`/`DeleteAnnotation` sobre la sesión compartida. El índice de borrado es el `index` real del objeto en `/Annots`, no la posición del listado.
- Estado de vista: `DocumentView` mantiene Single/TwoUp, continuo, huecos, portada, `Fit::Page/Width`, historial de vistas y render multipágina.

La ventana usa `AdwApplicationWindow`, `AdwToolbarView`, `AdwHeaderBar`, `AdwTabView`, `AdwTabBar` y `AdwToastOverlay`. Las páginas se presentan en un `GtkPicture` dentro de un `GtkScrolledWindow`. Los formularios, resultados y la paleta usan `AdwWindow` modal con `set_content`; se evitó `AdwDialog` porque su allocación y captura resultaron no deterministas en los humos.

## Verificación

Los gates observables están en `GATES.md`. El punto de entrada de QA es:

```sh
scripts/gtk-qa.sh build
scripts/gtk-qa.sh test-engine
scripts/gtk-qa.sh smoke
scripts/gtk-qa.sh viewer
scripts/gtk-qa.sh document
scripts/gtk-qa.sh palette
scripts/gtk-qa.sh automation
scripts/gtk-qa.sh form
scripts/gtk-qa.sh reconcile
scripts/gtk-qa.sh coverage
scripts/gtk-qa.sh editing
scripts/gtk-qa.sh screens
scripts/gtk-qa.sh captures
```

Los humos cargan un PDF real del repositorio, ejercitan acciones y capturan la ventana GTK real. Los PNG, logs y artefactos se escriben fuera del repo en `/tmp/opencode/pdfcraft-gtk-qa/`, o en `PDFCRAFT_GTK_QA_DIR` si se define.

La captura usa `WidgetPaintable` de la ventana, `GskRenderer` y `GdkTexture`. No depende del portal de captura de GNOME Shell. Hacer snapshot del hijo excluye el fondo de la `AdwWindow` (PNG transparente a negro): hay que pintar la ventana completa vía paintable. El renderer se realiza solo si hace falta y siempre se libera con `unrealize` después de `render_texture`. `captures` encadena los nueve modos y solo da verde si todos pasan.

## Registro de cambios

### Incremento 1: shell y lectura

- Aplicación y ventana libadwaita.
- Apertura por diálogo y por archivos pasados a la aplicación.
- Pestañas de documentos con cierre.
- Render de página, navegación, zoom y ajuste.
- Humos `smoke` y `viewer` con captura.

### Incremento 2: operaciones de documento

- Nuevo blank, guardado atómico, guardar como, revertir, cerrar y cerrar todo.
- Deshacer y rehacer.
- Rotar, duplicar, insertar blank, borrar e invertir páginas.
- Humo `document` con PDF de salida validado por `pdfinfo`.

### Incremento 3: registry y Automation

- 209 comandos en menús, acciones, atajos y paleta.
- Una única Automation compartida con la sesión interactiva.
- Dispatcher `command_run`, formularios JSON Schema y resultados JSON/PNG nativos.
- Reconciliación de documentos y pestañas.
- Humos `palette`, `automation`, `form` y `reconcile` con capturas y artefactos reales.

### Incremento 4: estado nativo y vistas

- Handlers nativos: recientes, carpeta fija, tema, preferencias, atajos, enlaces, desbloqueo.
- Estado de vista multipágina: single/two-up, continuo, huecos, portada, ajuste a página/ancho y historial.
- Modo lectura, paneles laterales/de herramientas y marquee zoom sobre el render real.
- Humos y cobertura de despacho `coverage` para los 209 comandos.

### Incremento 5: edición interactiva

- Portapapeles de páginas, pegado entre documentos, texto de página y snapshot al portapapeles.
- Creación desde el portapapeles, organizador de páginas, búsqueda y preferencias de redacción.
- Anotaciones interactivas por shape con coordenadas reales de página; borrado con índice real de `/Annots`.
- Humo `editing` con copia/pegado, snapshot, nota añadida y borrada, y captura.

Las operaciones de formularios PDF, Fill&Sign, firmas digitales y redacción aplicada siguen por Automation; su equivalencia interactiva se registra en `docs/gtk-parity.md`.

### Incremento 6: pantallas y captura de ventanas

- Modo `screens`: ejercita organizador, recientes (con siembra previa), propiedades de redacción y paneles laterales con layout two-up, y captura cada ventana.
- Las ventanas modales (`adw::Window`) deben asociarse con `.application(...)` para aparecer en `application.windows()`; sin eso el selector de QA no las ve.
- Captura corregida: `WidgetPaintable` de la ventana completa; el snapshot del hijo dejaba el fondo transparente (PNG negro).
- Modo agregado `captures` que exige los nueve humos en verde.

### Incremento 7: paneles reales y robustez de captura

- Panel lateral Document con siete secciones reales (Comentarios, Marcadores, Páginas, Campos, Capas, Adjuntos, Firmas) alimentadas de `DocInfo`/`Document`, con navegación al hacer clic y miniaturas de página renderizadas con un `RenderPool` propio de baja resolución.
- Panel Tools con las 19 herramientas de anotación como `ToggleButton` excluyentes que activan el modo de anotación del visor.
- Fallback de captura: si el nodo del `WidgetPaintable` no está disponible, se compone el snapshot del contenido sobre fondo blanco en lugar de fallar; reintentos espaciados en los pasos de QA.
- `pdfcraft_render::{RenderConfig, RenderPool}` reexportados desde `pdfcraft-engine` para respetar la capa L6.
- Limpieza de clippy en la app (imports sin comodines en anotaciones, casts y avisos menores).

### Incremento 8: humos funcionales de organizador y herramientas de texto

- Modo `organizer`: fixture con tres documentos de texto marcados (A/B/C) combinados en uno, abre la ventana Organizar páginas y aplica los mismos `Edit` que sus botones (mover, rotar, borrar); verifica orden y rotación por el texto real de cada página (`text_lines`) y captura `organizer-ops.png`.
- Modo `text-tools`: copia el texto de la página al portapapeles (`edit.copy`) y lee el portapapeles para verificarlo; ejecuta la herramienta real `text_find` y presenta la ventana nativa de resultados; captura `text-tools.png` con el JSON de coincidencias.
- La ventana de resultados de Automation se asocia ahora a la `GtkApplication` y se publica en `LAST_RESULTS` (thread_local) para que el QA y futuros selectores la encuentren.
- Gotcha de fixtures: los documentos abiertos por `open_paths` se seleccionan de forma asíncrona; el fixture creado por código debe abrirse después (500 ms) para que quede como pestaña activa.

### Incremento 9: redacción interactiva

- Herramienta Redactar (`comment.redact`) en el panel Tools: arrastre sobre la página crea `Shape::Redact` con `rect_quad` y el texto superpuesto guardado en las preferencias de redacción (`native::redact_defaults`, ahora `pub(crate)`).
- `redact.mark`/`redact.apply` son handlers nativos: el primero activa la herramienta y muestra el panel Tools; el segundo cuenta las marcas (`Document::redaction_marks`), avisa si no hay ninguna, confirma en una ventana modal con botón destructivo y aplica `Edit::ApplyRedactions { pages: None }` (deshacible hasta guardar), con toast de resultado.
- `window_with_content` asocia ahora cada modal nativa a la `GtkApplication` (`.application(...)`), de modo que todas aparecen en `application.windows()` para el QA y selectores.
- Modo QA `redact`: activa la herramienta, crea la marca por el mismo camino del motor (`redact_mark` con `find`), aplica y verifica que `redaction_marks` llega a 0 y que el texto redactado ya no es extraíble en `text_lines`; captura `redact-apply.png` con la caja aplicada y el toast. El agregado `captures` encadena 12 QA.

### Incremento 10: captura headless determinista (Xvfb)

- Causa raíz de los flakes de captura: en Wayland, GDK suspende el frame clock de las ventanas que no están visibles en pantalla. Con la sesión ocluida, los snapshots devuelven 0 nodos aunque el árbol esté alocado y mapeado, y los `add_tick_callback` ni siquiera llegan a dispararse (los timeouts del hilo principal sí corren, por eso el fallo parecía intermitente).
- `window_png_when_ready` reintenta dentro de un `add_tick_callback` (alineado al frame) y, antes del primer intento y tras cada fallo, pide expresamente `frame_clock().request_phase(PAINT)` para despertar el reloj.
- `capture_once` prueba tres estrategias en cascada y usa la primera que produzca render node: snapshot desde el padre directo (`parent.snapshot_child`), `WidgetPaintable` del contenido y `WidgetPaintable` de la ventana; el nodo se compone sobre un fondo blanco del tamaño de la ventana para evitar transparencias a negro.
- Gotcha de `gtk::Snapshot`: `clone()` no es una copia profunda; hacer `to_node` sobre el clon consume el estado compartido y el `to_node` original posterior aborta con `gtk_snapshot_get_current_state: (size > 0)`. Nunca clonar un `Snapshot` para inspección.
- El renderer de la captura se desconecta SIEMPRE tras `render_texture` (`unrealize`); dejarlo realizado aborta en `gsk_renderer_dispose` al salir. `mark` hace flush de stdout/stderr para que el marcador `GTK_QA_OK` no se pierda en un abort posterior.
- `scripts/gtk-qa.sh` ejecuta ahora cada modo bajo `xvfb-run -a -s "-screen 0 1280x900x24"` con `GDK_BACKEND=x11`: gates deterministas con la sesión oculta o desbloqueada, sin depender de que el usuario tenga la pantalla visible. `PDFCRAFT_QA_NATIVE=1` fuerza la sesión gráfica real.
- `screens` secuenciado sin carreras: pasos a 2300/3400/4600/5900 ms con captura a +400 ms; el éxito de cada modal solo registra (`[DEBUG-screens] X: capturada`) y cierra la ventana, el chequeo final (7000 ms) valida los cuatro PNG y emite el único `GTK_QA_OK screens`.
- Regresión completa en verde bajo Xvfb: agregado `captures` con los 13 modos + agregado (14 marcadores), clippy `--all-targets` sin avisos propios y fmt limpio.

### Incremento 11: Fill&Sign interactivo

- Las ocho herramientas de relleno (`sign.fill.text/check/cross/dot/line/date/signature/initials`) son botones reales del panel Tools: activan el modo de anotación y se aplican con clic sobre la página, igual que el original egui (`FillTool`).
- Marcas (`check/cross/dot/line`) usan `FillMark::rect_at` (`Shape::Mark`); texto y fecha usan `Shape::Typewriter` con el top calculado vía `displayed_offset` y ancho estimado; la fecha se formatea `dd/mm/AAAA` con `glib::DateTime`. Firma e iniciales guardadas se colocan con `typed_signature_shape` (alto 32/24).
- Si no hay firma/iniciales guardadas, se abre el diálogo de creación (EntryRow + Guardar) que persiste en Settings; los diálogos son `adw::Window` modales con `set_application` (visibles en `application.windows()` para QA).
- API 0.9.2: `adw::Window` no expone `content-width/content-height` (ni builder ni ext, solo `AdwDialog`); usar `set_default_size`. `entry.connect_changed` mueve el botón al closure: clonar antes (`apply.clone()`).
- QA modo `fill-sign`: fixture propio, coloca check + fecha + firma guardada ("QA Firmado"), verifica +3 anotaciones y captura `fill-sign.png`. Agregado `captures` en verde con 15 marcadores (14 modos + agregado).
- Pendientes honestos de Fill&Sign: pad de dibujo de firma (trazos), imagen/escaneo, formato de fecha desde preferencias y "change" de firma guardada.

### Incremento 12: firmas digitales (p12)

- Nuevo módulo `app/signing.rs` con los cinco comandos del registry: `sign.digital`/`sign.certify` activan una herramienta de arrastre (el rectángulo arrastrado abre el diálogo de firma), `sign.certify_invisible` firma sin rectángulo con nivel DocMDP 2, `sign.validate` revalida contra el almacén de confianza y hace toast "Firmas válidas: X de Y", `sign.panel` selecciona la sección Firmas del panel Document.
- Diálogo de firma (`DocumentView::open_sign_dialog`): EntryRow certificado (.p12) precargado desde Settings, PasswordEntryRow contraseña, motivo y ubicación; botón Firmar solo sensible con certificado y contraseña. Botones "Firmar" y "Certificar" añadidos al panel Tools (29 herramientas).
- `perform_sign` sigue el flujo canónico del original: `pkcs12::open` -> `Session::sign` (no hace check_save) -> escritura atómica (.part + rename) -> `set_trusted_certificates` + `mark_signed` (borra undo/redo: firmar no se deshace) -> refresh. El certificado usado pasa a confianza, por lo que la firma propia sale válida.
- QA modo `sign`: fixture propio, captura del diálogo y firma real con `crates/sign/tests/data/ec-p256.p12` (password `test`), rectángulo visible en página 0; asserts performed/signed/total==1/valid==1/visible y `pdfinfo` Pages 1 del PDF firmado. Capturas `sign-dialog.png` (diálogo) y `sign-applied.png` (sello "Digitally signed by Test Signer EC").
- API: `adw::Window` no expone `content-width/height` (solo `AdwDialog`); usar `set_default_size`. Clippy `too_many_arguments` obligó a agrupar en struct `SignRequest`. `capture_once` ahora usa el hijo directo como contenido cuando la ventana no lleva ToolbarView (diálogos simples).
- No portado aún (TSA/LTV): `sign_with_timestamp`, `timestamp_document` y `embed_ltv` requieren autoridad de sellado de tiempo.

### Incremento 13: reemplazar, dividir, recortar y numerar páginas

- QA modo `page-ops` (`schedule_page_ops_qa`): fixture A+B combinado de 2 páginas y cuatro tools Automation por el mismo camino que los formularios: `page_number` (etiquetas `/PageLabels` lower-roman p-iii/p-iv verificadas en `info.pages[].label`), `page_set_box` crop con márgenes `[10,20,30,40]` (page_sizes exactos contra el tamaño original menos 40/60 pt), `page_replace` de la página 2 por un PDF nuevo (texto "R" presente, "B" ausente, 2 páginas conservadas) y `doc_split` `every:1` (2 partes de 1 página validadas con `pdfinfo` y ficheros no vacíos).
- Captura `page-ops.png` tras las cuatro operaciones. Agregado `captures` en verde con 17 marcadores (16 modos + agregado).
- Gotchas: `PageInfo.label` es `String` (no `Option`); `Content::Png` es variante struct `{data,width,height}`; el alias actual de `page.crop` con defaults vacíos hace reset del CropBox (`BoxSpec::Remove`), no recorte interactivo.
- Pendiente honesto: recorte interactivo por arrastre (QuickTool::Crop del original); diálogo de páginas en los formularios ya disponible vía `tool_form`.

### Incremento 14: edición desde los paneles laterales

- Marcadores: fila con botones renombrar (prompt `EntryRow`) y borrar, y botón `+` en la cabecera que añade un marcador raíz a la página actual (`AddBookmark` con `parent: []`, `index: outline.len()`). Las rutas son `Vec<usize>` contra el `info.outline` vigente tras cada `apply`.
- Campos: los campos `Text` editan su valor desde el panel (prompt) y los `CheckBox` conmutan con un `Switch`-like de botón (`SetFieldValue` con `FieldValue::Text/Check`). Navegación por clic ya existente.
- Capas: `Switch` por fila conectado a `Session::set_layer_visible` (no es un `Edit`: no lleva undo ni permiso; la elección sobrevive a los refrescos del engine por id de OCG) y botón de reset a la visibilidad inicial (`Session::reset_layers`).
- Comentarios y adjuntos: botón papelera en cada fila (comentarios: `DeleteAnnotation` con el índice real de `/Annots`; adjuntos de documento: `DeleteAttachment` con su `key`). Acciones gated por `allows_*` del documento.
- El `fingerprint` de reconstrucción del panel ahora incluye la visibilidad por capa, el hash de tipo+valor de campo y el total recursivo del outline, para refrescar cuando cambian sin cambio de longitud.
- QA modo `panels-edit` (`schedule_panels_edit_qa`): siembra 2 marcadores + adjunto + nota por Edits reales, ejercita añadir/renombrar/borrar marcador desde el panel, `form_add_field` + edición de valor desde panel, borrado de adjunto y comentario, y verifica estado final (`roots == ["Raíz","Nuevo"]`, sin hijos, campo `"QA"`, 0 adjuntos, 0 comentarios) con captura `panels-edit.png` (panel Document visible, sección Marcadores).
- Gotchas: no mantener `automation.borrow()` al llamar `session_mut()` dentro del mismo closure (`RefCell already borrowed`); capturas con nombres `Rc` distintos por closure para evitar E0382/E0505; `NewAnnotation.contents/author` son `String`, no `Option`.

### Incremento 15: exportaciones (imagen, texto, office, imágenes incrustadas)

- Las siete acciones del registry (`export.image/text/all_images/docx/pptx/html/rtf`) ya resolvían a sus tools de automation (`doc_export_images`, `doc_export_text`, `doc_export_all_images`, `doc_export_office`) por el dispatcher normal. Este incremento añade la verificación funcional y mejora el formulario.
- `tool_form` ahora pone un selector junto a los campos `folder` (`FileDialog::select_folder`) y `path`/`out` (`FileDialog::save` con `initial_file` tomado del texto actual), transient al propio diálogo; hasta ahora esos campos se tecleaban a mano.
- QA modo `export-ops` (`schedule_export_ops_qa`): sobre un fixture de texto ejercita las cuatro rutas reales por `automation.call` y verifica resultados: página exportada a PNG (magia `\x89PNG`, >100 bytes), texto (`export-ops.txt` con el contenido), office DOCX (formato `docx`, bytes con magia `PK`), y `doc_export_all_images` devuelve `count: 0` en un PDF sin imágenes. Captura `export-ops.png`.
- Script: modo `export-ops` con limpieza de artefactos (txt/docx/carpetas de imágenes) y validación de que la carpeta contiene exactamente 1 PNG.
- El original además ofrece opciones del diálogo de export (resolución 72-600 ppp, PNG/JPEG/TIFF con calidad, filtro de tamaño para all images, rango de páginas): hoy llegan vía `tool_form` como campos del schema; los defaults del motor (150 ppp, png, calidad 85) se aplican cuando el campo queda vacío.
- Gotchas: en `tool_form` el closure del picker necesita su propio clon de la fila antes del `move` (E0382); `gtk::gio::File::for_path` acepta el `GString` directamente (sin `to_string`).
- Verificación posterior de imágenes reales (ciclo render→create): la misma QA exporta la página a PNG (`doc_export_images`), crea un PDF nuevo con `doc_create {from:"images", paths:[...]}` y sobre él ejecuta `doc_export_all_images`, que debe devolver `count: 1` con `files[0].path` terminado en `_Page_1_Image_0001.png` y magia PNG. El script valida además el PNG resultante en disco. Gotcha: en `doc_export_all_images` `files` es un array de objetos `{path,page,width,height}` (en `doc_export_images` son strings planos); el nombre del fichero extraído lleva el stem del PDF fuente, no del original.

### Incremento 17: gate parity (G9), ventana, comparación y accesibilidad

- Gate G9 nuevo: modo `parity` en el script = ejecuta `coverage` (209/209) y falla si queda alguna fila `Pendiente` en `docs/gtk-parity.md`. Con esto los nueve gates de `GATES.md` están cerrados y el agregado `captures` sube a 20 marcadores (19 submodos + agregado).
- Ventana (reabrir/revelar): el original reabre la pestaña cerrada releyendo el fichero guardado y revela por plataforma; el port GTK añade historial `closed_tabs` (nombre + path, tope 10) alimentado desde `connect_close_page`, métodos `reopen_closed_tab` (reabre con `open_paths` si el fichero existe) y `reveal_active_file` (`FileLauncher::open_containing_folder`), y submenú Window con ambas acciones (`win.reopen-closed-tab`, `win.reveal-file`).
- Comparación y accesibilidad: QA modo `parity-ops`: sobre el fixture que abre el opener genérico (PDF de texto puro generado por el script), crea un segundo documento con `doc_create {from:"text"}`, cierra la pestaña activa, la reabre y verifica vista única con la ruta restaurada; luego `doc_compare` (debe detectar cambios: `identical=false` y replaced+inserted+deleted > 0) y `accessibility_autotag_apply` (etiquetas creadas; si el motor lo rechaza por campos de formulario, cae a `accessibility_autotag_form_fields`). Captura `parity-ops.png` con el menú Window y la pestaña reabierta.
- Gotchas: `doc_create` devuelve el id bajo la clave `doc` (no `id`); el estado de un check a11y se serializa como `passed`/`failed`/`manual`/`skipped`; el opener genérico de `schedule_qa` abre `PDFCRAFT_GTK_QA_FILE` a los 200 ms, así que el fixture de un modo debe servirse por esa vía (con path en disco) y no abrir otro documento en paralelo (carrera de pestañas); el autotag de texto rechaza PDFs con campos de formulario ("form fields need their own tags").

### Incremento 18: ciclo de vida completo y reordenar pestañas

- Humo dedicado a las tres acciones de archivo que no tenían verificación individual: `file.revert` (rota la página activa, revierte y verifica rotación 0 y documento limpio), guardar como (la QA llama a `save_to`, el mismo helper del diálogo, y verifica fichero en disco, ruta de la vista actualizada y sin cambios pendientes) y `file.close_all` (0 vistas y 0 pestañas al final).
- Reordenar pestañas: `adw::TabBar` ya permite arrastrar por diseño; la QA lo verifica programáticamente con `TabView::reorder_page` (posición 1) y `page_position`, con la pestaña anotada antes de abrir el segundo documento.
- QA modo `lifecycle` (`schedule_lifecycle_qa`): fixture de dos páginas vía `QA_FILE`, segundo documento creado con `create_from_text`, captura `lifecycle.png` (título con la nueva ruta, toast de guardado, pestaña reordenada, menú Window). Script: cleanup de `lifecycle-copy.pdf` y validación `pdfinfo` de 2 páginas.
- Gotchas: `open_created` pide `String` (no `&str`); el `borrow()` no puede quedar como temporal de un `let ... else` (E0716: ligar el guard a una variable); `save_to` pasó a `pub(super)` para reutilizarlo desde la QA.
