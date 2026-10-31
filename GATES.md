# Gates del port GTK4/libadwaita

OWNS: apps/pdfcraft-gtk/**, crates/engine/src/lib.rs, crates/automation/src/lib.rs, docs/gtk-parity.md, docs/gtk-port.md, scripts/gtk-qa.sh, GATES.md, Cargo.lock

Scope: portar el frontend real de storytold/pdfcraft a GTK4/libadwaita sobre pdfcraft-engine, con equivalencias funcionales y capturas verificables.

- [x] G1: El frontend GTK compila sin errores
  CHECK: ./scripts/gtk-qa.sh build
  EXPECT: GTK_GATE_OK build
  EVIDENCE: `cargo check -p pdfcraft-gtk` exit 0, 31-Oct-2026.

- [x] G2: La fachada engine mantiene sus tests unitarios
  CHECK: ./scripts/gtk-qa.sh test-engine
  EXPECT: GTK_GATE_OK test-engine
  EVIDENCE: 389 passed, 0 failed, 2 ignored.

- [x] G3: La app abre un PDF real mediante Session y presenta píxeles de RenderPool en GTK
  CHECK: ./scripts/gtk-qa.sh smoke
  EXPECT: GTK_GATE_OK smoke
  EVIDENCE: GTK_QA_OK smoke y smoke.png inspeccionado.

- [x] G4: Navegación, zoom y cierre de pestañas funcionan con acciones GTK y estados comprobados
  CHECK: ./scripts/gtk-qa.sh viewer
  EXPECT: GTK_GATE_OK viewer
  EVIDENCE: página 2 de 2, zoom 125 %, cero vistas al cerrar, viewer.png inspeccionado.

- [x] G5: Rotar, deshacer, rehacer, duplicar, borrar, insertar, invertir y guardar ejecutan operaciones reales del engine
  CHECK: ./scripts/gtk-qa.sh document
  EXPECT: GTK_GATE_OK document
  EVIDENCE: GTK_QA_OK document; document-output.pdf tiene 3 páginas y document.png fue inspeccionado.

- [x] G5R: Menús y paleta se generan desde el registry completo y la paleta se captura de forma determinista
  CHECK: ./scripts/gtk-qa.sh palette
  EXPECT: GTK_GATE_OK palette
  EVIDENCE: 209/209 comandos, tres ejecuciones consecutivas verdes, palette.png inspeccionado.

- [x] G5A: Un comando Automation devuelve datos reales y los presenta en una ventana nativa
  CHECK: ./scripts/gtk-qa.sh automation
  EXPECT: GTK_GATE_OK automation
  EVIDENCE: file.properties devuelve 1767 bytes JSON, automation.png inspeccionado.

- [x] G5F: Un formulario generado desde JSON Schema ejecuta una exportación real
  CHECK: ./scripts/gtk-qa.sh form
  EXPECT: GTK_GATE_OK form
  EVIDENCE: export.text escribe exported.txt; form.png y form-result.png inspeccionados.

- [x] G5T: Los documentos creados por Automation se reconcilian con vistas y pestañas GTK
  CHECK: ./scripts/gtk-qa.sh reconcile
  EXPECT: GTK_GATE_OK reconcile
  EVIDENCE: 2 documentos, 2 vistas, 2 pestañas; reconcile-output.pdf tiene 1 página; captura inspeccionada.

- [x] G6: Edición, anotaciones, formularios y firmas tienen pruebas funcionales GTK y de datos
  CHECK: ./scripts/gtk-qa.sh editing
  EXPECT: GTK_GATE_OK editing
  EVIDENCE: copy/paste de páginas crea 3 páginas, snapshot al portapapeles, Note añadida y borrada con index real, editing.png inspeccionado. Tests engine 389 passed y automation 349 passed. Cobertura de despacho: 209/209 (81 nativos + 128 automation).

- [x] G7: Cada pantalla portada genera una captura GTK real después de ejercitar sus acciones
  CHECK: ./scripts/gtk-qa.sh captures
  EXPECT: GTK_GATE_OK captures
  EVIDENCE: modo agregado `captures` encadena smoke, viewer, document, palette, automation, form, reconcile, editing, screens, organizer, text-tools, redact y recents-open. Ejecución bajo Xvfb (incremento 10 de docs/gtk-port.md): gates deterministas con la sesión oculta. Capturas leídas visualmente en /tmp/opencode/pdfcraft-gtk-qa/.

- [x] G8: Las capturas se inspeccionan visualmente y muestran controles nativos sin placeholders ni duplicados
  EVIDENCE: capturas inspeccionadas (smoke, viewer, document, palette, automation, form, reconcile, editing, organizer, recents, redact, panels): chrome nativo libadwaita, menús desde el registry, paleta con 209 comandos reales, panel Tools con 19 herramientas de anotación reales y panel Document con 7 secciones y miniaturas renderizadas; sin widgets de relleno ni etiquetas duplicadas.

- [x] G9: La matriz cubre todos los comandos y áreas shipped/partial del frontend original sin filas pendientes
  CHECK: ./scripts/gtk-qa.sh parity
  EXPECT: GTK_GATE_OK parity
  EVIDENCE: gate `parity` = coverage 209/209 (96 nativos + 113 automation) + cero filas Pendiente en docs/gtk-parity.md; humo `parity-ops` verifica reabrir pestaña cerrada desde disco, doc_compare con cambios reales y autotag de accesibilidad.
  EVIDENCE: pending
