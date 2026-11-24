//! Headless UI tests for the Combine files grid: a tile per file, selected, moved and undone like the
//! list's rows; thumbnails never stale; a file shown as its pages; the preview.

use std::time::Duration;

use crate::support::AppHarness as _;
use craft::ui::primitives::textures::{TextureState, ThumbRequest};
use craft::ui::tile_grid::TileZoom;
use egui::accesskit::Role;
use egui::{Key, Modifiers, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};
use pdfcraft_ui_egui::settings::import_json;
use pdfcraft_ui_egui::{CombineView, FilePurpose, PdfCraftApp};

/// An `n`-page document with a proper xref table; page `i` shows "Page i+1".
fn fixture(n: usize) -> Vec<u8> {
    fixture_sized(n, 200.0, 300.0)
}

/// The same, with pages of `w` × `h` points.
fn fixture_sized(n: usize, w: f32, h: f32) -> Vec<u8> {
    fixture_pages(&vec![(w, h); n])
}

/// A document with one page of each of `sizes` (width, height in points); page `i` shows "Page i+1".
fn fixture_pages(sizes: &[(f32, f32)]) -> Vec<u8> {
    let n = sizes.len();
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for (i, (w, h)) in sizes.iter().enumerate() {
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
            5 + 2 * i
        ));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// A two-page fixture that opens with `user` and allows everything with `owner`.
fn protected(user: &str, owner: &str) -> Vec<u8> {
    protected_with(pdfcraft_cos::Algorithm::Aes256, user, owner)
}

fn protected_with(algorithm: pdfcraft_cos::Algorithm, user: &str, owner: &str) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(fixture(2))).unwrap();
    doc.set_encryption_legacy(&pdfcraft_cos::NewEncryption {
        algorithm,
        user_password: user,
        owner_password: owner,
        permissions: -1,
        encrypt_metadata: true,
        seed: [4; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

/// The Combine tab with these files (name, bytes), in its default (grid) view.
fn grid_of(files: Vec<(&str, Vec<u8>)>, size: egui::Vec2) -> Harness<'static, PdfCraftApp> {
    grid_after(files, size, 4)
}

/// The same, after only `steps` frames.
fn grid_after(files: Vec<(&str, Vec<u8>)>, size: egui::Vec2, steps: usize) -> Harness<'static, PdfCraftApp> {
    let files: Vec<(String, Vec<u8>)> = files.into_iter().map(|(n, b)| (n.to_string(), b)).collect();
    let mut h = Harness::builder().with_size(size).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.use_files(&egui::Context::default(), FilePurpose::Combine, files);
            app
        }
    });
    h.run_steps(steps);
    h
}

fn grid_of_pages(names: &[(&str, usize)]) -> Harness<'static, PdfCraftApp> {
    grid_of(names.iter().map(|(n, p)| (*n, fixture(*p))).collect(), vec2(1400.0, 900.0))
}

/// A ⌘-click on the node named `label`, with ⌘ held first, as a platform reports it.
fn command_click(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    let at = h.get_by_label(label).rect().center();
    h.event(egui::Event::ModifiersChanged(Modifiers::COMMAND));
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::COMMAND });
    }
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
}

fn names(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    h.state().combine_draft.iter().map(|f| f.name.clone()).collect()
}

/// The rule in `view`: only files with the same bytes are warned, and undo and redo follow it.
fn check_duplicate_warnings(view: &str) {
    let first = fixture_sized(1, 200.0, 300.0);
    let other = fixture_sized(1, 300.0, 200.0);
    assert_eq!(first.len(), other.len());
    assert_ne!(first, other);
    let mut h = grid_of(vec![("report.pdf", first.clone()), ("report.pdf", other)], vec2(1400.0, 900.0));
    h.set_option("combine-view", view).unwrap();
    h.run_steps(2);
    let warnings = |h: &Harness<'static, PdfCraftApp>| h.query_all_by_label_contains("Added more than once").count();
    assert_eq!(warnings(&h), 0, "different equal-sized PDFs are not duplicates in {view}");
    let ctx_h = h.ctx.clone();
    h.state_mut().use_files(&ctx_h, FilePurpose::Combine, vec![("report.pdf".into(), first)]);
    h.run_steps(3);
    assert_eq!(warnings(&h), 2, "only the two identical files are warned in {view}");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(warnings(&h), 0, "undo removes the duplicate warning in {view}");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(3);
    assert_eq!(warnings(&h), 2, "redo restores the duplicate warning in {view}");
}

#[test]
fn list_duplicate_warnings_compare_contents_and_follow_undo() {
    check_duplicate_warnings("list");
    check_duplicate_warnings("grid");
}

/// Run frames until `done`, giving render threads time; whether it happened.
fn settle(h: &mut Harness<'static, PdfCraftApp>, done: impl Fn(&PdfCraftApp) -> bool) -> bool {
    for _ in 0..400 {
        h.run_steps(1);
        if done(h.state()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// What the grid's thumbnails are read at.
fn request(h: &Harness<'static, PdfCraftApp>, name: &str, stacked: bool) -> ThumbRequest {
    let env = craft::ui::runtime::env::Env::of_root(&h.ctx);
    let tokens = &env.tokens;
    let side = card(h, name).width() - 2.0 * tokens.size.icon.md - if stacked { 2.0 * tokens.space.gap.tight } else { 0.0 };
    ThumbRequest::for_cell(side, h.ctx.pixels_per_point())
}

/// Each file's grid thumbnail, as the harness's frames have stored them.
fn thumbnails(h: &Harness<'static, PdfCraftApp>, request: &ThumbRequest) -> Vec<TextureState> {
    h.state().combine_thumbnails(&h.ctx, request)
}

fn ready(t: &TextureState) -> bool {
    matches!(t, TextureState::Ready(_))
}

/// The picture's size in pixels, when it is drawn.
fn shape(t: &TextureState) -> Option<(f32, f32)> {
    match t {
        TextureState::Ready(texture) => {
            let size = texture.size_vec2();
            Some((size.x, size.y))
        }
        _ => None,
    }
}

/// A picture wider than it is high.
fn landscape(t: &TextureState) -> bool {
    shape(t).is_some_and(|(w, h)| w > h)
}

/// A picture higher than it is wide.
fn portrait(t: &TextureState) -> bool {
    shape(t).is_some_and(|(w, h)| h > w)
}

/// Press, move in steps and release, as a user drags.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=6 {
        h.hover_at(from + (to - from) * (k as f32 / 6.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(4);
}

/// Where a file's tile is.
fn card(h: &Harness<'static, PdfCraftApp>, name: &str) -> egui::Rect {
    h.get_by_label(name).rect()
}

fn disabled(h: &Harness<'static, PdfCraftApp>, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().is_disabled()
}

fn texts_of(app: &PdfCraftApp, tab: usize) -> Vec<String> {
    let doc = app.session.get(app.views[tab].id).unwrap();
    let mut r = PageRenderer::new(doc.bytes.clone(), Default::default());
    (0..r.page_count())
        .map(|p| {
            let out = r.render(RenderRequest { page: p, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
            out.text.map(|t| t.plain_text().trim().to_string()).unwrap_or_default()
        })
        .collect()
}

#[test]
fn combine_shows_a_grid_by_default_and_the_view_is_kept_in_the_settings() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 2)]);
    assert_eq!(h.state().combine_view, CombineView::Grid);
    h.get_by_label_contains("Files are combined in the order shown");
    assert!(h.query_by_label("File name").is_none(), "no table headings in the grid");
    // The totals stay on screen below the grid.
    let totals = h.get_by_label_contains("2 files · 3 pages").rect();
    assert!(totals.bottom() <= 900.0, "the totals line is visible: {totals:?}");
    h.get_by_label("List view").click();
    h.run_steps(2);
    assert_eq!(h.state().combine_view, CombineView::List);
    h.get_by_label("File name");
    // Kept in the settings, and read back.
    let saved = h.state().settings_snapshot();
    let mut fresh = crate::control::new_app();
    assert_eq!(fresh.combine_view, CombineView::Grid);
    fresh.apply_settings(&saved);
    assert_eq!(fresh.combine_view, CombineView::List);
    // A damaged setting keeps the default.
    let mut damaged = crate::control::new_app();
    damaged.apply_settings(&import_json(r#"{"combine_view": "tiles"}"#).0);
    assert_eq!(damaged.combine_view, CombineView::Grid);
    // Scriptable, with a clear refusal for anything else.
    let app = h.state_mut();
    app.set_option(&egui::Context::default(), "combine-view", "grid").unwrap();
    assert_eq!(app.combine_view, CombineView::Grid);
    assert_eq!(app.set_option(&egui::Context::default(), "combine-view", "tiles"), Err("combine-view must be grid or list".into()));
}

#[test]
fn dragging_a_card_moves_it_and_undo_puts_it_back() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1), ("d.pdf", 1)]);
    // Sorted first: a move by hand ends the sort, as in the list.
    h.state_mut().combine_tab.sort = Some((pdfcraft_ui_egui::SortKey::Name, true));
    let (a, c) = (card(&h, "a.pdf"), card(&h, "c.pdf"));
    drag(&mut h, a.center(), c.center() + vec2(c.width() * 0.3, 0.0));
    assert_eq!(names(&h), ["b.pdf", "c.pdf", "a.pdf", "d.pdf"]);
    assert_eq!(h.state().combine_selection(), [2], "the moved card is selected");
    assert_eq!(h.state().combine_tab.sort, None);
    // The list shows the same order.
    h.get_by_label("List view").click();
    h.run_steps(2);
    let rows: Vec<f32> = ["b.pdf", "c.pdf", "a.pdf", "d.pdf"]
        .iter()
        .map(|name| h.get_by(|n| n.role() == Role::ListItem && n.label().as_deref() == Some(*name)).rect().top())
        .collect();
    assert!(rows.windows(2).all(|w| w[0] < w[1]), "the table lists them in the same order: {rows:?}");
    h.get_by_label("Grid view").click();
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(names(&h), ["a.pdf", "b.pdf", "c.pdf", "d.pdf"]);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(2);
    assert_eq!(names(&h), ["b.pdf", "c.pdf", "a.pdf", "d.pdf"]);
}

#[test]
fn thumbnails_show_the_first_page_each_file_takes() {
    // Page 3 is the only landscape page, so the shape of a thumbnail says which page it shows.
    let pages = fixture_pages(&[(200.0, 300.0), (200.0, 300.0), (300.0, 200.0)]);
    let mut h = grid_of(vec![("three.pdf", pages), ("one.pdf", fixture(1))], vec2(1400.0, 900.0));
    h.state_mut().combine_draft[0].range = "3, 1".into();
    h.run_steps(2);
    let ctx = h.ctx.clone();
    let stacked = request(&h, "three.pdf", true);
    let single = request(&h, "one.pdf", false);
    assert!(
        settle(&mut h, |app| {
            let first = app.combine_thumbnails(&ctx, &stacked);
            let second = app.combine_thumbnails(&ctx, &single);
            landscape(&first[0]) && ready(&second[1])
        }),
        "{:?}",
        thumbnails(&h, &stacked)
    );
    let thumbs = thumbnails(&h, &stacked);
    assert!(landscape(&thumbs[0]), "\"3, 1\" starts with page 3: {:?}", thumbs[0]);
    let other = thumbnails(&h, &single);
    assert!(portrait(&other[1]), "{:?}", other[1]);
    let (width, height) = shape(&other[1]).unwrap();
    // Within the thumbnails' longest side, which is not visible from here.
    assert!(width > 0.0 && height > 0.0 && width <= 1536.0 && height <= 1536.0, "{width}×{height}");
    // A new range is a new thumbnail, never the old page.
    h.state_mut().combine_draft[0].range = "2".into();
    h.run_steps(2);
    let single_first = request(&h, "three.pdf", false);
    assert!(settle(&mut h, |app| portrait(&app.combine_thumbnails(&ctx, &single_first)[0])), "{:?}", thumbnails(&h, &single_first));
    // A range that can't be used shows no page at all.
    h.state_mut().combine_draft[0].range = "9".into();
    h.run_steps(2);
    assert_eq!(thumbnails(&h, &single_first)[0], TextureState::Missing);
}

#[test]
fn a_locked_file_shows_no_page_until_unlocked_and_undo_locks_it_again() {
    let mut h = grid_of(vec![("one.pdf", fixture(1)), ("secret.pdf", protected("pw", "owner"))], vec2(1400.0, 900.0));
    let ctx = h.ctx.clone();
    let single = request(&h, "one.pdf", false);
    let stacked = request(&h, "one.pdf", true);
    assert!(settle(&mut h, |app| ready(&app.combine_thumbnails(&ctx, &single)[0])));
    assert_eq!(thumbnails(&h, &stacked)[1], TextureState::Missing, "nothing is drawn of a locked file");
    assert!(h.state_mut().combine_unlock_rows(&[1], "pw"));
    assert!(settle(&mut h, |app| ready(&app.combine_thumbnails(&ctx, &stacked)[1])), "{:?}", thumbnails(&h, &stacked));
    // Undo brings back the locked file: its unlocked thumbnail goes with the password.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert!(h.state().combine_draft[1].lock.is_some());
    assert_eq!(thumbnails(&h, &stacked)[1], TextureState::Missing);
    // Unlocked again (a new branch of the history): drawn again, from this unlock.
    assert!(h.state_mut().combine_unlock_rows(&[1], "pw"));
    assert!(settle(&mut h, |app| ready(&app.combine_thumbnails(&ctx, &stacked)[1])), "{:?}", thumbnails(&h, &stacked));
}

#[test]
fn combine_follows_the_order_shown_in_the_grid() {
    let mut h = grid_of_pages(&[("one.pdf", 2), ("two.pdf", 1), ("three.pdf", 1)]);
    let (three, one) = (card(&h, "three.pdf"), card(&h, "one.pdf"));
    drag(&mut h, three.center(), one.center() - vec2(one.width() * 0.3, 0.0));
    assert_eq!(names(&h), ["three.pdf", "one.pdf", "two.pdf"]);
    h.get_by_role_and_label(Role::Button, "Combine").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.views.len(), 1, "the result opens");
    assert_eq!(texts_of(app, 0), ["Page 1", "Page 1", "Page 2", "Page 1"]);
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.info.outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), ["three", "one", "two"]);
}

#[test]
fn a_bad_page_range_on_a_card_says_where_to_fix_it() {
    let mut h = grid_of_pages(&[("a.pdf", 3), ("b.pdf", 1)]);
    h.state_mut().combine_draft[0].range = "9".into();
    h.run_steps(2);
    h.get_by_label_contains("Switch to List view to change the pages it takes");
}

/// The pointer over a tile (its centre), which shows the tile's own actions.
fn hover_card(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    let at = card(h, name).center();
    h.hover_at(at);
    h.run_steps(2);
}

/// Click an action inside the named tile, even when other tiles expose the same action.
fn tile_action(h: &mut Harness<'static, PdfCraftApp>, name: &str, action: &str) {
    hover_card(h, name);
    let rect = card(h, name);
    h.query_all_by_label(action).find(|n| rect.contains(n.rect().center())).expect("the tile offers the action").click();
    h.run_steps(2);
}

/// A page tile's label: "Page 3 · b.pdf".
fn page_card(page: usize, name: &str) -> String {
    format!("Page {page} · {name}")
}

/// Expand a file from its tile's own action.
fn expand(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    tile_action(h, name, "Expand");
}

/// Trash removes only its own file, whatever else is selected, in one undo step.
fn row_remove_acts_on_its_own_file() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1), ("d.pdf", 1)]);
    h.get_by_label("b.pdf").click();
    h.run_steps(1);
    command_click(&mut h, "c.pdf");
    h.run_steps(1);
    // The trash of a tile that isn't selected: that file only, the selection stays.
    tile_action(&mut h, "a.pdf", "Remove");
    assert_eq!(names(&h), ["b.pdf", "c.pdf", "d.pdf"]);
    assert_eq!(h.state().combine_selection(), [0, 1], "b and c stay selected");
    // The trash of a selected tile: still that file only.
    tile_action(&mut h, "b.pdf", "Remove");
    assert_eq!(names(&h), ["c.pdf", "d.pdf"]);
    assert_eq!(h.state().combine_selection(), [0], "c stays selected");
    // One undo step each.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(names(&h), ["b.pdf", "c.pdf", "d.pdf"]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(names(&h), ["a.pdf", "b.pdf", "c.pdf", "d.pdf"]);
}

/// Preview opens the tile's own file, from its first page; the grid's selection stays.
fn row_preview_opens_that_file() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 3)]);
    h.get_by_label("a.pdf").click();
    h.run_steps(1);
    tile_action(&mut h, "b.pdf", "Preview");
    h.run_steps(2);
    assert_eq!(h.state().combine_preview(), Some(("b.pdf".to_string(), 0)));
    assert_eq!(h.state().combine_selection(), [0]);
}

/// A page tile's Preview opens that page of its file.
fn row_page_preview_opens_that_page() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 5)]);
    // Pages 3, 1, 4: page 4 is the third the file adds.
    h.state_mut().combine_draft[1].range = "3, 1, 4".into();
    h.run_steps(2);
    expand(&mut h, "b.pdf");
    tile_action(&mut h, &page_card(4, "b.pdf"), "Preview");
    h.run_steps(2);
    assert_eq!(h.state().combine_preview(), Some(("b.pdf".to_string(), 3)));
}

/// Unlock… is on a locked file's menu only.
fn row_unlock_is_on_a_locked_files_menu_only() {
    let mut h = grid_of(vec![("one.pdf", fixture(1)), ("secret.pdf", protected("pw", "owner"))], vec2(1400.0, 900.0));
    // (The toolbar may have an Unlock… of its own: the menu's item is one more.)
    let toolbar = h.query_all_by_label("Unlock…").count();
    h.get_by_label("one.pdf").click_secondary();
    h.run_steps(2);
    assert_eq!(h.query_all_by_label("Unlock…").count(), toolbar, "an open file's menu adds no Unlock…");
    h.key_press(Key::Escape);
    h.run_steps(2);
    h.get_by_label("secret.pdf. Password-protected").click_secondary();
    h.run_steps(2);
    assert_eq!(h.query_all_by_label("Unlock…").count(), toolbar + 1, "a locked file's menu offers it");
    // The menu's item comes after the toolbar's button of the same name.
    h.get_all_by_label("Unlock…").last().unwrap().click();
    h.run_steps(2);
    for _ in 0..30 {
        if h.query_by_role_and_label(Role::Dialog, "Password required").is_some() {
            break;
        }
        h.run_steps(1);
    }
    h.get_by_role_and_label(Role::Dialog, "Password required").get_by_label_contains("secret.pdf");
}

/// Expand and Collapse on a tile, and from the toolbar for every file.
fn row_expand_and_collapse() {
    let mut h = grid_of(
        vec![("a.pdf", fixture(1)), ("b.pdf", fixture(3)), ("c.pdf", fixture(2)), ("secret.pdf", protected("pw", "owner"))],
        vec2(1400.0, 900.0),
    );
    assert!(disabled(&h, "Collapse all"), "nothing to collapse yet");
    h.get_by_label("Expand all").click();
    h.run_steps(2);
    assert_eq!(h.state().combine_expanded(), [1, 2], "every file adding more than one page; not a single page, not a locked file");
    assert!(disabled(&h, "Expand all"));
    h.get_by_label("Collapse all").click();
    h.run_steps(2);
    assert!(h.state().combine_expanded().is_empty());
    expand(&mut h, "c.pdf");
    assert_eq!(h.state().combine_expanded(), [2]);
    tile_action(&mut h, &page_card(2, "c.pdf"), "Collapse");
    h.run_steps(2);
    assert!(h.state().combine_expanded().is_empty());
}

/// What each tile action does to its own tile, one row each.
const TILE_ACTIONS: [(&str, fn()); 5] = [
    ("Remove acts on its own file", row_remove_acts_on_its_own_file),
    ("Preview opens that file", row_preview_opens_that_file),
    ("a page's Preview opens that page", row_page_preview_opens_that_page),
    ("Unlock is on a locked file's menu only", row_unlock_is_on_a_locked_files_menu_only),
    ("Expand and Collapse", row_expand_and_collapse),
];

#[test]
fn tile_actions_act_on_their_own_tile() {
    for (name, row) in TILE_ACTIONS {
        println!("tile action: {name}");
        row();
    }
}

#[test]
fn the_preview_shows_the_files_range_in_order_and_the_grid_selection_stays() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 4)]);
    h.state_mut().combine_draft[1].range = "3, 1".into();
    h.get_by_label("a.pdf").click();
    h.run_steps(1);
    tile_action(&mut h, "b.pdf", "Preview");
    h.run_steps(2);
    // The first page it adds: page 3.
    assert_eq!(h.state().combine_preview(), Some(("b.pdf".to_string(), 2)));
    // The page keys flip in the range's order, and stop at the ends; the grid's selection doesn't move.
    h.key_press(Key::PageDown);
    h.run_steps(2);
    assert_eq!(h.state().combine_preview().map(|p| p.1), Some(0), "then page 1");
    h.key_press(Key::PageDown);
    h.run_steps(2);
    assert_eq!(h.state().combine_preview().map(|p| p.1), Some(0), "the last stays");
    h.key_press(Key::PageUp);
    h.run_steps(2);
    assert_eq!(h.state().combine_preview().map(|p| p.1), Some(2));
    assert_eq!(h.state().combine_selection(), [0]);
    // Escape closes it, and nothing else changed.
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().combine_preview().is_none());
    assert_eq!(names(&h), ["a.pdf", "b.pdf"]);
}

#[test]
fn the_preview_closes_when_its_file_leaves() {
    let mut h = grid_of_pages(&[("f00.pdf", 2), ("f01.pdf", 2), ("f02.pdf", 2)]);
    tile_action(&mut h, "f01.pdf", "Preview");
    h.run_steps(2);
    assert!(h.state().combine_preview().is_some());
    // The file leaves the list (as undo of its adding would): the preview closes.
    h.state_mut().combine_draft.retain(|f| f.name != "f01.pdf");
    h.run_steps(2);
    assert!(h.state().combine_preview().is_none());
}

#[test]
fn leaving_the_tab_closes_the_preview_and_coming_back_does_not_reopen_it() {
    let mut h = grid_of_pages(&[("a.pdf", 6)]);
    tile_action(&mut h, "a.pdf", "Preview");
    h.run_steps(2);
    assert!(h.state().combine_preview().is_some());
    // Home, then back: the preview is gone, not waiting there.
    h.state_mut().combine_tab.focused = false;
    h.run_steps(2);
    assert!(h.state().combine_preview().is_none());
    h.state_mut().open_combine_tab();
    h.run_steps(2);
    assert!(h.state().combine_preview().is_none());
    assert!(h.query_by_label("Page 1 of 6").is_none());
}

#[test]
fn the_toolbar_sizes_the_cards_within_limits_and_the_size_is_kept_in_the_settings() {
    let ladder = TileZoom::Ladder.range();
    let (smallest, largest) = (*ladder.start(), *ladder.end());
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 2)]);
    assert_eq!(h.state().combine_zoom, 1.0);
    let before = card(&h, "a.pdf");
    h.get_by_label("Larger Thumbnails").click();
    h.run_steps(3);
    assert!(h.state().combine_zoom > 1.0, "{}", h.state().combine_zoom);
    let after = card(&h, "a.pdf");
    assert!(after.width() > before.width() && after.height() > before.height(), "{before:?} → {after:?}");
    // Larger until it can't go further, which is the end of the ladder (#2531).
    for _ in 0..12 {
        h.get_by_label("Larger Thumbnails").click();
        h.run_steps(2);
    }
    assert_eq!(h.state().combine_zoom, largest);
    for _ in 0..14 {
        h.get_by_label("Smaller Thumbnails").click();
        h.run_steps(2);
    }
    assert_eq!(h.state().combine_zoom, smallest);
    let smallest_card = card(&h, "a.pdf");
    assert!(smallest_card.width() < before.width() && smallest_card.height() < before.height(), "{smallest_card:?}");
    // Kept in the settings, and read back; a damaged setting is clamped or ignored.
    let saved = h.state().settings_snapshot();
    let mut fresh = crate::control::new_app();
    fresh.apply_settings(&saved);
    assert_eq!(fresh.combine_zoom, smallest);
    fresh.apply_settings(&import_json(r#"{"combine_zoom": 99}"#).0);
    assert_eq!(fresh.combine_zoom, largest);
    fresh.apply_settings(&import_json(r#"{"combine_zoom": "big"}"#).0);
    assert_eq!(fresh.combine_zoom, largest);
    // Scriptable in percent, with a clear refusal outside the range.
    let app = h.state_mut();
    app.set_option(&egui::Context::default(), "combine-zoom", "150%").unwrap();
    assert_eq!(app.combine_zoom, 1.5);
    // #2531: 200% was not enough to read a value off a card; 500% has to be reachable.
    for pct in ["200", "300", "400", "500"] {
        app.set_option(&egui::Context::default(), "combine-zoom", pct).unwrap();
        assert_eq!(app.combine_zoom, pct.parse::<f64>().unwrap() / 100.0);
    }
    for bad in ["59", "501", "big", "NaN", "inf", ""] {
        assert!(app.set_option(&egui::Context::default(), "combine-zoom", bad).is_err(), "{bad:?}");
    }
    assert_eq!(app.combine_zoom, largest, "a refused value leaves the size alone");
    // The list has no card size.
    h.get_by_label("List view").click();
    h.run_steps(2);
    assert!(h.query_by_label("Larger Thumbnails").is_none());
}

#[test]
fn expanding_a_file_shows_the_pages_it_adds_in_order_with_their_numbers() {
    // Page 3 is landscape, page 4 square, the others portrait: the shape of a thumbnail says its page.
    let pages = fixture_pages(&[(200.0, 300.0), (200.0, 300.0), (300.0, 200.0), (250.0, 250.0)]);
    let mut h = grid_of(vec![("a.pdf", fixture(1)), ("b.pdf", pages)], vec2(1400.0, 900.0));
    h.state_mut().combine_draft[1].range = "3, 1, 4".into();
    h.run_steps(2);
    // A single page has nothing to expand.
    hover_card(&mut h, "a.pdf");
    assert!(!h.query_all_by_label("Expand").any(|n| card(&h, "a.pdf").contains(n.rect().center())));
    expand(&mut h, "b.pdf");
    assert_eq!(h.state().combine_expanded(), [1]);
    assert!(h.query_by_role_and_label(Role::ListItem, "b.pdf").is_none(), "the file's tile gives way to its pages");
    // The pages it adds, in the range's order, each saying which page of which file.
    let xs: Vec<f32> = [3, 1, 4].iter().map(|p| card(&h, &page_card(*p, "b.pdf")).left()).collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "pages 3, 1, 4 in that order: {xs:?}");
    assert!(h.query_by_label(&page_card(2, "b.pdf")).is_none(), "page 2 isn't added, so isn't shown");
    // Each renders its own page.
    let ctx = h.ctx.clone();
    let page_request = request(&h, &page_card(3, "b.pdf"), false);
    assert!(settle(&mut h, |app| {
        let pages = app.combine_page_thumbnails(&ctx, 1, &page_request);
        pages.len() == 3 && pages.iter().all(|(_, t)| ready(t))
    }));
    let shapes: Vec<(usize, TextureState)> = h.state().combine_page_thumbnails(&ctx, 1, &page_request);
    assert_eq!(shapes.iter().map(|(p, _)| *p).collect::<Vec<_>>(), [2, 0, 3]);
    assert!(landscape(&shapes[0].1) && portrait(&shapes[1].1), "{shapes:?}");
    let (w, h4) = shape(&shapes[2].1).unwrap();
    assert!((w - h4).abs() < 2.0, "page 4 is square: {w}×{h4}");
    // A page tile collapses the file again.
    tile_action(&mut h, &page_card(1, "b.pdf"), "Collapse");
    h.run_steps(2);
    assert!(h.state().combine_expanded().is_empty());
    h.get_by_label("b.pdf");
}

#[test]
fn expanding_is_a_view_kept_across_views_and_zoom_and_never_an_undo_step() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 3)]);
    let undo = disabled(&h, "Undo");
    expand(&mut h, "b.pdf");
    assert_eq!(disabled(&h, "Undo"), undo, "not an edit: the undo history is as it was");
    assert!(disabled(&h, "Redo"));
    h.get_by_label("List view").click();
    h.run_steps(2);
    h.get_by_label("Grid view").click();
    h.run_steps(2);
    h.get_by_label("Larger Thumbnails").click();
    h.run_steps(2);
    assert_eq!(h.state().combine_expanded(), [1], "still shown as its pages");
    h.get_by_label(&page_card(3, "b.pdf"));
    // Its range edited down to one page: that page's tile (a part of a file can be one page).
    h.state_mut().combine_draft[1].range = "2".into();
    h.run_steps(2);
    h.get_by_label(&page_card(2, "b.pdf"));
    // A bad range: its tile again, no longer expanded, and it doesn't come back expanded by itself
    // when the range is fixed.
    h.state_mut().combine_draft[1].range = "9".into();
    h.run_steps(2);
    assert!(h.state().combine_expanded().is_empty(), "no longer expanded");
    assert!(disabled(&h, "Collapse all"));
    h.state_mut().combine_draft[1].range = String::new();
    h.run_steps(2);
    h.get_by_label("b.pdf");
    assert!(h.state().combine_expanded().is_empty());
    expand(&mut h, "b.pdf");
    h.get_by_label(&page_card(2, "b.pdf"));
    // The file removed (its file selected with the keys, then Delete): forgotten; undo brings the file
    // back as its tile.
    h.get_by_label("a.pdf").click();
    h.run_steps(1);
    h.key_press(Key::ArrowDown);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [1]);
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(names(&h), ["a.pdf"]);
    assert!(h.state().combine_expanded().is_empty());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(names(&h), ["a.pdf", "b.pdf"]);
    h.get_by_label("b.pdf");
}

#[test]
fn dragging_a_page_moves_that_page_even_between_another_files_pages() {
    let mut h = grid_of(vec![("a.pdf", fixture(1)), ("b.pdf", fixture(4)), ("c.pdf", fixture(1))], vec2(2600.0, 1600.0));
    expand(&mut h, "b.pdf");
    // b's page 2 dragged after c: that page alone moves; b is now in two parts.
    let (p2, c) = (card(&h, &page_card(2, "b.pdf")), card(&h, "c.pdf"));
    drag(&mut h, p2.center(), c.center() + vec2(c.width() * 0.3, 0.0));
    let parts =
        |h: &Harness<'static, PdfCraftApp>| h.state().combine_parts_shown().into_iter().map(|(n, p)| format!("{n}:{p:?}")).collect::<Vec<_>>();
    assert_eq!(parts(&h), ["a.pdf:[0]", "b.pdf:[0, 2, 3]", "c.pdf:[0]", "b.pdf:[1]"]);
    // One undo step puts it back, as one file again.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(parts(&h), ["a.pdf:[0]", "b.pdf:[0, 1, 2, 3]", "c.pdf:[0]"]);
    // Within its file: only its order changes (page 4 to the front).
    let (p4, p1) = (card(&h, &page_card(4, "b.pdf")), card(&h, &page_card(1, "b.pdf")));
    drag(&mut h, p4.center(), p1.center() - vec2(p1.width() * 0.3, 0.0));
    assert_eq!(parts(&h), ["a.pdf:[0]", "b.pdf:[3, 0, 1, 2]", "c.pdf:[0]"]);
    // Selected pages go together, in their order: pages 1 and 3 (now 2nd and 4th) before a.
    h.get_by_label(&page_card(1, "b.pdf")).click();
    h.run_steps(1);
    command_click(&mut h, &page_card(3, "b.pdf"));
    h.run_steps(2);
    let (p3, a) = (card(&h, &page_card(3, "b.pdf")), card(&h, "a.pdf"));
    drag(&mut h, p3.center(), a.center() - vec2(a.width() * 0.3, 0.0));
    assert_eq!(parts(&h), ["b.pdf:[0, 2]", "a.pdf:[0]", "b.pdf:[3, 1]", "c.pdf:[0]"]);
    // A file's tile still moves its whole file, and a drop among a file's pages goes before that file.
    let (cc, early) = (card(&h, "c.pdf"), card(&h, &page_card(3, "b.pdf")));
    drag(&mut h, cc.center(), early.center() - vec2(early.width() * 0.3, 0.0));
    assert_eq!(names(&h), ["c.pdf", "b.pdf", "a.pdf", "b.pdf"], "before the file whose page it was dropped on");
}

#[test]
fn combining_with_files_expanded_gives_the_same_document() {
    let mut h = grid_of_pages(&[("one.pdf", 2), ("two.pdf", 3)]);
    h.state_mut().combine_draft[1].range = "3, 1".into();
    h.get_by_label("Expand all").click();
    h.run_steps(2);
    assert_eq!(h.state().combine_expanded(), [0, 1]);
    h.get_by_role_and_label(Role::Button, "Combine").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.views.len(), 1, "the result opens");
    assert_eq!(texts_of(app, 0), ["Page 1", "Page 2", "Page 3", "Page 1"]);
}

#[test]
fn removing_pages_from_the_tile_the_toolbar_and_delete_never_touches_the_file() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 4)]);
    let original = h.state().combine_draft[1].bytes.clone();
    expand(&mut h, "b.pdf");
    let parts =
        |h: &Harness<'static, PdfCraftApp>| h.state().combine_parts_shown().into_iter().map(|(n, p)| format!("{n}:{p:?}")).collect::<Vec<_>>();
    // A page tile's trash: that page.
    tile_action(&mut h, &page_card(2, "b.pdf"), "Remove");
    assert_eq!(parts(&h), ["a.pdf:[0]", "b.pdf:[0, 2, 3]"]);
    // Two selected: the toolbar's trash says so, and takes both.
    h.get_by_label(&page_card(1, "b.pdf")).click();
    h.run_steps(1);
    command_click(&mut h, &page_card(4, "b.pdf"));
    h.run_steps(2);
    h.hover_at(egui::pos2(5.0, 890.0));
    h.run_steps(1);
    h.get_by_label("Remove 2 pages").click();
    h.run_steps(2);
    assert_eq!(parts(&h), ["a.pdf:[0]", "b.pdf:[2]"]);
    // Delete on the last page left: the file leaves the list.
    h.get_by_label(&page_card(3, "b.pdf")).click();
    h.run_steps(2);
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(parts(&h), ["a.pdf:[0]"]);
    // Each was one undo step; the file itself was never changed.
    for expected in [vec!["a.pdf:[0]", "b.pdf:[2]"], vec!["a.pdf:[0]", "b.pdf:[0, 2, 3]"], vec!["a.pdf:[0]", "b.pdf:[0, 1, 2, 3]"]] {
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run_steps(2);
        assert_eq!(parts(&h), expected);
    }
    assert!(std::sync::Arc::ptr_eq(&h.state().combine_draft[1].bytes, &original), "the same, untouched bytes");
}

#[test]
fn a_split_file_counts_once_and_combines_with_one_bookmark() {
    let mut h = grid_of_pages(&[("one.pdf", 2), ("two.pdf", 3)]);
    expand(&mut h, "one.pdf");
    expand(&mut h, "two.pdf");
    let totals = |h: &Harness<'static, PdfCraftApp>| {
        let n = h.get_by_label_contains(" pages · ");
        let a = n.accesskit_node();
        a.label().or(a.value()).unwrap_or_default().to_string()
    };
    let before = totals(&h);
    assert!(before.starts_with("2 files · 5 pages · "), "{before}");
    // one's page 2 between two's pages 1 and 2.
    let (p2, t2) = (card(&h, &page_card(2, "one.pdf")), card(&h, &page_card(2, "two.pdf")));
    drag(&mut h, p2.center(), t2.center() - vec2(t2.width() * 0.3, 0.0));
    assert_eq!(names(&h), ["one.pdf", "two.pdf", "one.pdf", "two.pdf"]);
    // Still two files, of the same size (each counted once), and the parts aren't "added more than
    // once".
    assert_eq!(totals(&h), before);
    assert!(h.query_by_label_contains("Added more than once").is_none());
    h.hover_at(egui::pos2(5.0, 890.0));
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, "Combine").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.views.len(), 1, "the result opens");
    assert_eq!(texts_of(app, 0), ["Page 1", "Page 1", "Page 2", "Page 2", "Page 3"], "the order shown");
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.info.outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), ["one", "two"], "one bookmark per file");
}

#[test]
fn pages_picked_out_of_sight_or_before_an_undo_are_let_go_of() {
    // Regression: a page picked, then hidden (its file collapsed, or the list shown) or undone,
    // stayed picked, and the trash or Delete took it.
    let mut h = grid_of(vec![("a.pdf", fixture(1)), ("b.pdf", fixture(4)), ("c.pdf", fixture(1))], vec2(2000.0, 1200.0));
    let parts =
        |h: &Harness<'static, PdfCraftApp>| h.state().combine_parts_shown().into_iter().map(|(n, p)| format!("{n}:{p:?}")).collect::<Vec<_>>();
    expand(&mut h, "b.pdf");
    // Collapsed from a page tile's action: nothing picked, Delete takes nothing.
    h.get_by_label(&page_card(2, "b.pdf")).click();
    h.run_steps(2);
    tile_action(&mut h, &page_card(3, "b.pdf"), "Collapse");
    h.run_steps(2);
    assert!(h.state().combine_selected_pages().is_empty());
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(parts(&h), ["a.pdf:[0]", "b.pdf:[0, 1, 2, 3]", "c.pdf:[0]"]);
    // In the list: the same.
    expand(&mut h, "b.pdf");
    h.get_by_label(&page_card(2, "b.pdf")).click();
    h.run_steps(2);
    h.get_by_label("List view").click();
    h.run_steps(2);
    assert!(h.state().combine_selected_pages().is_empty());
    h.get_by_label("Grid view").click();
    h.run_steps(2);
    // After an undo: let go of, and a Delete then changes nothing and keeps the redo.
    let (p2, c) = (card(&h, &page_card(2, "b.pdf")), card(&h, "c.pdf"));
    drag(&mut h, p2.center(), c.center() + vec2(c.width() * 0.3, 0.0));
    assert_eq!(parts(&h), ["a.pdf:[0]", "b.pdf:[0, 2, 3]", "c.pdf:[0]", "b.pdf:[1]"]);
    // Page 3, the second of b's first part: after the undo that place is page 2, another page.
    h.get_by_label(&page_card(3, "b.pdf")).click();
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert!(h.state().combine_selected_pages().is_empty());
    assert!(!disabled(&h, "Redo"));
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(parts(&h), ["a.pdf:[0]", "b.pdf:[0, 1, 2, 3]", "c.pdf:[0]"]);
    assert!(!disabled(&h, "Redo"), "a Delete with nothing picked makes no undo step");
    // A file picked (by dragging its tile) lets go of the pages: Delete takes the file.
    h.get_by_label(&page_card(2, "b.pdf")).click();
    h.run_steps(2);
    let (cc, a) = (card(&h, "c.pdf"), card(&h, "a.pdf"));
    drag(&mut h, cc.center(), a.center() - vec2(a.width() * 0.3, 0.0));
    assert_eq!(h.state().combine_selection(), [0]);
    assert!(h.state().combine_selected_pages().is_empty());
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(parts(&h), ["a.pdf:[0]", "b.pdf:[0, 1, 2, 3]"]);
}

#[test]
fn a_files_parts_join_when_what_was_between_them_goes_and_one_file_cannot_be_combined() {
    let mut h = grid_of_pages(&[("one.pdf", 2), ("two.pdf", 1)]);
    let parts =
        |h: &Harness<'static, PdfCraftApp>| h.state().combine_parts_shown().into_iter().map(|(n, p)| format!("{n}:{p:?}")).collect::<Vec<_>>();
    expand(&mut h, "one.pdf");
    // one's page 2 after two: one is in two parts.
    let (p2, two) = (card(&h, &page_card(2, "one.pdf")), card(&h, "two.pdf"));
    drag(&mut h, p2.center(), two.center() + vec2(two.width() * 0.3, 0.0));
    assert_eq!(parts(&h), ["one.pdf:[0]", "two.pdf:[0]", "one.pdf:[1]"]);
    // two goes: one's parts meet and are one entry again, in the same undo step.
    tile_action(&mut h, "two.pdf", "Remove");
    assert_eq!(parts(&h), ["one.pdf:[0, 1]"]);
    // One file is nothing to combine, however it is split.
    assert!(h.get_by_role_and_label(Role::Button, "Combine").accesskit_node().is_disabled());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(parts(&h), ["one.pdf:[0]", "two.pdf:[0]", "one.pdf:[1]"]);
}

#[test]
fn files_picked_by_automation_let_go_of_the_pages_picked() {
    // Regression (found by review): selecting rows through the API left a page picked, and
    // Delete took that page instead of the file.
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 3)]);
    expand(&mut h, "b.pdf");
    h.get_by_label(&page_card(2, "b.pdf")).click();
    h.run_steps(2);
    assert_eq!(h.state().combine_selected_pages(), [(1, 1)]);
    h.state_mut().select_combine_rows(&[0]);
    h.run_steps(2);
    assert!(h.state().combine_selected_pages().is_empty());
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(h.state().combine_parts_shown(), [("b.pdf".to_string(), vec![0, 1, 2])], "the file picked goes, b keeps its pages");
}

#[test]
fn open_documents_already_in_the_list_are_not_added_again() {
    // #1356: Add open documents left out none of the open documents, but added again the ones
    // already listed. Two documents with the same name and the same bytes are still two documents.
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            for (name, pages) in [("a.pdf", 1), ("b.pdf", 2), ("report.pdf", 3)] {
                app.open_bytes(&egui::Context::default(), name, None, fixture(pages)).unwrap();
            }
            app
        }
    });
    h.run_steps(4);
    h.execute("page.combine");
    h.run_steps(3);
    let add_all = |h: &mut Harness<'static, PdfCraftApp>| {
        h.get_by_label("Add open documents").click();
        h.run_steps(2);
        h.get_by_label("All open documents").click();
        h.run_steps(3);
    };
    add_all(&mut h);
    assert_eq!(names(&h), ["a.pdf", "b.pdf", "report.pdf"]);
    add_all(&mut h);
    assert_eq!(names(&h), ["a.pdf", "b.pdf", "report.pdf"], "nothing is added twice");
    // Another report.pdf, opened while the first is listed: a different document all the same.
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "report.pdf", None, fixture(3)).unwrap();
    h.execute("page.combine");
    h.run_steps(3);
    add_all(&mut h);
    assert_eq!(names(&h), ["a.pdf", "b.pdf", "report.pdf", "report.pdf"]);
    // A document taken out of the list can be added again.
    h.state_mut().select_combine_rows(&[1]);
    h.run_steps(2);
    h.key_press(Key::Delete);
    h.run_steps(2);
    add_all(&mut h);
    assert_eq!(names(&h), ["a.pdf", "report.pdf", "report.pdf", "b.pdf"]);
}

/// Passes egui ran in the frames a harness has run so far.
fn passes(h: &Harness<'static, PdfCraftApp>) -> u64 {
    h.ctx.cumulative_pass_nr()
}

#[test]
/// A settled Combine grid runs one pass a frame: nothing in it asks egui for a discard every frame (egui's PERF WARNING
/// "request_discard has been called N frames in a row").
fn a_settled_grid_asks_for_no_discard_pass() {
    for expanded in [false, true] {
        let mut h = grid_of(vec![("a.pdf", fixture(1)), ("b.pdf", fixture(3)), ("c.pdf", fixture(2))], vec2(1400.0, 900.0));
        if expanded {
            h.get_by_label("Expand all").click();
        }
        h.run_steps(30);
        let (before, frames) = (passes(&h), 20u64);
        h.run_steps(frames as usize);
        assert_eq!(passes(&h) - before, frames, "expanded={expanded}: a settled frame runs one pass");
    }
}
