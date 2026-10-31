# Equivalencia del frontend GTK con PdfCraft

## Criterio

La referencia funcional es el frontend egui de este mismo commit y los comandos de `crates/engine/src/commands.rs`. El checklist `parity/acrobat-features.toml` describe paridad con Acrobat, no paridad entre frontends, y no se usa para exigir funciones que el original todavía marca como planificadas.

Estados:

- `Pendiente`: no existe equivalencia GTK verificada.
- `En curso`: existe código o una ruta Automation, pero falta comportamiento o evidencia de parte del área.
- `Equivalente`: acción GTK accesible, mismo efecto de motor y QA en verde.
- `No aplica`: función de otra plataforma sin equivalente de escritorio, con justificación.

No se cuenta como equivalente una entrada de menú que solo muestra un mensaje o un placeholder. Un formulario Automation verificado equivale a la operación headless correspondiente, pero no demuestra por sí solo la edición interactiva del canvas.

## Matriz por áreas

| Área original | Equivalencia GTK | Estado | Evidencia |
|---|---|---|---|
| Inicio y apertura | Home, diálogo de apertura y apertura por archivos | Equivalente | `scripts/gtk-qa.sh smoke` |
| Pestañas | `AdwTabView`, selección, cierre y reconciliación | Equivalente | `viewer`, `reconcile` |
| Lectura de página | `GtkPicture` alimentado por `RenderPool` | Equivalente | `smoke.png` |
| Navegación | Anterior, siguiente y atajos PageUp/PageDown | Equivalente | `viewer` |
| Zoom | Acercar, alejar, 100 % y ajustar | Equivalente | `viewer.png` |
| Archivo: guardar | Escritura atómica y `mark_saved` | Equivalente | `document`, PDF de 3 páginas |
| Archivo: guardar como, revertir y cerrar todo | Acciones nativas sobre la Automation compartida | Equivalente | Humo `lifecycle`: revertir restaura rotación y estado limpio; guardar como escribe y actualiza la ruta de la vista; cerrar todo deja 0 vistas |
| Archivo: crear, combinar, reducir y optimizar | Blank nativo y comandos Automation con formularios | En curso | Blank y dispatcher implementados, falta QA por operación |
| Archivo: exportaciones e imágenes | `export.text` mediante formulario JSON Schema | Equivalente parcial | `export-ops` verifica página-imagen PNG, texto, office DOCX, all_images (0 en PDF sin imágenes) y all_images con imagen REAL (ciclo render→`doc_create` from images→reextracción `_Page_1_Image_0001.png` con magia PNG verificada); tool_form con selector de fichero/carpeta para `path`/`out`/`folder`; faltan opciones completas del diálogo del original (JPEG/TIFF/calidad/min_size/rango) y humos de comment/form export y measure.export |
| Archivo: contraseñas, seguridad, propiedades, word count e imprimir | Propiedades mediante `command_run`; unlock y recientes nativos; resto por Automation | En curso | `automation`, 1767 bytes JSON |
| Edición: deshacer y rehacer | `Session::undo/redo` y refresh GTK | Equivalente | `document` |
| Edición: portapapeles, búsqueda, selección y preferencias | Clipboard de páginas, texto y snapshot nativos; búsqueda `text_find`; preferencias y recientes nativos | Equivalente | `editing` cubre copy/paste de páginas y snapshot; `text-tools` verifica copia de texto real al portapapeles y búsqueda con resultados nativos |
| Páginas: rotar, duplicar, borrar, insertar blank e invertir | `Edit` nativo sobre la sesión compartida | Equivalente | `document` |
| Páginas: extraer | `page.extract` con salida y apertura del documento resultante | Equivalente | `reconcile`, PDF de 1 página |
| Páginas: organizar, reemplazar, dividir, recortar y numerar | Organizer nativo y rutas Automation con formularios | Equivalente, salvo recorte por arrastre | `organizer` verifica mover/rotar/borrar con el mismo camino de los botones; `page-ops` verifica numerar (p-iii/p-iv), recortar (tamaños exactos), reemplazar (texto nuevo) y dividir (2 partes de 1 página con `pdfinfo`); queda el recorte interactivo por arrastre del original |
| Vista: layouts, historial, pantalla completa y modo lectura | Single/TwoUp, continuo, huecos, portada, Fit Page/Width, historial, lectura y paneles nativos | Equivalente | Humo `screens` activa side/tools + two-up y captura; atajos y menús cubren el resto de comandos de vista |
| Menús, paleta y atajos | `GtkPopoverMenuBar`, `GioSimpleAction` y paleta generados desde `COMMANDS` | Equivalente | `palette`, 209/209, 3 repeticiones |
| Paneles laterales y herramientas | Panel Document con 7 secciones reales (Comentarios, Marcadores, Páginas con miniaturas, Campos, Capas, Adjuntos, Firmas); panel Tools lateral con 29 herramientas de anotación como botones de ICONO TINTADO con el color de su tipo (estilo Acrobat) y rail de COMANDOS del catálogo del original (15 grupos en 5 categorías, iconos lucide tintados por `hue`, chips de disponibilidad, expandibles hasta cada ToolItem que ejecuta su comando) | En curso | Rail verificado en `tools-rail` (23 grupos, 30 botones, captura); edición verificada en `panels-edit` y `panels-more`: marcadores añadir/renombrar/borrar/mover/indentar/desindentar/fijar página, campos valor/check/propiedades (tooltip, read-only, required), capas on/off + reset, borrar comentarios y adjuntos; faltan propiedades completas de campo (resto de `FieldProps`), `MoveInTabOrder` y visibilidad de capas en un PDF con capas reales |
| Anotaciones y comentarios | Gestos por página con `AddAnnotation`/`DeleteAnnotation` nativos; resto por Automation | En curso | `editing` añade y borra una Note real; falta humo de cada shape |
| Formularios PDF | Rutas Automation (`form_*`) con formularios JSON Schema | En curso | Dispatcher cubre las tools; falta humo por tipo de campo |
| Fill & Sign | Marcas, texto, fecha y firma/iniciales tipografiadas guardadas interactivas | En curso | 8 herramientas en panel Tools con humo `fill-sign` (check + fecha + firma verificadas en PDF); pendientes: pad de dibujo, imagen/escaneo, formato de fecha de preferencias, change de firma |
| Firmas digitales y certificación | Firma/certificación visibles e invisibles interactivas con .p12, validación y panel de firmas | En curso | Humo `sign` con ec-p256.p12: diálogo de firma, sello visible renderizado, 1/1 válida y PDF firmado verificado; falta crear ID auto-firmado desde la UI, keychain y TSA/LTV |
| Redacción | Marcar (arrastre, buscar/palabras), aplicar con confirmación y limpiar | Equivalente (sin cuadros de códigos ni diálogo de alcance por páginas del original) | `redact.apply` nativo con `Edit::ApplyRedactions`; QA `redact` verifica eliminación real del texto |
| Saneado, OCR, acciones y JavaScript | Rutas Automation con formularios | En curso | `redact.sanitize` y acciones builtin mapeadas; falta humo por acción |
| Accesibilidad, etiquetas y comparación | `accessibility_autotag_apply` (con fallback a campos), `doc_compare` con resultados nativos | Equivalente básico | `parity-ops` verifica autotag real (etiquetas creadas) y comparación de dos documentos con cambios detectados; faltan reporte/marcado de comparación y panel a11y dedicado |
| Ventana: orden de pestañas, reabrir y revelar | Submenú Window con reabrir pestaña cerrada (historial de 10), revelar en gestor de archivos y reordenado (arrastre nativo de `TabBar`, verificado con `reorder_page`) | Equivalente | `parity-ops` cierra la pestaña activa, la reabre desde disco y verifica ruta y vista; `lifecycle` reordena la pestaña del fixture a la posición 1 y captura el resultado; revelar usa `FileLauncher::open_containing_folder` |
| Ayuda y preferencias completas | About y enlaces nativos; resto pendiente | En curso | Falta QA |

## Cobertura de comandos

El inventario canónico medido en runtime contiene 209 comandos. Todos aparecen en los menús y en la paleta. `coverage` clasifica en ejecución 96 con handler nativo y 113 con herramienta Automation, sin IDs sin destino.

Esa cobertura de despacho no equivale a paridad funcional. La tabla por áreas solo avanza a `Equivalente` cuando existe un humo que ejecuta la acción y comprueba sus datos o su estado.

## Evidencia visual

Las capturas se guardan fuera del árbol fuente en `/tmp/opencode/pdfcraft-gtk-qa/`. Cada captura se produce después de ejecutar la acción correspondiente y se inspecciona, no solo se comprueba que el archivo existe. Desde el incremento 10 se generan bajo Xvfb (headless, `GDK_BACKEND=x11`), con el mismo tema claro Adwaita; `PDFCRAFT_QA_NATIVE=1` las reproduce en la sesión real.

Capturas inspeccionadas: `smoke.png`, `viewer.png`, `document.png`, `palette.png`, `automation.png`, `form.png`, `form-result.png`, `reconcile.png`, `coverage.png`, `editing.png`, `organizer.png`, `organizer-ops.png`, `recents.png`, `redact.png`, `panels.png`, `text-tools.png`, `redact-apply.png`, `recents-open.png`, `fill-sign.png`, `sign-dialog.png`, `sign-applied.png`, `page-ops.png`, `panels-edit.png` y `export-ops.png`. El modo agregado `scripts/gtk-qa.sh captures` exige las dieciocho QA en verde y reproduce todas las capturas.
