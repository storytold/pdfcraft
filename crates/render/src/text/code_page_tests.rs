//! Copy with formatting's code-page rule (hayro-interpret patch 105, `CodePageByOutline`): a
//! simple font that reads as WinAnsi but whose glyphs draw the letters of Windows-1250 or
//! Windows-1257. The fonts here are synthetic TrueType programs whose glyphs are polygons: a
//! base letter and the accent the test needs (a caron, a double acute, a macron …).

use super::*;
use hayro::hayro_interpret::font::{RecoveryCounters, recovery_counters};
use pdfcraft_testkit::truetype::{Cmap, mac_gid, with_outlines};

type Contour = &'static [(i16, i16)];

/// A lowercase letter's body, from the baseline to 450.
const BODY: Contour = &[(50, 0), (450, 0), (450, 450), (50, 450)];
/// A capital's body, to 700.
const CAPITAL: Contour = &[(50, 0), (550, 0), (550, 700), (50, 700)];
const CARON: Contour = &[(100, 650), (170, 650), (250, 560), (330, 650), (400, 650), (250, 520)];
/// A caron above a capital.
const CAPITAL_CARON: Contour = &[(100, 850), (170, 850), (250, 760), (330, 850), (400, 850), (250, 720)];
const CIRCUMFLEX: Contour = &[(250, 650), (400, 520), (330, 520), (250, 600), (170, 520), (100, 520)];
const GRAVE: Contour = &[(130, 650), (190, 650), (290, 520), (230, 520)];
const ACUTE: Contour = &[(230, 520), (290, 520), (390, 650), (330, 650)];
const ACUTE_LEFT: Contour = &[(130, 520), (180, 520), (260, 650), (210, 650)];
const ACUTE_RIGHT: Contour = &[(260, 520), (310, 520), (390, 650), (340, 650)];
const DOT_LEFT: Contour = &[(120, 600), (200, 600), (200, 520), (120, 520)];
const DOT_RIGHT: Contour = &[(300, 600), (380, 600), (380, 520), (300, 520)];
const MACRON: Contour = &[(100, 560), (400, 560), (400, 610), (100, 610)];
const BREVE: Contour = &[(100, 640), (150, 640), (250, 575), (350, 640), (400, 640), (250, 540)];
const DOT: Contour = &[(220, 540), (280, 540), (280, 600), (220, 600)];
const RING: Contour = &[(180, 520), (320, 520), (320, 660), (180, 660)];
const RING_HOLE: Contour = &[(220, 560), (280, 560), (280, 620), (220, 620)];
const TILDE: Contour = &[(100, 540), (180, 640), (260, 560), (340, 640), (400, 600), (330, 520), (250, 600), (170, 520)];
const OGONEK: Contour = &[(350, -150), (420, -150), (420, 0), (350, 0)];
const STEM: Contour = &[(200, 0), (260, 0), (260, 700), (200, 700)];
const STROKE: Contour = &[(120, 300), (340, 420), (340, 460), (120, 340)];
const SUPERIOR: Contour = &[(100, 400), (300, 400), (300, 700), (100, 700)];
/// A `t`'s stem, and the apostrophe beside it that `ť` has for a caron.
const T_STEM: Contour = &[(150, 0), (250, 0), (250, 600), (150, 600)];
/// A slash through a letter, from below the baseline.
const SLASH: Contour = &[(20, -120), (80, -120), (480, 520), (420, 520)];
const LOW_APOSTROPHE: Contour = &[(330, 500), (370, 500), (400, 700), (360, 700)];
const APOSTROPHE: Contour = &[(330, 620), (370, 620), (400, 780), (360, 780)];

/// Each accented letter's contours, by the character it draws.
fn drawing(c: char) -> &'static [Contour] {
    match c {
        'č' | 'š' => &[BODY, CARON],
        'Č' | 'Ž' => &[CAPITAL, CAPITAL_CARON],
        'ť' => &[T_STEM, APOSTROPHE],
        // A `ť` whose apostrophe starts below the `t`'s top, as many fonts draw it.
        'Ť' => &[T_STEM, LOW_APOSTROPHE],
        // A bullet, floating above the baseline.
        '•' => &[DOT],
        // A cedilla alone, below the baseline.
        '¸' => &[OGONEK],
        // A spacing double acute, all of it above the baseline.
        '˝' => &[ACUTE_LEFT, ACUTE_RIGHT],
        // An `O` whose slash reaches below the baseline.
        'Ø' | 'ø' => &[BODY, SLASH],
        // Three marks: no letter's.
        '*' => &[BODY, DOT_LEFT, DOT, DOT_RIGHT],
        'ő' | 'ű' => &[BODY, ACUTE_LEFT, ACUTE_RIGHT],
        'ā' | 'ē' => &[BODY, MACRON],
        'ö' | 'ä' => &[BODY, DOT_LEFT, DOT_RIGHT],
        'à' | 'è' => &[BODY, GRAVE],
        'á' | 'é' => &[BODY, ACUTE],
        'â' => &[BODY, CIRCUMFLEX],
        'ğ' => &[BODY, BREVE],
        'ż' => &[BODY, DOT],
        'å' => &[BODY, RING, RING_HOLE],
        'õ' => &[BODY, TILDE],
        'ą' | 'ş' => &[BODY, OGONEK],
        'ł' => &[STEM, STROKE],
        '³' => &[SUPERIOR],
        // A plain letter.
        _ => &[BODY],
    }
}

fn parse(bytes: Vec<u8>) -> Pdf {
    Pdf::new(std::sync::Arc::new(bytes)).unwrap()
}

/// Both layers of page 0, the strict layer as the plain extraction reads it, and what reading
/// both cost the recovery layer.
fn layers(pdf: &Pdf) -> (PageText, Option<RecoveredText>, PageText, RecoveryCounters) {
    let settings = InterpreterSettings::default();
    let before = recovery_counters();
    let (strict, recovered) = extract_page_recovering(pdf, 0, &settings).unwrap();
    let cost = recovery_counters().since(&before);
    (strict, recovered, extract_page(pdf, 0, &settings).unwrap(), cost)
}

/// A font of the test: `glyphs` are the codes it shows (each drawing `drawn`), named by
/// WinAnsi; the program's Unicode `cmap` maps each code's WinAnsi character to its glyph.
struct Font<'a> {
    base: &'a str,
    /// `(code, drawn)`: the code shown and the character its glyph draws.
    glyphs: &'a [(u8, char)],
    /// A `/ToUnicode` map: `(code, text)` pairs.
    to_unicode: Option<&'a [(u8, char)]>,
    /// `/Widths` for the codes shown (600 when `None`).
    width: Option<&'a dyn Fn(u8) -> u32>,
}

impl<'a> Font<'a> {
    fn new(base: &'a str, glyphs: &'a [(u8, char)]) -> Self {
        Self { base, glyphs, to_unicode: None, width: None }
    }
}

/// The WinAnsi character at `code` (ISO 32000-2 Annex D.2), for the codes the tests show.
fn win_ansi(code: u8) -> char {
    match code {
        0x8A => 'Š',
        0x9A => 'š',
        _ => char::from(code),
    }
}

/// A `/ToUnicode` CMap of `pairs`.
fn to_unicode_cmap(pairs: &[(u8, char)]) -> Vec<u8> {
    let entries: String = pairs.iter().map(|(code, c)| format!("<{code:02X}> <{:04X}>\n", u32::from(*c))).collect();
    format!(
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /T def 1 begincodespacerange <00> <FF> endcodespacerange {} beginbfchar\n{entries}endbfchar endcmap CMapName currentdict /CMap defineresource pop end end",
        pairs.len()
    )
    .into_bytes()
}

/// A page showing `font`'s codes, as a nonsymbolic TrueType font with `/Encoding
/// /WinAnsiEncoding`.
fn page(font: &Font<'_>) -> Vec<u8> {
    let outlines: Vec<(u16, &[Contour])> = font.glyphs.iter().enumerate().map(|(i, (_, c))| (u16::try_from(i + 1).unwrap(), drawing(*c))).collect();
    let cmap: Vec<(char, u16)> = font.glyphs.iter().enumerate().map(|(i, (code, _))| (win_ansi(*code), u16::try_from(i + 1).unwrap())).collect();
    let program = with_outlines(u16::try_from(font.glyphs.len() + 1).unwrap(), &outlines, Cmap::Unicode(&cmap), None);
    page_with_program(font, Program::TrueType(program), "/Encoding /WinAnsiEncoding", 32)
}

/// An embedded program.
enum Program {
    TrueType(Vec<u8>),
    Type1(Vec<u8>),
    Cff(Vec<u8>),
}

/// A page showing `font`'s codes through `program`, with `encoding` and `/Flags` `flags`.
fn page_with_program(font: &Font<'_>, program: Program, encoding: &str, flags: u32) -> Vec<u8> {
    let (subtype, file, stream, program) = match program {
        Program::TrueType(p) => ("TrueType", "FontFile2", "", p),
        Program::Type1(p) => ("Type1", "FontFile", "", p),
        Program::Cff(p) => ("Type1", "FontFile3", "/Subtype /Type1C", p),
    };
    let shown: String = font.glyphs.iter().map(|(code, _)| format!("{code:02X}")).collect();
    let widths: Vec<String> = (0..=255u8).map(|c| font.width.map_or(600, |w| w(c)).to_string()).collect();
    let to_unicode = if font.to_unicode.is_some() { " /ToUnicode 9 0 R" } else { "" };
    let base = font.base;
    let mut pdf = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 300] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F3 6 0 R >> >> >>")
        .stream("", format!("BT /F3 6 Tf 20 150 Td <{shown}> Tj ET"))
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .obj(format!(
            "<< /Type /Font /Subtype /{subtype} /BaseFont /{base} /FirstChar 0 /LastChar 255 /Widths [{}] {encoding} /FontDescriptor 7 0 R{to_unicode} >>",
            widths.join(" ")
        ))
        .obj(format!(
            "<< /Type /FontDescriptor /FontName /{base} /Flags {flags} /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /{file} 8 0 R >>"
        ))
        .stream(stream, program);
    if let Some(pairs) = font.to_unicode {
        pdf = pdf.stream("", to_unicode_cmap(pairs));
    }
    pdf.build()
}

/// The strict text of `font`'s line and, when the rule read it, its recovered text.
fn read(font: &Font<'_>) -> (String, Option<String>, RecoveryCounters) {
    let (strict, recovered, plain, cost) = layers(&parse(page(font)));
    assert_eq!(strict, plain, "the strict layer is the plain extraction's");
    let strict = strict.plain_text();
    let recovered = recovered.map(|r| {
        assert!(
            r.recovery.iter().flatten().all(|x| x.rule == RecoveryRule::CodePageByOutline && matches!(x.mode, RecoveryMode::Override { .. })),
            "{:?}",
            r.recovery
        );
        r.text.plain_text()
    });
    (strict, recovered, cost)
}

/// Windows-1250 letters drawn at their codes in a font that reads as WinAnsi: `č` at 0xE8 (`è`),
/// `ő` at 0xF5 (`õ`), `ű` at 0xFB (`û`), `ł` at 0xB3 (`³`). The glyphs' accents (a caron, double
/// acutes, a stroke and no mark above) choose Windows-1250; Copy with formatting reads them so,
/// and the strict text keeps WinAnsi.
#[test]
fn windows_1250_letters_drawn_at_winansi_codes_are_read_as_drawn() {
    let glyphs = [(b'k', 'k'), (0xE8, 'č'), (0xF5, 'ő'), (0xFB, 'ű'), (0xB3, 'ł'), (0xBD, '˝')];
    let (strict, recovered, cost) = read(&Font::new("ABCDEF+TimesNewRomanCE", &glyphs));
    assert_eq!(strict, "kèõû³½");
    // A floating `˝` is no vote for WinAnsi's `½` (whose denominator reaches the baseline), and
    // only letters are read: Windows-1250's `˝` at 0xBD, a spacing accent, would replace a `½`.
    assert_eq!(recovered.as_deref(), Some("kčőűł½"));
    assert!(cost.outlines_classified <= 5, "only the high codes are classified: {cost:?}");
}

/// Windows-1257 letters: `ā` at 0xE2 (`â`), `š` at 0xF0 (`ð`), `ē` at 0xE7 (`ç`). Windows-1250
/// has `â` and `ç` there too, and `đ` at 0xF0, which no caron draws; Windows-1254's `ğ` at 0xF0
/// (a breve, which counts as a caron) is one vote to Windows-1257's three.
#[test]
fn windows_1257_letters_drawn_at_winansi_codes_are_read_as_drawn() {
    let glyphs = [(0xE2, 'ā'), (0xF0, 'š'), (0xE7, 'ē'), (b'k', 'k')];
    let (strict, recovered, _) = read(&Font::new("ABCDEF+TimesNewRomanBaltic", &glyphs));
    assert_eq!(strict, "âðçk");
    assert_eq!(recovered.as_deref(), Some("āšēk"));
}

/// One code is not enough evidence (a `¿` whose dot an outline check reads as `ż`'s); two are.
/// `ł` at 0xB3 and `ű` at 0xFB are Windows-1250's alone (Mac OS Central European has `≥` and
/// `Ż` there).
#[test]
fn one_code_alone_reads_nothing() {
    let (strict, recovered, _) = read(&Font::new("ABCDEF+Arial", &[(0xB3, 'ł'), (b'o', 'o')]));
    assert_eq!(strict, "³o");
    assert_eq!(recovered, None);
    let (_, recovered, _) = read(&Font::new("ABCDEF+Arial", &[(0xB3, 'ł'), (0xFB, 'ű'), (b'o', 'o')]));
    assert_eq!(recovered.as_deref(), Some("łűo"));
}

/// A single glyph that draws the WinAnsi letter rules the code page out, however many draw its
/// letters: `à` at 0xE0, where Windows-1250 has `ŕ` and Windows-1257 `ą`.
#[test]
fn one_winansi_glyph_keeps_the_strict_text() {
    let glyphs = [(0xE8, 'č'), (0xF5, 'ő'), (0xFB, 'ű'), (0xE0, 'à')];
    let (strict, recovered, _) = read(&Font::new("ABCDEF+TimesNewRomanCE", &glyphs));
    assert_eq!(strict, "èõûà");
    assert_eq!(recovered, None);
    // A WinAnsi font: every glyph draws its WinAnsi letter.
    let glyphs = [(0xE8, 'è'), (0xE0, 'à'), (0xE9, 'é'), (0xF5, 'õ')];
    let (_, recovered, _) = read(&Font::new("ABCDEF+TimesNewRoman", &glyphs));
    assert_eq!(recovered, None);
}

/// Mac OS Central European and Windows-1254 are rivals only: a font whose glyphs they explain
/// best is left alone. `ä` at 0x8A and `ö` at 0x9A are Mac OS Central European (WinAnsi `Š`,
/// `š`); `ğ`, `ş` and a dotless `ı` at 0xF0, 0xFE and 0xFD are Windows-1254 (`ð`, `þ`, `ý`).
#[test]
fn rival_code_pages_are_never_read() {
    let glyphs = [(0x8A, 'ä'), (0x9A, 'ö'), (b'k', 'k')];
    let (strict, recovered, cost) = read(&Font::new("ABCDEF+HelveticaCE", &glyphs));
    assert_eq!(strict, "Ššk");
    assert_eq!(recovered, None);
    assert_eq!(cost.guard_decisions, 1, "{cost:?}");
    let glyphs = [(0xF0, 'ğ'), (0xFE, 'ş'), (0xFD, 'k')];
    let (strict, recovered, _) = read(&Font::new("ABCDEF+TimesNewRomanTUR", &glyphs));
    assert_eq!(strict, "ðþý");
    assert_eq!(recovered, None);
}

/// A `/ToUnicode` map that only repeats WinAnsi (or the font's own glyph names) is no evidence,
/// as producers wrote such maps from the names: the font is read as one without a map. A map
/// that says anything else is the producer's word, and the font is left alone.
#[test]
fn a_to_unicode_map_that_repeats_winansi_is_overridden_and_any_other_is_kept() {
    let glyphs = [(b'k', 'k'), (0xE8, 'č'), (0xF5, 'ő'), (0xFB, 'ű')];
    let repeats = [(b'k', 'k'), (0xE8, 'è'), (0xF5, 'õ'), (0xFB, 'û')];
    let font = Font { to_unicode: Some(&repeats), ..Font::new("ABCDEF+TimesNewRomanCE", &glyphs) };
    let (strict, recovered, _) = read(&font);
    assert_eq!(strict, "kèõû");
    assert_eq!(recovered.as_deref(), Some("kčőű"));

    // The map names the letters drawn: the strict text is right and nothing is recovered.
    let names = [(b'k', 'k'), (0xE8, 'č'), (0xF5, 'ő'), (0xFB, 'ű')];
    let font = Font { to_unicode: Some(&names), ..Font::new("ABCDEF+TimesNewRomanCE", &glyphs) };
    let (strict, recovered, _) = read(&font);
    assert_eq!(strict, "kčőű");
    assert_eq!(recovered, None);

    // One code mapped otherwise (`k` as `x`) is the producer's word too.
    let other = [(b'k', 'x'), (0xE8, 'è'), (0xF5, 'õ'), (0xFB, 'û')];
    let font = Font { to_unicode: Some(&other), ..Font::new("ABCDEF+TimesNewRomanCE", &glyphs) };
    let (strict, recovered, _) = read(&font);
    assert_eq!(strict, "xèõû");
    assert_eq!(recovered, None);
}

/// In a family whose widths a standard font shares (Arial, Helvetica's), `/Widths` that give the
/// WinAnsi letters' widths (`è` 556 at 0xE8, `È` 667 at 0xC8) and not the code page's (`č` 500,
/// `Č` 722) keep the strict text, whatever the outlines say; the code page's widths don't.
#[test]
fn widths_of_the_winansi_letters_keep_the_strict_text() {
    let glyphs = [(0xE8, 'č'), (0xC8, 'Č'), (0xF5, 'ő')];
    let winansi = |c: u8| match c {
        0xE8 => 556,
        0xC8 => 667,
        _ => 556,
    };
    let font = Font { width: Some(&winansi), ..Font::new("ABCDEF+Arial", &glyphs) };
    let (strict, recovered, _) = read(&font);
    assert_eq!(strict, "èÈõ");
    assert_eq!(recovered, None);
    let target = |c: u8| match c {
        0xE8 => 500,
        0xC8 => 722,
        _ => 556,
    };
    let font = Font { width: Some(&target), ..Font::new("ABCDEF+Arial", &glyphs) };
    let (_, recovered, _) = read(&font);
    assert_eq!(recovered.as_deref(), Some("čČő"));
}

/// The accent classifier names each mark of the test's glyphs as a reader would.
#[test]
fn the_accent_classifier_names_each_mark() {
    use hayro::hayro_interpret::font::test_hooks::accent_of;
    let path = |contours: &[Contour]| {
        let mut p = kurbo::BezPath::new();
        for c in contours {
            for (i, &(x, y)) in c.iter().enumerate() {
                let pt = (f64::from(x), f64::from(y));
                if i == 0 { p.move_to(pt) } else { p.line_to(pt) }
            }
            p.close_path();
        }
        p
    };
    for (c, mark, below) in [
        ('č', "Caron", false),
        ('ő', "DoubleAcute", false),
        ('ā', "Macron", false),
        ('ö', "Dieresis", false),
        ('è', "Grave", false),
        ('é', "Acute", false),
        ('â', "Circumflex", false),
        ('ğ', "Breve", false),
        ('ż', "Dot", false),
        ('å', "Ring", false),
        ('õ', "Tilde", false),
        ('ą', "Nothing", true),
        ('ł', "Nothing", false),
        ('³', "Superior", false),
        ('k', "Nothing", false),
    ] {
        assert_eq!(accent_of(&path(drawing(c))), Some((mark.to_string(), below)), "{c}");
    }
    assert_eq!(accent_of(&kurbo::BezPath::new()), None, "an empty glyph");
    // Too many points to look at, and points that aren't finite.
    let mut big = kurbo::BezPath::new();
    big.move_to((0.0, 0.0));
    for i in 0..5000 {
        big.line_to((f64::from(i % 400), f64::from(i % 700)));
    }
    assert_eq!(accent_of(&big), None);
    let mut nan = path(drawing('č'));
    nan.line_to((f64::NAN, 0.0));
    assert_eq!(accent_of(&nan), None);
}

/// Rule K and this rule may read the same code: a symbolic TrueType subset without `/Encoding`
/// that kept its glyph ids in the Macintosh order (rule K reads glyph 105 as `á`, 121 as `ó`)
/// can draw `č` and `ő` there. The outline is the stronger evidence: this rule reads those
/// codes, rule K the rest.
#[test]
fn the_outline_wins_over_the_macintosh_order_where_both_read() {
    let text = "Besluitenlijst";
    let mut pairs: Vec<(u8, u16)> = text.chars().enumerate().map(|(i, c)| (u8::try_from(i + 1).unwrap(), mac_gid(c))).collect();
    // Codes 0xE8, 0xF5 and 0xFB draw glyphs 105, 121 and 126, which the Macintosh order calls
    // `aacute`, `oacute` and `uacute`, but which draw `č`, `ő` and `ű`.
    pairs.extend([(0xE8, 105), (0xF5, 121), (0xFB, 126)]);
    let mut outlines: Vec<(u16, &[Contour])> = text.chars().map(|c| (mac_gid(c), drawing(c))).collect();
    outlines.extend([(105, drawing('č')), (121, drawing('ő')), (126, drawing('ű'))]);
    let program = with_outlines(300, &outlines, Cmap::Symbolic(&pairs), Some(0x0003_0000));
    let glyphs: Vec<(u8, char)> = pairs.iter().map(|&(code, _)| (code, 'x')).collect();
    let font = Font::new("ABCDEF+TTE1A2B3C4t00", &glyphs);
    let (strict, recovered, plain, _) = layers(&parse(page_with_program(&font, Program::TrueType(program), "", 4)));
    assert_eq!(strict, plain);
    assert_eq!(strict.plain_text(), "èõû", "the strict text reads the high codes as WinAnsi");
    let r = recovered.expect("recovered");
    assert_eq!(r.text.plain_text(), format!("{text}čőű"));
    let rules: Vec<RecoveryRule> = r.recovery.iter().flatten().map(|x| x.rule).collect();
    assert_eq!(rules.iter().filter(|r| **r == RecoveryRule::CodePageByOutline).count(), 3, "{rules:?}");
    assert_eq!(rules.iter().filter(|r| **r == RecoveryRule::MacGlyphOrder).count(), text.len(), "{rules:?}");
}

/// The rule's work is bounded by the font, not the page: a font is decided once whatever it
/// shows, and at most its 128 codes from 0x80 on are classified. Outlines too large to look at
/// vote for nothing.
#[test]
fn the_rules_work_is_bounded_by_the_font() {
    let mut glyphs: Vec<(u8, char)> = Vec::new();
    for _ in 0..20 {
        glyphs.extend([(0xE8, 'č'), (0xF5, 'ő'), (0xFB, 'ű')]);
    }
    let (_, recovered, cost) = read(&Font::new("ABCDEF+TimesNewRomanCE", &glyphs));
    assert_eq!(recovered.map(|r| r.chars().filter(|c| *c == 'č').count()), Some(20));
    assert_eq!(cost.guard_decisions, 1, "{cost:?}");
    assert_eq!(cost.outlines_classified, 3, "{cost:?}");

    // Every code from 0x80 on drawn: 128 classified at most, once.
    let all: Vec<(u8, char)> = (0x80..=0xFFu8).map(|c| (c, 'č')).collect();
    let (_, _, cost) = read(&Font::new("ABCDEF+TimesNewRomanCE", &all));
    assert!((1..=128).contains(&cost.outlines_classified), "{cost:?}");
    assert_eq!(cost.guard_decisions, 1, "{cost:?}");
}

/// The glyphs of a Type 1 or CFF program drawing Windows-1250 letters under their WinAnsi glyph
/// names, as the fonts of Windows printer drivers name them: `egrave` draws `č`, `otilde` `ő`,
/// `ucircumflex` `ű`.
const NAMED: &[(&str, &[Contour])] =
    &[("k", &[BODY]), ("egrave", &[BODY, CARON]), ("otilde", &[BODY, ACUTE_LEFT, ACUTE_RIGHT]), ("ucircumflex", &[BODY, ACUTE_LEFT, ACUTE_RIGHT])];

/// The codes of [`NAMED`]'s glyphs, by WinAnsi.
const NAMED_CODES: &[(u8, char)] = &[(b'k', 'k'), (0xE8, 'č'), (0xF5, 'ő'), (0xFB, 'ű')];

/// The strict and recovered text of `program`'s page, shown as [`NAMED_CODES`] by WinAnsi.
fn read_program(program: Program) -> (String, Option<String>) {
    let font = Font::new("ABCDEF+TimesNewRomanCE", NAMED_CODES);
    let (strict, recovered, plain, cost) = layers(&parse(page_with_program(&font, program, "/Encoding /WinAnsiEncoding", 32)));
    assert_eq!(strict, plain);
    assert!(cost.outlines_classified >= 3, "{cost:?}");
    (strict.plain_text(), recovered.map(|r| r.text.plain_text()))
}

/// The rule reads each simple font kind it lists: Type 1 (`/FontFile`) and CFF (`/FontFile3`)
/// programs as well as TrueType ones. Removing a kind from the rule's list fails here.
#[test]
fn type1_and_cff_programs_are_read_as_well() {
    use pdfcraft_testkit::type1::{Shape, program};
    let (strict, recovered) = read_program(Program::Type1(program(NAMED, Shape::COMPLETE)));
    assert_eq!(strict, "kèõû");
    assert_eq!(recovered.as_deref(), Some("kčőű"));
    let (strict, recovered) = read_program(Program::Cff(pdfcraft_testkit::cff::named(NAMED)));
    assert_eq!(strict, "kèõû");
    assert_eq!(recovered.as_deref(), Some("kčőű"));
}

/// Type 1 programs as some producers embed them: without a `/Subrs` array, or cut off after the
/// encrypted part (no zeros, no `cleartomark`). Their glyphs are read as drawn.
#[test]
fn type1_programs_without_subrs_or_trailer_are_read() {
    use pdfcraft_testkit::type1::{Shape, program};
    for shape in [Shape { subrs: false, trailer: true }, Shape { subrs: true, trailer: false }, Shape { subrs: false, trailer: false }] {
        let (strict, recovered) = read_program(Program::Type1(program(NAMED, shape)));
        assert_eq!(strict, "kèõû", "{shape:?}");
        assert_eq!(recovered.as_deref(), Some("kčőű"), "{shape:?}");
    }
}

/// A TrueType program whose `cmap` table is cut short, as some producers wrote it: its last
/// subtable, the Unicode one, needs six bytes more than the table has, and its Macintosh one is
/// whole. Its glyphs are still found and read.
#[test]
fn a_truetype_program_with_a_cut_cmap_is_read() {
    let glyphs = [(b'k', 'k'), (0xE8, 'č'), (0xF5, 'ő'), (0xFB, 'ű')];
    // Mac OS Roman codes of `k`, `è`, `õ` and `û`.
    let mac = [(b'k', 1), (0x8F, 2), (0x9B, 3), (0x9E, 4)];
    let unicode = [('k', 1), ('è', 2), ('õ', 3), ('û', 4)];
    let outlines: Vec<(u16, &[Contour])> = glyphs.iter().enumerate().map(|(i, (_, c))| (u16::try_from(i + 1).unwrap(), drawing(*c))).collect();
    let mut program = with_outlines(5, &outlines, Cmap::MacThenUnicode(&mac, &unicode), None);
    // The cmap table's record in the table directory: tag, checksum, offset, length.
    let at = program.windows(4).position(|w| w == b"cmap").expect("a cmap record");
    let length = u32::from_be_bytes(program[at + 12..at + 16].try_into().unwrap());
    program[at + 12..at + 16].copy_from_slice(&(length - 6).to_be_bytes());
    let font = Font::new("ABCDEF+DutchTL", &glyphs);
    let (strict, recovered, plain, _) = layers(&parse(page_with_program(&font, Program::TrueType(program), "/Encoding /WinAnsiEncoding", 32)));
    assert_eq!(strict, plain);
    assert_eq!(strict.plain_text(), "kèõû");
    assert_eq!(recovered.map(|r| r.text.plain_text()).as_deref(), Some("kčőű"));
}

/// The glyphs that draw neither letter must be fewer than those that draw the code page's: `ű`
/// and `ł` against two glyphs that draw no letter of Windows-1250 or WinAnsi is a font of some
/// other encoding; a `č` decides it.
#[test]
fn glyphs_that_draw_neither_letter_must_be_fewer() {
    let even = [(0xFB, 'ű'), (0xB3, 'ł'), (0xEC, '*'), (0xF2, '*')];
    let (strict, recovered, _) = read(&Font::new("ABCDEF+TimesNewRomanCE", &even));
    assert_eq!(strict, "û³ìò");
    assert_eq!(recovered, None);
    let more = [(0xFB, 'ű'), (0xB3, 'ł'), (0xEC, '*'), (0xF2, '*'), (0xE8, 'č')];
    let (_, recovered, _) = read(&Font::new("ABCDEF+TimesNewRomanCE", &more));
    assert_eq!(recovered.as_deref(), Some("űłěňč"));
}

/// A font whose codes from 0x80 on read as the letters of another encoding (Mac OS Roman's
/// `Adieresis` and `Aring` at 0x80 and 0x81, where WinAnsi has the euro sign and nothing) has an
/// encoding of its own: it is no candidate, however its glyphs vote. One such letter, or a
/// punctuation mark named otherwise (`endash` at 0x85), doesn't make it one.
#[test]
fn a_font_whose_high_codes_read_as_other_letters_is_not_read() {
    let glyphs = [(0xE8, 'č'), (0xF5, 'ő'), (0xFB, 'ű'), (0x80, 'k'), (0x81, 'k'), (0x85, 'k')];
    let outlines: Vec<(u16, &[Contour])> = glyphs.iter().enumerate().map(|(i, (_, c))| (u16::try_from(i + 1).unwrap(), drawing(*c))).collect();
    let page_of = |differences: &str, names: [char; 3]| {
        let cmap = [('è', 1), ('õ', 2), ('û', 3), (names[0], 4), (names[1], 5), (names[2], 6)];
        let program = with_outlines(7, &outlines, Cmap::Unicode(&cmap), None);
        let font = Font::new("ABCDEF+TimesNewRomanCE", &glyphs);
        let encoding = format!("/Encoding << /BaseEncoding /WinAnsiEncoding /Differences [{differences}] >>");
        layers(&parse(page_with_program(&font, Program::TrueType(program), &encoding, 32)))
    };
    let (strict, recovered, plain, _) = page_of("128 /Adieresis /Aring 133 /endash", ['Ä', 'Å', '–']);
    assert_eq!(strict, plain);
    assert_eq!(strict.plain_text(), "èõûÄÅ–");
    assert!(recovered.is_none());
    // One letter and a dash named otherwise.
    let (_, recovered, _, _) = page_of("128 /Adieresis 133 /endash", ['Ä', '\u{2022}', '–']);
    assert_eq!(recovered.map(|r| r.text.plain_text()).as_deref(), Some("čőűÄ\u{2022}–"));
}

/// Codes Windows-1252 leaves undefined read as bullets in WinAnsi (ISO 32000-2 Annex D.2); the
/// code page's letter replaces one only where its own glyph doesn't contradict it. `ť` at 0x9D is
/// read, whether its apostrophe stands above the `t` or starts below its top; a `Ž` at 0x8F, where
/// Windows-1250 has `Ź`, and a bullet at 0x8D, where it has `Ť`, are not.
#[test]
fn undefined_codes_are_read_only_where_their_glyphs_dont_contradict_the_letter() {
    for t in ['ť', 'Ť'] {
        let glyphs = [(0xE8, 'č'), (0xF5, 'ő'), (0xFB, 'ű'), (0x9D, t), (0x8F, 'Ž'), (0x8D, '•')];
        let pairs: Vec<(u8, u16)> = glyphs.iter().enumerate().map(|(i, (code, _))| (*code, u16::try_from(i + 1).unwrap())).collect();
        let outlines: Vec<(u16, &[Contour])> = glyphs.iter().enumerate().map(|(i, (_, c))| (u16::try_from(i + 1).unwrap(), drawing(*c))).collect();
        let program = with_outlines(7, &outlines, Cmap::Symbolic(&pairs), None);
        let font = Font::new("ABCDEF+TimesNewRomanCE", &glyphs);
        let (strict, recovered, plain, _) = layers(&parse(page_with_program(&font, Program::TrueType(program), "", 4)));
        assert_eq!(strict, plain);
        assert_eq!(strict.plain_text(), "èõû\u{2022}\u{2022}\u{2022}");
        let r = recovered.expect("recovered");
        assert_eq!(r.text.plain_text(), "čőűť\u{2022}\u{2022}", "{t}");
        let modes: Vec<RecoveryMode> = r.recovery.iter().flatten().map(|x| x.mode).collect();
        assert_eq!(modes.len(), 4, "{modes:?}");
        assert!(modes.iter().all(|m| matches!(m, RecoveryMode::Override { .. })), "{modes:?}");
    }
}

/// A slashed `O` whose slash reaches below the baseline is no ogonek: `Ø` and `ø` at 0xD8 and 0xF8
/// (a diameter sign in a German catalogue) don't read as Windows-1257's `Ų` and `ų`.
#[test]
fn a_slashed_o_reaching_below_the_baseline_is_not_an_ogonek() {
    let glyphs = [(0xD8, 'Ø'), (0xF8, 'ø'), (b'k', 'k')];
    let (strict, recovered, _) = read(&Font::new("ABCDEF+MetaNormal-Roman", &glyphs));
    assert_eq!(strict, "Øøk");
    assert_eq!(recovered, None);
}

/// Review on #6977: a page of many fonts can't keep the rules busy past the render pool's
/// watchdog, which would fail the page's strict text with its recovered layer. Once the rules
/// have spent `RECOVERY_RULES_BUDGET` on the page's fonts, the page stops recovering at its next
/// font: no more guards are decided, there is no recovered layer, and the strict text is the
/// plain extraction's.
#[test]
fn a_page_whose_fonts_spend_the_rules_time_keeps_its_strict_text() {
    let glyphs = [(0xE8, 'č'), (0xF5, 'ő'), (0xFB, 'ű')];
    let outlines: Vec<(u16, &[Contour])> = glyphs.iter().enumerate().map(|(i, (_, c))| (u16::try_from(i + 1).unwrap(), drawing(*c))).collect();
    let cmap: Vec<(char, u16)> = glyphs.iter().enumerate().map(|(i, (code, _))| (win_ansi(*code), u16::try_from(i + 1).unwrap())).collect();
    let program = with_outlines(4, &outlines, Cmap::Unicode(&cmap), None);
    // Six fonts, each with its own descriptor and program, each showing the three letters.
    let fonts = 6;
    let resources: String = (0..fonts).map(|i| format!("/F{i} {} 0 R ", 5 + 3 * i)).collect();
    let content: String = (0..fonts).map(|i| format!("/F{i} 6 Tf 20 {} Td <E8F5FB> Tj ", if i == 0 { 250 } else { -20 })).collect();
    let widths = vec!["600"; 256].join(" ");
    let mut pdf = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 300] >>")
        .obj(format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << {resources}>> >> >>"))
        .stream("", format!("BT {content}ET"));
    for i in 0..fonts {
        let (descriptor, file) = (6 + 3 * i, 7 + 3 * i);
        pdf = pdf
            .obj(format!(
                "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+TimesNewRomanCE{i} /FirstChar 0 /LastChar 255 /Widths [{widths}] /Encoding /WinAnsiEncoding /FontDescriptor {descriptor} 0 R >>"
            ))
            .obj(format!(
                "<< /Type /FontDescriptor /FontName /ABCDEF+TimesNewRomanCE{i} /Flags 32 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 {file} 0 R >>"
            ))
            .stream("", program.clone());
    }
    let pdf = parse(pdf.build());
    let (strict, recovered, plain, cost) = layers(&pdf);
    assert_eq!(strict, plain);
    assert_eq!(recovered.map(|r| r.text.plain_text().matches("čőű").count()), Some(fonts), "in time, every font is read");
    assert_eq!(cost.guard_decisions, fonts, "{cost:?}");

    // Each font's rules now take 40% of the budget: the third font finds it spent.
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            RULES_MILLIS.with(|m| m.set(None));
        }
    }
    let millis = u64::try_from(RECOVERY_RULES_BUDGET.as_millis() * 2 / 5).unwrap_or(u64::MAX);
    RULES_MILLIS.with(|m| m.set(Some(millis)));
    let _reset = Reset;
    let (slow, recovered, plain, cost) = layers(&pdf);
    assert_eq!(slow, plain);
    assert_eq!(slow, strict, "the strict text is the same");
    assert!(recovered.is_none(), "past the rules' budget the page has no recovered layer");
    assert_eq!(cost.guard_decisions, 3, "no font past the budget is decided: {cost:?}");
    assert!(RECOVERY_RULES_BUDGET.saturating_mul(4) <= crate::raster::STUCK_AFTER);
}

/// Review on #6977: one font's outlines are bounded too. A font whose codes from 0x80 on draw
/// more than 65,536 points in all (sixteen glyphs of 4,300 points, too large for the classifier
/// to look at, so they vote for nothing) stops being drawn there and is not read, though the
/// letters it shows would be.
#[test]
fn a_font_whose_outlines_are_too_large_is_not_read() {
    let shown = [(0xE8, 'č'), (0xF5, 'ő'), (0xFB, 'ű')];
    // A body and 1,432 small triangles above it: 4,300 points.
    let contours: Vec<Vec<(i16, i16)>> = std::iter::once(BODY.to_vec())
        .chain((0..1432).map(|k: i16| {
            let (x, y) = ((k % 100) * 10, 500 + (k / 100) * 10);
            vec![(x, y), (x + 3, y), (x, y + 3)]
        }))
        .collect();
    let heavy: Vec<&[(i16, i16)]> = contours.iter().map(Vec::as_slice).collect();
    let read_with = |heavy_codes: std::ops::RangeInclusive<u8>| {
        let mut outlines: Vec<(u16, pdfcraft_testkit::Outline<'_>)> =
            shown.iter().enumerate().map(|(i, (_, c))| (u16::try_from(i + 1).unwrap(), drawing(*c))).collect();
        outlines.push((4, heavy.as_slice()));
        let mut cmap: Vec<(char, u16)> = shown.iter().enumerate().map(|(i, (code, _))| (win_ansi(*code), u16::try_from(i + 1).unwrap())).collect();
        // Capital letters with an accent in Windows-1252 at 0xC0 on (`À`, `Á`, ...).
        cmap.extend(heavy_codes.map(|code| (char::from(code), 4)));
        let program = with_outlines(5, &outlines, Cmap::Unicode(&cmap), None);
        let font = Font::new("ABCDEF+TimesNewRomanCE", &shown);
        let (strict, recovered, plain, cost) = layers(&parse(page_with_program(&font, Program::TrueType(program), "/Encoding /WinAnsiEncoding", 32)));
        assert_eq!(strict, plain);
        (recovered.map(|r| r.text.plain_text()), cost)
    };
    // Fifteen such glyphs (64,500 points) still leave the letters their votes.
    let (recovered, cost) = read_with(0xC0..=0xCE);
    assert_eq!(recovered.as_deref(), Some("čőű"));
    assert_eq!(cost.outlines_classified, 18, "{cost:?}");
    let (recovered, cost) = read_with(0xC0..=0xCF);
    assert_eq!(recovered, None);
    assert_eq!(cost.outlines_classified, 15, "the sixteenth outline is past the budget: {cost:?}");
}

/// Review on #6977: read with the document's recovery state, font objects with byte-identical
/// dictionaries are fonts of their own (each decided by the rules), and the page's budgets count
/// them so: shown one after another, each one's guards are timed and counted, not taken as the
/// font before.
#[test]
fn font_objects_with_the_same_dictionary_each_count_toward_the_rules_time() {
    use hayro::hayro_interpret::font::RecoveryDocument;
    let glyphs = [(0xE8, 'č'), (0xF5, 'ő'), (0xFB, 'ű')];
    let outlines: Vec<(u16, &[Contour])> = glyphs.iter().enumerate().map(|(i, (_, c))| (u16::try_from(i + 1).unwrap(), drawing(*c))).collect();
    let cmap: Vec<(char, u16)> = glyphs.iter().enumerate().map(|(i, (code, _))| (win_ansi(*code), u16::try_from(i + 1).unwrap())).collect();
    let program = with_outlines(4, &outlines, Cmap::Unicode(&cmap), None);
    let fonts = 6;
    let resources: String = (0..fonts).map(|i| format!("/F{i} {} 0 R ", 7 + i)).collect();
    let content: String = (0..fonts).map(|i| format!("/F{i} 6 Tf 20 {} Td <E8F5FB> Tj ", if i == 0 { 250 } else { -20 })).collect();
    let widths = vec!["600"; 256].join(" ");
    let mut pdf = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 300] >>")
        .obj(format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << {resources}>> >> >>"))
        .stream("", format!("BT {content}ET"))
        .obj("<< /Type /FontDescriptor /FontName /ABCDEF+TimesNewRomanCE /Flags 32 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 6 0 R >>")
        .stream("", program);
    for _ in 0..fonts {
        pdf = pdf.obj(format!(
            "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+TimesNewRomanCE /FirstChar 0 /LastChar 255 /Widths [{widths}] /Encoding /WinAnsiEncoding /FontDescriptor 5 0 R >>"
        ));
    }
    let pdf = parse(pdf.build());
    let read = || {
        let settings =
            InterpreterSettings { recovery_document: Some(std::sync::Arc::new(RecoveryDocument::default())), ..InterpreterSettings::default() };
        let before = recovery_counters();
        let (strict, recovered) = extract_page_recovering(&pdf, 0, &settings).unwrap();
        (strict, recovered, recovery_counters().since(&before))
    };
    let (strict, recovered, cost) = read();
    assert_eq!(recovered.map(|r| r.text.plain_text().matches("čőű").count()), Some(fonts));
    assert_eq!(cost.guard_decisions, fonts, "each font object is decided: {cost:?}");

    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            RULES_MILLIS.with(|m| m.set(None));
        }
    }
    let millis = u64::try_from(RECOVERY_RULES_BUDGET.as_millis() * 2 / 5).unwrap_or(u64::MAX);
    RULES_MILLIS.with(|m| m.set(Some(millis)));
    let _reset = Reset;
    let (slow, recovered, cost) = read();
    assert_eq!(slow, strict);
    assert!(recovered.is_none(), "past the rules' budget the page has no recovered layer");
    assert_eq!(cost.guard_decisions, 3, "no font object past the budget is decided: {cost:?}");
}
