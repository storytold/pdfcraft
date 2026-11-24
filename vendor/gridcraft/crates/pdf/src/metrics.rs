//! Advance widths of the standard 14 PDF fonts, in 1/1000 em, for the printable ASCII range.
//!
//! Source: the glyph advance widths ("WX" values) of Adobe's Core 14 AFM font metrics files
//! (Helvetica, Helvetica-Bold, Times-Roman, Times-Bold, Courier). Adobe distributes those metrics
//! files with permission to use, copy and distribute them for any purpose ("This file and the 14
//! PostScript(R) AFM files it accompanies may be used, copied, and distributed for any purpose and
//! without charge, with or without modification, provided that all copyright notices are retained;
//! that the AFM files are not distributed without this file; that all modifications to this file
//! or any of the AFM files are prominently noted in the modified file(s); and that this paragraph
//! is not modified."). Only the numeric widths are reproduced here, typed in from the public
//! metrics; no AFM file is included. Oblique/italic faces reuse the upright widths (an
//! approximation that is exact for Helvetica and close for Times), and characters outside ASCII
//! are approximated by their base letter.

/// Helvetica and Helvetica-Oblique, chars 32..=126.
pub(crate) const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // ' '..'/'
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // '0'..'9'
    278, 278, 584, 584, 584, 556, 1015, // ':'..'@'
    667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, // 'A'..'M'
    722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, // 'N'..'Z'
    278, 278, 278, 469, 556, 333, // '['..'`'
    556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, // 'a'..'m'
    556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, // 'n'..'z'
    334, 260, 334, 584, // '{'..'~'
];

/// Helvetica-Bold and Helvetica-BoldOblique, chars 32..=126.
pub(crate) const HELVETICA_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, // ' '..'/'
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // '0'..'9'
    333, 333, 584, 584, 584, 611, 975, // ':'..'@'
    722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, // 'A'..'M'
    722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, // 'N'..'Z'
    333, 278, 333, 584, 556, 333, // '['..'`'
    556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, // 'a'..'m'
    611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, // 'n'..'z'
    389, 280, 389, 584, // '{'..'~'
];

/// Times-Roman and Times-Italic (approximation), chars 32..=126.
pub(crate) const TIMES: [u16; 95] = [
    250, 333, 408, 500, 500, 833, 778, 180, 333, 333, 500, 564, 250, 333, 250, 278, // ' '..'/'
    500, 500, 500, 500, 500, 500, 500, 500, 500, 500, // '0'..'9'
    278, 278, 564, 564, 564, 444, 921, // ':'..'@'
    722, 667, 667, 722, 611, 556, 722, 722, 333, 389, 722, 611, 889, // 'A'..'M'
    722, 722, 556, 722, 667, 556, 611, 722, 722, 944, 722, 722, 611, // 'N'..'Z'
    333, 278, 333, 469, 500, 333, // '['..'`'
    444, 500, 444, 500, 444, 333, 500, 500, 278, 278, 500, 278, 778, // 'a'..'m'
    500, 500, 500, 500, 333, 389, 278, 500, 500, 722, 500, 500, 444, // 'n'..'z'
    480, 200, 480, 541, // '{'..'~'
];

/// Times-Bold and Times-BoldItalic (approximation), chars 32..=126.
pub(crate) const TIMES_BOLD: [u16; 95] = [
    250, 333, 555, 500, 500, 1000, 833, 278, 333, 333, 500, 570, 250, 333, 250, 278, // ' '..'/'
    500, 500, 500, 500, 500, 500, 500, 500, 500, 500, // '0'..'9'
    333, 333, 570, 570, 570, 500, 930, // ':'..'@'
    722, 667, 722, 722, 667, 611, 778, 778, 389, 500, 778, 667, 944, // 'A'..'M'
    722, 778, 611, 778, 722, 556, 667, 722, 722, 1000, 722, 722, 667, // 'N'..'Z'
    333, 278, 333, 581, 500, 333, // '['..'`'
    500, 556, 444, 556, 444, 333, 500, 556, 278, 333, 556, 278, 833, // 'a'..'m'
    556, 500, 556, 556, 444, 389, 333, 556, 500, 722, 500, 500, 444, // 'n'..'z'
    394, 220, 394, 520, // '{'..'~'
];

/// Maps a non-ASCII character to an ASCII character of similar width, or `None` to use the
/// font's default width.
pub(crate) fn fold(c: char) -> Option<char> {
    Some(match c {
        'À'..='Å' => 'A',
        'Æ' => 'W',
        'Ç' => 'C',
        'È'..='Ë' => 'E',
        'Ì'..='Ï' => 'I',
        'Ð' => 'D',
        'Ñ' => 'N',
        'Ò'..='Ö' | 'Ø' => 'O',
        '×' => '+',
        'Ù'..='Ü' => 'U',
        'Ý' | 'Ÿ' => 'Y',
        'Þ' => 'P',
        'ß' => 'b',
        'à'..='å' => 'a',
        'æ' => 'm',
        'ç' => 'c',
        'è'..='ë' => 'e',
        'ì'..='ï' => 'i',
        'ð' => 'o',
        'ñ' => 'n',
        'ò'..='ö' | 'ø' => 'o',
        '÷' => '+',
        'ù'..='ü' => 'u',
        'ý' | 'ÿ' => 'y',
        'þ' => 'p',
        'Š' => 'S',
        'š' => 's',
        'Ž' => 'Z',
        'ž' => 'z',
        'Œ' => 'W',
        'œ' => 'm',
        '‘'..='‚' => '\'',
        '“'..='„' => '"',
        '–' | '€' | '£' | '¥' | '¢' | '¤' => '0',
        '\u{a0}' => ' ',
        '·' | '•' => '.',
        '«' | '»' | '‹' | '›' => '<',
        '¡' => '!',
        '¿' => '?',
        '¦' => '|',
        '°' | '¹' | '²' | '³' | 'ª' | 'º' => '*',
        _ => return None,
    })
}
