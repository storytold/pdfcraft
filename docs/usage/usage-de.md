# PdfCraft – Bedienungsanleitung

[English](usage.md) · [Alle Sprachen / Übersetzen](README.md) · [Projekt-README](../../README.md)

Englisch ist die Ausgangsfassung; Deutsch dient als erste Beispielübersetzung. Weitere Sprachen
sind nach demselben Aufbau willkommen (siehe Sprachindex).

Hier beginnen alltägliche PDF-Abläufe, Tastenkürzel und Verweise auf ausführliche Anleitungen.
Englische UI-Bezeichnungen bleiben erhalten, damit die Befehle in Menüs oder der Befehlspalette
auffindbar sind, auch wenn die Oberfläche eine andere Sprache verwendet. Abgeglichen mit Quellstand
[1dd2737](https://github.com/storytold/pdfcraft/tree/1dd2737b47a9c1c9562f0af563053d1bf510e4bf)
am 27.10.2026. Eine heruntergeladene Version kann davon abweichen.

## Inhalt

- [Öffnen, speichern und Werkzeuge finden](#öffnen-speichern-und-werkzeuge-finden)
- [Lesen, suchen und kopieren](#lesen-suchen-und-kopieren)
- [Seiten ordnen, zusammenführen und teilen](#seiten-ordnen-zusammenführen-und-teilen)
- [Kommentare, Formulare und Bearbeitung](#kommentare-formulare-und-bearbeitung)
- [Weitere Werkzeuge und Ausgabe](#weitere-werkzeuge-und-ausgabe)
- [Tastenkürzelreferenz](#tastenkürzelreferenz)
- [Ansichts- und Kontextsteuerung](#ansichts--und-kontextsteuerung)
- [Einstellungen, Grenzen und weitere Hilfe](#einstellungen-grenzen-und-weitere-hilfe)

## Öffnen, speichern und Werkzeuge finden

1. Unter **File › Open…** eine PDF auswählen oder auf Home eine zuletzt verwendete Datei öffnen.
   Ein verlangtes Öffnungspasswort eingeben. Vom Autor gesetzte Beschränkungen können Kopieren,
   Drucken oder Bearbeiten deaktivieren; **File › Security properties…** zeigt die Berechtigungen.
2. Eine Aufgabe unter **All tools** wählen oder mit **Ctrl+K** (**Cmd+K** unter macOS) einen Befehl
   suchen. Die Palette akzeptiert englische und übersetzte Namen sowie Befehls-IDs. Ein als noch in
   Entwicklung markiertes Werkzeug ist kein abgeschlossener Arbeitsablauf.
3. Änderungen mit **File › Save** speichern. **Save as…** erstellt eine separate Ausgabedatei.
   Ungespeicherte Tabs zeigen einen Punkt; beim Schließen wird nach ungespeicherten Änderungen gefragt.
   Nach einem unerwarteten Ende wird Wiederherstellung angeboten.

Installation, Downloads und Bauen aus dem Quellcode stehen im [README](../../README.md).

## Lesen, suchen und kopieren

Über das Seitenzahlfeld, Seitenminiaturen oder den Bookmarks-Bereich navigieren. Die Suche nach
Lesezeichentiteln findet auch verschachtelte Einträge. Unter **View › Page display** Einzel-/Doppelseite
und kontinuierliches Scrollen wählen. Für einen durchgehenden Streifen Single page wählen,
Continuous scrolling aktivieren und Show gaps between pages deaktivieren. Das entfernt Abstände
der Ansicht, nicht weiße PDF-Seitenränder. [Seitenanzeige](../page-display.md) erklärt Beispiele und Vorgaben.

Mit **Ctrl+F / Cmd+F** einen Suchtext eingeben und mit **Ctrl+G / Cmd+G** bzw. dem Kürzel für den
vorherigen Treffer wechseln. **Advanced search…** öffnet die erweiterte Suchoberfläche.
Das Auswahlwerkzeug (**V**) wählen, über Text ziehen und mit **Ctrl+C / Cmd+C** extrahierten Text
kopieren. Ein Doppelklick wählt ein Wort. Kopieren hängt von PDF-Berechtigungen und vorhandenem
Text ab; eine reine Bildseite benötigt zuerst **Scan & OCR**, damit Wörter such- und kopierbar sind.
**Edit › Column select** bzw. Alt/Option-Ziehen mit dem Auswahlwerkzeug begrenzt die Auswahl auf ein Rechteck.

Mit dem Handwerkzeug (**H**) verschieben oder beim Ziehen vorübergehend Space halten.
**File › Word count…** zählt das Dokument oder die aktuelle Seite; die [Wortzählanleitung](../word-count.md)
erklärt Ausschlüsse und Scans. Beim Kopieren von PDF-Text ist keine Übernahme vollständiger
Textverarbeitungsformatierung zugesichert.

## Seiten ordnen, zusammenführen und teilen

1. **Pages › Organize pages** öffnet die Seitenübersicht.
2. Eine Seite wählen, mit Ctrl-Klick/Cmd-Klick einzelne Seiten ergänzen oder mit Shift-Klick einen Bereich.
3. Über die Seitensteuerung die Auswahl drehen, löschen, einfügen oder verschieben. **Delete** entfernt
   ausgewählte Seiten; **Ctrl+A / Cmd+A** wählt alle Seiten, wenn die Übersicht die Tastatureingabe hat.
4. Die neue Reihenfolge prüfen und speichern. **Undo** macht eine Bearbeitung rückgängig, **Redo** stellt sie wieder her.

**File › Combine files…** verbindet PDFs zu einem Dokument. **Extract** kopiert ausgewählte Seiten
in ein anderes Dokument; **Split** teilt nach Seitenanzahl oder Trennstellen. In den jeweiligen
Datei-/Ausgabeoptionen den Speicherort wählen. Vorhandene Kommandozeilenrezepte stehen unter
[CLI-Stapelverarbeitung](../cli-batch.md).

## Kommentare, Formulare und Bearbeitung

| Aufgabe | Schritte / Einstieg | Ergebnis und Grenzen |
|---|---|---|
| PDF kommentieren | **Add a sticky note** oder ein Markierungswerkzeug in der Palette suchen, auf der Seite klicken/ziehen und **Comments panel** prüfen | Erstellt Anmerkungen; schreibt den ursprünglichen Text nicht um |
| Vorhandenes Formular ausfüllen | Textfelder, Kontrollkästchen oder Listen anklicken; Werte im **Form fields panel** prüfen und speichern | Benötigt ausfüllbare Felder und entsprechende Berechtigung |
| Formularfelder erstellen | **Prepare a form**, dann ein Feldwerkzeug oder **Detect form fields** wählen; Eigenschaften und Tab-Reihenfolge prüfen | Erstellt interaktive Felder; erkannte Felder vor der Weitergabe kontrollieren |
| Flaches Formular beschriften | **Fill & Sign: add text** suchen und Text platzieren | Erstellt eine sichtbare Anmerkung statt eines interaktiven Felds |
| Vorhandenen Text bearbeiten | **Edit a PDF › Edit text & images**, dann Textbereich anklicken und bearbeiten | Schrift-/Zeichenverfügbarkeit und PDF-Struktur können begrenzen; siehe [Textbearbeitung](../editing-existing-text.md) |
| Abbildung verschieben/vergrößern | In **Edit text & images** die Abbildung auswählen, sie oder eine Ecke ziehen; zum Drehen das Kontextmenü verwenden | Abbildungen können gruppierte Grafiken sein; siehe [Bildbearbeitung und Grenzen](../editing-images.md) |
| Mehrere Objekte verschieben | In **Edit text & images** mit Modifikatortaste anklicken oder Auswahlrechteck ziehen, dann die Gruppe verschieben | Jeweils auf einer Seite; siehe [Mehrfachauswahl](../editing-multiple-objects.md) |

Nach der Bearbeitung speichern. Soll die Quelldatei erhalten bleiben, zuerst **Save as…** wählen.

## Weitere Werkzeuge und Ausgabe

Die Palette bietet diese Einstiege. Den Optionen des jeweiligen Dialogs folgen und die Ausgabe
prüfen; die Übersicht verspricht keine vollständige Kompatibilität mit jeder PDF.

| Aufgabe | Einstieg und nächster Schritt |
|---|---|
| Scan erkennen | **Recognize text…** in Scan & OCR; Seiten/Optionen wählen, Erkennung starten und den suchbaren Text prüfen |
| Vertraulichen Seiteninhalt entfernen | Mit **Redact text and images** markieren, danach **Apply redactions…** ausführen; Markierungen allein entfernen keinen Inhalt. Gespeichertes Ergebnis prüfen |
| Verborgene Informationen entfernen | **Remove hidden information…** oder **Sanitize document…**; vor dem Anwenden prüfen, was entfernt wird |
| Passwortschutz | **Protect using password…**; Schutzoptionen festlegen und speichern, danach **Security properties…** prüfen |
| Sichtbare Unterschrift/Initialen | **Fill & Sign: sign** bzw. **Fill & Sign: initials** platzieren; das unterscheidet sich von einer digitalen Signatur mit Zertifikat |
| Digitale Signaturen | **Digitally sign**, **Signatures panel**, **Validate all signatures**; dem Zertifikatsablauf folgen und den gemeldeten Prüfstatus beachten |
| Barrierefreiheit | **Check for accessibility…** starten, Bericht und passende Werkzeuge für Tags/Alternativtexte prüfen |
| Export | Unter **File** stehen Bild-, Text-, Word-, PowerPoint-, HTML- und RTF-Export; Ziel wählen und Wiedergabetreue prüfen |
| Drucken | **File › Print…**; Seiten und Druckereinstellungen wählen, dann drucken (Desktop) |
| Dateigröße verringern | **File › Reduce file size…** oder **Optimize PDF…**; Optionen wählen und die entstandene Kopie prüfen |

Umgesetzter Umfang und Einschränkungen stehen in der [Formatreferenz](../file-format-parity.md)
und den [bekannten Lücken](../gaps.md). Office-Konvertierungen, erweiterte Signaturen und
Standardkonformitätsabläufe sind unvollständig; ein Menüeintrag garantiert keine professionelle Funktionsparität.

## Tastenkürzelreferenz

Die Tabelle enthält alle **25 Standardkürzel** aus dem
[Befehlsverzeichnis](../../crates/engine/src/commands.rs). Weitere Ansichts-/Werkzeugtasten folgen danach.
**Ctrl** bedeutet **Strg**, **Shift** bedeutet **Umschalt**, **Cmd** bedeutet Command (⌘).
Alt heißt auf macOS Option (⌥). **Delete** bedeutet **Entf**, **Home/End** bedeutet **Pos1/Ende**,
**PageUp/PageDown** bedeutet **Bild auf/ab**, **Space** bedeutet **Leertaste**. Schrägstriche trennen
Alternativen. Funktionstasten können Fn benötigen. Die Plattformspalten gelten für native Builds;
Browser oder Betriebssystem können Tasten abfangen, Satzzeichen hängen vom Tastaturlayout ab.

Die meisten Befehlskürzel sind außerhalb der Textbearbeitung nutzbar; Undo/Redo wirken in einem
fokussierten Texteditor auf dessen Text statt auf das Dokument. Modale Dialoge verarbeiten ihre
eigenen Tasten. Deaktivierte Befehle benötigen weiterhin das passende Dokument, eine Auswahl oder
Berechtigungen. **Help › Keyboard shortcuts** öffnet die Referenz in der Anwendung; dieser Dialog
zeigt Belegungen an und dient nicht zum Zuweisen eigener Kürzel.

| Befehl / Bedeutung | Windows / Linux / BSD | macOS |
|---|---|---|
| Open… — Öffnen | `Ctrl+O` | `Cmd+O` |
| Save — Speichern | `Ctrl+S` | `Cmd+S` |
| Save as… — Speichern unter | `Ctrl+Shift+S` | `Cmd+Shift+S` |
| Close file — Datei schließen | `Ctrl+W` | `Cmd+W` |
| Close all — Alle Dateien schließen | `Ctrl+Shift+W` | `Cmd+Shift+W` |
| Print… — Drucken | `Ctrl+P` | `Cmd+P` |
| Document properties… — Dokumenteigenschaften | `Ctrl+D` | `Cmd+D` |
| Undo — Rückgängig | `Ctrl+Z` | `Cmd+Z` |
| Redo — Wiederherstellen | `Ctrl+Shift+Z` | `Cmd+Shift+Z` |
| Find… — Suchen | `Ctrl+F` | `Cmd+F` |
| Advanced search… — Erweiterte Suche | `Ctrl+Shift+F` | `Cmd+Shift+F` |
| Find tools and commands… — Werkzeuge und Befehle suchen | `Ctrl+K` | `Cmd+K` |
| Fit visible — Sichtbaren Seiteninhalt einpassen | `Ctrl+3` | `Cmd+3` |
| Full screen mode — Vollbildmodus | `Ctrl+L` | `Cmd+L` |
| Read mode — Lesemodus | `Ctrl+H` | `Cmd+Ctrl+H` |
| Next tab — Nächster Tab | `Ctrl+Tab` | `Ctrl+Tab` |
| Previous tab — Vorheriger Tab | `Ctrl+Shift+Tab` | `Ctrl+Shift+Tab` |
| Previous view — Vorherige Ansicht | `Ctrl+[` | `Cmd+[` |
| Next view — Nächste Ansicht | `Ctrl+]` | `Cmd+]` |
| Go to page… — Gehe zu Seite | `Ctrl+Shift+N` | `Cmd+Shift+N` |
| Show or hide the side panel — Seitenbereich anzeigen/ausblenden | `F4` | `F4` |
| Show or hide the Tools panel — Werkzeugbereich anzeigen/ausblenden | `Shift+F4` | `Shift+F4` |
| Preferences… — Einstellungen | `Ctrl+,` | `Cmd+,` |
| JavaScript console… — JavaScript-Konsole | `Ctrl+J` | `Cmd+J` |
| New bookmark — Neues Lesezeichen | `Ctrl+B` | `Cmd+B` |

## Ansichts- und Kontextsteuerung

Hier bedeutet **Mod** Ctrl auf Windows/Linux/BSD und Cmd auf macOS. Vor Ansichtskürzeln ins
Dokument klicken; Textfelder, Formularfelder, Palette oder Dialoge können die Eingabe zuerst verarbeiten.

| Kontext | Tasten / Geste | Wirkung |
|---|---|---|
| Suche | Mod+G / Mod+Shift+G | Nächster/vorheriger Treffer, auch mit Fokus im Suchfeld |
| Textauswahl | Mod+C; Doppelklick | Kopiert ausgewählten Text; wählt ein Wort |
| Auswahl | Mod+A | Wählt Dokumenttext, Seiten in Organize oder Felder in Prepare a form; ein fokussierter Texteditor wählt seinen eigenen Text |
| Ausgewähltes Bild | Mod+C | Kopiert bei Berechtigung das Bild als Datei; im Web Save Image As verwenden |
| Desktop-Dokument mit Bild in Zwischenablage | Mod+V | Fügt das Bild auf der aktuellen Seite ein; Textfelder, Organize und Prepare a form haben eigenes Einfügeverhalten |
| Ansicht | Mod+1 / Mod+0 / Mod+2 | Originalgröße / ganze Seite / Seitenbreite |
| Ansicht | Mod+Plus oder Mod+Equals / Mod+Minus | Vergrößert/verkleinert; in Organize ändert das die Miniaturgröße |
| Organize pages | Mod+0 | Setzt den Miniaturzoom zurück |
| Ansicht | Mod+Shift+Plus oder Mod+Shift+Equals / Mod+Shift+Minus | Dreht die Ansicht im/gegen den Uhrzeigersinn; das ist eine andere Funktion als das Drehen gespeicherter Dokumentseiten |
| Ansicht | Pinch-Geste; Ctrl-Scrollen | Ändert den Zoom |
| Navigation | Home / End | Erste/letzte Seite |
| Navigation | Left / Right; Mod+PageUp / Mod+PageDown | Vorherige/nächste Seite |
| Navigation | PageUp / PageDown | Scrollt in kontinuierlicher Ansicht um einen Bildschirm, sonst Seitenwechsel |
| Navigation | Up / Down | Scrollt um eine Zeile; kann am Rand einer Einzelseite weiterblättern |
| Vollbildpräsentation | Space oder Enter / Backspace | Nächste/vorherige Seite |
| Freie Dokumentfläche | V / H; Space halten | Auswahl-/Handwerkzeug; vorübergehendes Verschieben bis Space losgelassen wird |
| Ausgewählte Seiten in Organize | Delete | Löscht ausgewählte Seiten |
| Ausgewählte Objekte / Anmerkungen / vorbereitete Felder | Pfeiltasten | Verschiebt die Auswahl, wenn das Werkzeug die Tasten verarbeitet, statt Seiten zu wechseln |
| Edit text & images | Shift/Ctrl/Cmd-Klick; durch leeren Bereich ziehen | Schaltet Objekte in der Auswahl um; zieht ein Auswahlrechteck |
| Auswahl / Suche / Vollbild | Escape | Hebt den nächsten aktiven Kontext auf bzw. bricht ihn ab; kann Suche oder Vollbild beenden |

Quellen: [Ansichtsnavigation](../../crates/ui-egui/src/canvas/navigation.rs),
[Tastaturverarbeitung](../../crates/ui-egui/src/app/keys.rs),
[Kürzelreferenz der Anwendung](../../crates/ui-egui/src/dialogs.rs),
[Zwischenablage](../../crates/ui-egui/src/app/clipboard.rs),
[Bildkopie](../../crates/ui-egui/src/canvas/image_pick.rs),
[Einfügen von Bildern](../../crates/ui-egui/src/tools/content_ui.rs),
[Mehrfachauswahl](../editing-multiple-objects.md).

## Einstellungen, Grenzen und weitere Hilfe

**Edit › Preferences…** (Ctrl+, / Cmd+,) enthält Anwendungseinstellungen wie Oberflächensprache
und Vorgaben für Dokument/Ansicht. Der Werkzeugbereich kann links oder rechts stehen; seine innere
Kante lässt sich zum Ändern der Breite ziehen. **View › Reset side panels** stellt die Standardanordnung
wieder her. F4 schaltet den Seitenbereich um, Shift+F4 den Werkzeugbereich.

Desktop und Web unterscheiden sich: Der geprüfte Web-Build bietet weder Drucken noch Wiederöffnen
eines geschlossenen dateibasierten Tabs oder Anzeigen seiner Datei im System-Dateimanager.
Bilddateien lassen sich nicht in die Systemzwischenablage kopieren; dafür **Save Image As** nutzen.
Vom Browser abgefangene Kürzel und Zwischenablageberechtigungen können die Bedienung ebenfalls
beeinflussen. Details stehen in den vorhandenen Referenzen:

| Thema | Dokumentation |
|---|---|
| Seitenlayout und Abstände | [Seitenanzeige](../page-display.md) |
| Text, Bilder und Gruppen | [Textbearbeitung](../editing-existing-text.md), [Bilder](../editing-images.md), [Mehrfachauswahl](../editing-multiple-objects.md) |
| Text zählen | [Wortzählung](../word-count.md) |
| Oberflächensprache | [Lokalisierung](../localization.md) |
| Startprobleme | [Windows-Renderer](../windows-renderer-startup.md), [Entwicklung/Logs](../development.md) |
| Automatisierung und Stapelverarbeitung | [CLI-Stapelverarbeitung](../cli-batch.md), [MCP](../mcp.md) |
| Kompatibilität und offene Arbeit | [Dateiformate](../file-format-parity.md), [UI-Abdeckung](../ui-parity.md), [Lücken](../gaps.md), [Roadmap](../roadmap.md) |

Dieses erste Handbuch deckt die Kernabläufe, alle Standardkürzel des Befehlsverzeichnisses und die
oben aufgeführten Ansichts-/Kontextsteuerungen ab. Es ist noch keine vollständige Anleitung für
jedes Werkzeug oder jeden Dialog. Ausführliche Abläufe für Signaturen, Barrierefreiheitskorrekturen,
OCR-Optionen, Optimierung, komplexe Formulare und weitere Dialog-/Werkzeuggesten müssen noch ergänzt
und geprüft werden. Verlinkte Themenseiten bleiben für ihre Details maßgeblich und sind derzeit
überwiegend englisch. Roadmap-Einträge sind Pläne, keine Anleitung für bereits verfügbare Funktionen.

Für Beitragende: Bei sichtbaren Änderungen die englische Anleitung und vorhandene Übersetzungen aktualisieren und relevante Tasten
sowie Links prüfen; siehe [AGENTS.md](../../AGENTS.md#user-documentation).
