use super::*;
use crate::{Block, Drawing, Image};

/// A block of 10-point text from `y0` to `y1`.
fn block(text: &str, y0: f64, y1: f64) -> Block {
    Block {
        text: text.into(),
        rect: [72.0, y0, 300.0, y1],
        size: 10.0,
        bold: false,
        italic: false,
        color: [0.0; 3],
        rotation: 0.0,
        space_around: [false; 2],
    }
}

/// A US Letter page: its margin bands are the 79.2 points at its top and bottom.
fn page(blocks: Vec<Block>) -> Page {
    Page { width: 612.0, height: 792.0, blocks, ..Page::default() }
}

fn running(pages: &[Page]) -> Vec<Vec<bool>> {
    structure(pages).into_iter().map(|s| s.running).collect()
}

#[test]
fn running_headers_and_page_numbers_repeat_in_the_margins() {
    let mut pages = vec![page(vec![block("XY123 Datasheet", 600.0, 624.0)])];
    for i in 1..5 {
        let mut blocks = vec![
            block("XY123 Low-noise amplifier", 740.0, 750.0),
            block(&format!("Page {} of 5", i + 1), 30.0, 40.0),
            block("Electrical characteristics", 700.0, 710.0),
        ];
        if i == 2 {
            blocks.push(block("Preliminary", 760.0, 770.0));
        }
        pages.push(page(blocks));
    }
    let found = running(&pages);
    assert_eq!(found[0], [false], "the title page has no header");
    for p in &found[1..] {
        assert_eq!(p[..3], [true, true, false], "the header and the page number, not the text below the margin");
    }
    assert!(!found[2][3], "a note on one page only");
}

#[test]
fn numbers_in_a_margin_must_follow_the_pages() {
    let lots = [page(vec![block("Lot 104", 30.0, 40.0)]), page(vec![block("Lot 230", 30.0, 40.0)])];
    assert_eq!(running(&lots), [[false], [false]]);
    let front = ["i", "ii", "iii", "iv"].map(|n| page(vec![block(n, 30.0, 40.0)]));
    assert_eq!(running(&front), [[true]; 4], "roman page numbers");
    let constant = [page(vec![block("Rev. 1.2  2026", 30.0, 40.0)]), page(vec![block("Rev. 1.2 2026", 30.0, 40.0)])];
    assert_eq!(running(&constant), [[true], [true]], "numbers that stay, spaces collapsed");
}

#[test]
fn a_continued_caption_in_the_margin_is_not_a_running_header() {
    let pages = [
        page(vec![block("Table 3. Pin functions", 600.0, 610.0)]),
        page(vec![block("Table 3. Pin functions (continued)", 740.0, 750.0), block("FIG. 2 (continued)", 30.0, 40.0)]),
        page(vec![block("Table 3. Pin functions (continued)", 740.0, 750.0), block("FIG. 2 (continued)", 30.0, 40.0)]),
    ];
    assert_eq!(running(&pages), [vec![false], vec![false, false], vec![false, false]]);
}

#[test]
fn text_repeated_on_few_pages_or_turned_is_not_running() {
    let mut pages: Vec<Page> = (0..10).map(|_| page(vec![block("Draft", 20.0, 30.0)])).collect();
    for p in &mut pages {
        p.blocks[0].rotation = 90.0;
    }
    for i in [3, 7] {
        pages[i].blocks.push(block("Notes", 740.0, 750.0));
    }
    let found = running(&pages);
    assert!(found.iter().all(|p| p.iter().all(|r| !r)), "{found:?}");
}

/// A run of `text` in `size` from `x0` to `x1`, its baseline at `base`.
fn run(text: &str, x0: f64, x1: f64, base: f64, size: f64) -> TextRun {
    TextRun {
        text: text.into(),
        rect: [x0, base - 0.2 * size, x1, base + 0.8 * size],
        words: Vec::new(),
        baseline: [x0, base],
        size,
        bold: false,
        italic: false,
        color: [0.0; 3],
        rotation: 0.0,
        block: Some(0),
    }
}

#[test]
fn smaller_runs_beside_a_line_above_or_below_its_baseline_are_scripts() {
    let mut turned = run("2", 100.0, 104.0, 304.0, 6.0);
    turned.rotation = 90.0;
    let runs = vec![
        run("E = mc", 72.0, 110.0, 700.0, 10.0),
        run("2", 110.0, 114.0, 704.0, 6.0),
        run("H", 72.0, 80.0, 650.0, 10.0),
        run("2", 80.0, 84.0, 648.0, 6.0),
        run("O", 84.0, 92.0, 650.0, 10.0),
        run("x", 72.0, 78.0, 600.0, 10.0),
        run("i", 78.0, 80.0, 598.0, 6.0),
        run("2", 78.0, 82.0, 604.0, 6.0),
        // A drop cap beside the first line it starts.
        run("T", 72.0, 90.0, 500.0, 24.0),
        run("he story", 92.0, 140.0, 512.0, 10.0),
        // Small capitals on the line's baseline.
        run("Note", 72.0, 95.0, 450.0, 10.0),
        run("SMALL", 96.0, 120.0, 450.0, 8.0),
        // Raised, but far along the line.
        run("foot", 72.0, 95.0, 400.0, 10.0),
        run("1", 300.0, 304.0, 404.0, 6.0),
        run("Turned", 72.0, 100.0, 300.0, 10.0),
        turned,
        run("long", 72.0, 95.0, 250.0, 10.0),
        run("a longer raised run", 95.0, 160.0, 254.0, 6.0),
    ];
    let pages = [Page { runs, ..page(Vec::new()) }];
    use Script::{Baseline as B, Sub, Super};
    assert_eq!(structure(&pages)[0].scripts, [B, Super, B, Sub, B, B, Sub, Super, B, B, B, B, B, B, B, B, B, B],);
}

#[test]
fn an_export_without_runs_has_no_scripts() {
    let pages = [page(vec![block("Text", 400.0, 410.0)])];
    assert_eq!(structure(&pages), [PageStructure { running: vec![false], ..PageStructure::default() }]);
}

#[test]
fn a_header_on_every_page_of_a_long_document_is_found_quickly() {
    let pages: Vec<Page> = (0..20_000).map(|i| page(vec![block("Header", 740.0, 750.0), block(&format!("{}", i + 1), 30.0, 40.0)])).collect();
    assert!(running(&pages).iter().all(|p| p == &[true, true]));
}

#[test]
fn captions_start_with_a_label_and_a_number() {
    let label = |t: &str| figures::caption_label(t).map(|(k, n)| (k, n.to_owned()));
    use CaptionKind::{Figure as F, Table as T};
    for (text, want) in [
        ("Figure 3. Typical application", Some((F, "3"))),
        ("Table 2-1 Pin descriptions", Some((T, "2-1"))),
        ("FIG. 12: Timing", Some((F, "12"))),
        ("Fig 4b \u{2014} Detail", Some((F, "4b"))),
        ("Table A.4", Some((T, "A.4"))),
        ("  Table 3 (continued)", Some((T, "3"))),
        ("Figure 3 shows the circuit.", None),
        ("Figure 3, which", None),
        ("Tables 3 and 4", None),
        ("Figure3", None),
        ("Figurehead 2", None),
        ("Table of contents", None),
    ] {
        assert_eq!(label(text), want.map(|(k, n)| (k, n.to_owned())), "{text}");
    }
}

#[test]
fn a_caption_that_repeats_an_earlier_one_continues_it() {
    let pages = [
        page(vec![block("Table 3. Pin functions", 600.0, 610.0)]),
        page(vec![
            block("Table 3. Pin functions (continued)", 700.0, 710.0),
            block("Figure 3 shows the circuit.", 500.0, 510.0),
            block("Figure 4. Block diagram", 300.0, 310.0),
        ]),
    ];
    let found: Vec<Vec<Caption>> = structure(&pages).into_iter().map(|s| s.captions).collect();
    let caption = |block, kind, number: &str, continued| Caption { block, kind, number: number.into(), continued };
    assert_eq!(
        found,
        [
            vec![caption(0, CaptionKind::Table, "3", false)],
            vec![caption(0, CaptionKind::Table, "3", true), caption(2, CaptionKind::Figure, "4", false)],
        ]
    );
}

fn drawing(rect: [f64; 4]) -> Drawing {
    Drawing { rect, stroked: true, filled: false }
}

fn image(rect: [f64; 4]) -> Image {
    Image { ext: "png", bytes: std::sync::Arc::from(Vec::new()), rect, size: [rect[2] - rect[0], rect[3] - rect[1]], rotation: 0.0 }
}

fn at(text: &str, rect: [f64; 4]) -> Block {
    Block { rect, ..block(text, 0.0, 0.0) }
}

#[test]
fn a_figure_is_the_drawing_and_labels_next_to_its_caption() {
    let blocks = vec![
        at("A paragraph of body text above the chart, long enough to be text rather than a label of it.", [72.0, 600.0, 540.0, 640.0]),
        at("100", [80.0, 550.0, 98.0, 560.0]),
        at("25", [118.0, 388.0, 130.0, 398.0]),
        at("Temperature (\u{b0}C)", [200.0, 372.0, 300.0, 382.0]),
        at("Figure 2. Output power", [100.0, 350.0, 300.0, 362.0]),
        at("Text under the caption", [100.0, 300.0, 300.0, 310.0]),
    ];
    let mut background = drawing([0.0, 0.0, 612.0, 792.0]);
    background.filled = true;
    let drawings = vec![
        background,
        drawing([100.0, 400.0, 400.0, 560.0]),
        drawing([120.0, 400.0, 140.0, 500.0]),
        drawing([160.0, 400.0, 180.0, 450.0]),
        drawing([72.0, 100.0, 540.0, 100.0]),
    ];
    let pages = [Page { drawings, images: vec![image([400.0, 150.0, 500.0, 250.0])], ..page(blocks) }];
    let s = &structure(&pages)[0];
    assert_eq!(
        s.figures,
        [Figure { caption: 0, rect: [80.0, 372.0, 400.0, 560.0], drawings: vec![1, 2, 3], images: Vec::new(), labels: vec![1, 2, 3] }],
        "the axis title joins through the tick label; the background, the footer rule and the picture without a caption stay out"
    );
}

#[test]
fn each_caption_takes_the_nearest_free_drawing() {
    let blocks = vec![at("Figure 1. Above", [100.0, 580.0, 300.0, 590.0]), at("Figure 2. Below", [100.0, 450.0, 300.0, 460.0])];
    let drawings = vec![drawing([100.0, 600.0, 300.0, 700.0]), drawing([100.0, 470.0, 300.0, 570.0])];
    let s = &structure(&[Page { drawings, ..page(blocks) }])[0];
    assert_eq!(s.figures.iter().map(|f| (f.caption, f.drawings.clone())).collect::<Vec<_>>(), [(0, vec![0]), (1, vec![1])]);

    let blocks = vec![at("Figure 5: A picture under its caption", [100.0, 700.0, 300.0, 710.0])];
    let s = &structure(&[Page { images: vec![image([100.0, 600.0, 300.0, 690.0])], ..page(blocks) }])[0];
    assert_eq!(s.figures.iter().map(|f| (f.caption, f.images.clone())).collect::<Vec<_>>(), [(0, vec![0])]);

    let blocks = vec![at("Figure 6. Too far", [100.0, 450.0, 300.0, 460.0]), at("Figure 7. Beside, not under", [400.0, 580.0, 550.0, 590.0])];
    let s = &structure(&[Page { drawings: vec![drawing([100.0, 600.0, 300.0, 700.0])], ..page(blocks) }])[0];
    assert!(s.figures.is_empty(), "{:?}", s.figures);

    let blocks = vec![at("Figure 10. Nearer", [100.0, 590.0, 300.0, 598.0]), at("Figure 11. Further", [100.0, 710.0, 300.0, 720.0])];
    let s = &structure(&[Page { drawings: vec![drawing([100.0, 600.0, 300.0, 700.0])], ..page(blocks) }])[0];
    assert_eq!(s.figures.iter().map(|f| f.caption).collect::<Vec<_>>(), [0], "one drawing, one caption");
}

#[test]
fn a_ruled_table_is_not_a_figure() {
    let mut blocks = Vec::new();
    let mut rules = Vec::new();
    for (r, y) in [700.0, 680.0, 660.0].into_iter().enumerate() {
        for (c, x) in [100.0, 200.0, 300.0].into_iter().enumerate() {
            blocks.push(at(&format!("r{r}c{c}"), [x + 4.0, y + 4.0, x + 40.0, y + 14.0]));
        }
    }
    for y in [720.0, 700.0, 680.0, 660.0] {
        rules.push([100.0, y, 400.0, y]);
    }
    for x in [100.0, 200.0, 300.0, 400.0] {
        rules.push([x, 660.0, x, 720.0]);
    }
    assert_eq!(crate::tables(&blocks, &rules).0.len(), 1, "the grid is a table");
    blocks.push(at("Figure 8. Under a table", [100.0, 640.0, 300.0, 650.0]));
    let drawings = rules.iter().map(|&r| drawing(r)).collect();
    let s = &structure(&[Page { drawings, rules, ..page(blocks) }])[0];
    assert_eq!(s.captions.len(), 1);
    assert!(s.figures.is_empty(), "{:?}", s.figures);
}

#[test]
fn a_page_of_scattered_paths_gives_up_on_figures_within_its_budget() {
    // Dots further apart than the paths of one drawing are.
    let dot = |i: usize| [(i % 100) as f64 * 13.0, (i / 100) as f64 * 13.0];
    let drawings = (0..6000).map(dot).map(|[x, y]| drawing([x, y, x + 0.5, y + 0.5])).collect();
    let pages = [Page { drawings, ..page(vec![at("Figure 9. Dots", [100.0, 780.0, 300.0, 790.0])]) }];
    assert!(structure(&pages)[0].figures.is_empty());
}

/// The blocks of a table whose rows start at `top`, one row every 14 points, its columns at
/// x = 100, 200, 300 and 400; an empty text leaves its cell empty, and the first `bold` rows are
/// bold.
fn grid(rows: &[&[&str]], top: f64, bold: usize) -> Vec<Block> {
    let mut out = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let y = top - 14.0 * r as f64;
        for (c, text) in row.iter().enumerate().filter(|(_, t)| !t.is_empty()) {
            let x = 100.0 + 100.0 * c as f64;
            out.push(Block { bold: r < bold, ..at(text, [x, y - 10.0, x + 60.0, y]) });
        }
    }
    out
}

/// The grid of ruled lines around the cells of [`grid`]'s `rows` rows of `cols` columns.
fn grid_rules(rows: usize, cols: usize, top: f64) -> Vec<[f64; 4]> {
    let bottom = top - 14.0 * rows as f64;
    let right = 100.0 + 100.0 * cols as f64;
    let mut out: Vec<[f64; 4]> = (0..=rows).map(|r| top + 2.0 - 14.0 * r as f64).map(|y| [96.0, y, right, y]).collect();
    out.extend((0..=cols).map(|c| 96.0 + 100.0 * c as f64).map(|x| [x, bottom + 2.0, x, top + 2.0]));
    out
}

fn tables_of(pages: &[Page]) -> Vec<Vec<TableStructure>> {
    structure(pages).into_iter().map(|s| s.tables).collect()
}

#[test]
fn a_table_header_is_its_bold_rows_or_a_group_over_sub_columns() {
    let bold = grid(&[&["Pin", "Name", "Function"], &["1", "VCC", "Supply"], &["2", "GND", "Ground"]], 700.0, 1);
    let t = &tables_of(&[page(bold)])[0];
    assert_eq!(t.iter().map(|t| (t.table.rows.len(), t.header_rows)).collect::<Vec<_>>(), [(3, 1)]);

    let mut grouped =
        grid(&[&["Parameter", "", "", "Unit"], &["", "Min", "Max", ""], &["Voltage", "1.8", "3.6", "V"], &["Current", "", "10", "mA"]], 700.0, 0);
    grouped.push(at("Limits", [200.0, 690.0, 380.0, 700.0]));
    let t = &tables_of(&[page(grouped)])[0];
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].table.rows[0].iter().map(|c| (c.text.as_str(), c.span)).collect::<Vec<_>>(), [("Parameter", 1), ("Limits", 2), ("Unit", 1)]);
    assert_eq!(t[0].header_rows, 2);
    assert!(t[0].row_spans.iter().flatten().all(|&s| s == 1), "a missing minimum is not spanned: {:?}", t[0].row_spans);

    let plain = grid(&[&["1", "VCC", "Supply"], &["2", "GND", "Ground"], &["3", "NC", "Not connected"]], 700.0, 0);
    assert_eq!(tables_of(&[page(plain)])[0][0].header_rows, 0);
}

#[test]
fn empty_cells_under_a_parameter_are_spanned_by_it_unless_rules_part_them() {
    let rows: &[&[&str]] = &[
        &["Parameter", "Symbol", "Conditions", "Max"],
        &["Supply current", "ICC", "Active", "10"],
        &["", "", "Standby", "1"],
        &["", "", "Sleep", "0.1"],
        &["Leakage", "IL", "", "1"],
    ];
    let t = &tables_of(&[page(grid(rows, 700.0, 1))])[0][0];
    assert_eq!(t.row_spans, [vec![1, 1, 1, 1], vec![3, 3, 1, 1], vec![0, 0, 1, 1], vec![0, 0, 1, 1], vec![1, 1, 1, 1]]);

    let ruled = Page { rules: grid_rules(5, 4, 700.0), ..page(grid(rows, 700.0, 1)) };
    let t = &tables_of(&[ruled])[0];
    assert!(t[0].table.ruled.is_some());
    assert!(t[0].row_spans.iter().flatten().all(|&s| s == 1), "{:?}", t[0].row_spans);
}

#[test]
fn a_table_has_its_caption_and_the_notes_under_it() {
    let rows: &[&[&str]] = &[&["Pin", "Name", "Function"], &["1", "VCC", "Supply (1)"], &["2", "EN", "Enable (2)"]];
    for caption_x in [150.0, 100.0] {
        let mut blocks = vec![at("Table 2. Pin functions", [caption_x, 706.0, caption_x + 200.0, 716.0])];
        blocks.extend(grid(rows, 700.0, 1));
        let first_note = blocks.len();
        blocks.push(at("Note 1: Decouple with 100 nF.", [100.0, 648.0, 300.0, 658.0]));
        blocks.push(at("(2) Internal pull-up.", [100.0, 636.0, 300.0, 646.0]));
        blocks.push(at("The device starts when EN is high, which an internal pull-up keeps it by default.", [100.0, 600.0, 540.0, 630.0]));
        blocks.push(at("Table 3. Too far", [100.0, 400.0, 300.0, 410.0]));
        let s = &structure(&[page(blocks)])[0];
        assert_eq!(s.tables.len(), 1, "caption at x = {caption_x}");
        let t = &s.tables[0];
        assert_eq!((t.caption, t.table.rows.len(), t.header_rows), (Some(0), 3, 1), "caption at x = {caption_x}: {:?}", t.table.rows);
        assert_eq!(t.notes, [first_note, first_note + 1]);
        assert!(t.table.rect[3] <= 706.0, "the box ends under the caption");
    }
}

#[test]
fn a_table_continues_the_table_that_ends_the_page_before() {
    let head: &[&str] = &["Pin", "Name", "Function"];
    let first = || page(grid(&[head, &["1", "VCC", "Supply"], &["2", "GND", "Ground"]], 200.0, 1));
    let continues =
        |second: Page| tables_of(&[first(), second])[1].iter().map(|t| (t.continues, t.repeats_header, t.header_rows)).collect::<Vec<_>>();

    let repeated = page(grid(&[head, &["3", "EN", "Enable"], &["4", "NC", "Not connected"]], 700.0, 1));
    assert_eq!(continues(repeated), [(Some((0, 0)), true, 1)], "its header repeats");

    let bare = || grid(&[&["3", "EN", "Enable"], &["4", "NC", "Not connected"], &["5", "OUT", "Output"]], 700.0, 0);
    assert_eq!(continues(page(bare())), [(Some((0, 0)), false, 0)], "nothing comes between them");

    let mut after_text = bare();
    after_text.push(at("A paragraph before the next table.", [110.0, 720.0, 400.0, 730.0]));
    assert_eq!(continues(page(after_text)), [(None, false, 0)]);

    let mut moved = bare();
    for b in &mut moved {
        b.rect[0] += 20.0;
        b.rect[2] += 20.0;
    }
    assert_eq!(continues(page(moved)), [(None, false, 0)], "its columns do not line up");

    let captioned = |label: &str| {
        let mut blocks = vec![at("Text that ends the page.", [100.0, 60.0, 300.0, 70.0]), at(label, [150.0, 706.0, 350.0, 716.0])];
        blocks.extend(bare());
        blocks
    };
    let mut before = first();
    before.blocks.insert(0, at("Table 4. Pins", [150.0, 206.0, 350.0, 216.0]));
    let pages = |label: &str| [before.clone(), page(captioned(label))];
    let t = &tables_of(&pages("Table 4. Pins (continued)"))[1];
    assert_eq!(t.iter().map(|t| t.continues).collect::<Vec<_>>(), [Some((0, 0))], "its caption continues the one before");
    let t = &tables_of(&pages("Table 5. Other pins"))[1];
    assert_eq!(t.iter().map(|t| t.continues).collect::<Vec<_>>(), [None]);
    let earlier = page(vec![at("Table 3. Timing", [150.0, 500.0, 350.0, 510.0])]);
    let t = &tables_of(&[earlier, before.clone(), page(captioned("Table 3. Timing (continued)"))])[2];
    assert_eq!(t.iter().map(|t| t.continues).collect::<Vec<_>>(), [None], "it continues another table than the one before");
}

#[test]
fn a_page_of_many_tables_with_notes_stays_within_its_budget() {
    let mut blocks = Vec::new();
    for k in 0..400 {
        let top = 790.0 - 60.0 * k as f64;
        blocks.extend(grid(&[&["a", "b", "c"], &["1", "2", "3"], &["4", "5", "6"]], top, 1));
        blocks.push(at("* A note.", [110.0, top - 52.0, 300.0, top - 42.0]));
    }
    let started = std::time::Instant::now();
    let s = &structure(&[page(blocks)])[0];
    assert_eq!(s.tables.len(), 400);
    assert!(s.tables[0].notes.len() == 1 && s.tables[399].notes.is_empty(), "the first tables have their notes, the last ones none");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

fn entry(title: &str, level: usize, page: usize, top: Option<f64>) -> OutlineEntry {
    OutlineEntry { title: title.into(), level, page, top }
}

#[test]
fn each_bookmark_names_the_block_with_its_title_nearest_where_it_goes() {
    let pages = [
        page(vec![
            block("Overview", 760.0, 770.0),
            block("1 Introduction", 700.0, 710.0),
            block("The introduction says what the device is for and who should read this.", 600.0, 640.0),
            block("Overview", 500.0, 510.0),
        ]),
        page(vec![
            block("Overview", 760.0, 770.0),
            block("Scope", 700.0, 710.0),
            block("A. 2 Scope", 400.0, 410.0),
            block("Scope", 300.0, 310.0),
            block("Scope", 200.0, 210.0),
        ]),
    ];
    let outline = [
        entry("INTRODUCTION:", 0, 0, None),
        entry("Overview", 1, 0, Some(770.0)),
        entry("Scope", 1, 1, None),
        entry("1.1 Scope", 1, 1, Some(205.0)),
        entry("Scope", 1, 1, Some(305.0)),
        entry("Scope", 1, 1, Some(700.0)),
        entry("Missing", 1, 1, None),
        entry("Beyond the document", 0, 9, None),
    ];
    let s = structure_with_outline(&pages, &outline);
    assert_eq!(s[0].running, [true, false, false, false], "the header repeats");
    let heading = |entry, level, block| Heading { entry, level, block };
    assert_eq!(s[0].headings, [heading(0, 0, Some(1)), heading(1, 1, Some(3))], "not the running header, though it is nearer");
    assert_eq!(
        s[1].headings,
        [heading(2, 1, Some(1)), heading(3, 1, Some(4)), heading(4, 1, Some(3)), heading(5, 1, None), heading(6, 1, None)],
        "the topmost without a height, the nearest with one; each block once"
    );
}

#[test]
fn mentions_of_figures_and_tables_point_to_their_first_caption() {
    let pages = [
        page(vec![
            block("Figure 1. Block diagram", 600.0, 610.0),
            block("See Figure 1 and table 2-1; Figures 3, figure 9b, Table 2-1xy and subtable 4.", 500.0, 520.0),
        ]),
        page(vec![block("Table 2-1 Pin descriptions (see Fig. 1)", 600.0, 610.0), block("Table 2-1 (continued)", 300.0, 310.0)]),
    ];
    let s = structure(&pages);
    let found = |p: usize| {
        s[p].references
            .iter()
            .map(|r| (r.block, &pages[p].blocks[r.block].text[r.range.clone()], r.kind, r.number.as_str(), r.target))
            .collect::<Vec<_>>()
    };
    use CaptionKind::{Figure as F, Table as T};
    assert_eq!(found(0), [(1, "Figure 1", F, "1", Some((0, 0))), (1, "table 2-1", T, "2-1", Some((1, 0))), (1, "figure 9b", F, "9b", None)]);
    assert_eq!(found(1), [(0, "Fig. 1", F, "1", Some((0, 0)))], "a caption's own label is not a mention");
}
