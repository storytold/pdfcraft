//! What a hidden layer's marked block leaves behind (ISO 32000-1 §8.11.3.1): hidden content is
//! not drawn, but its graphics-state, clipping and text-position changes still apply after `EMC`.
//!
//! [`Residue`] takes a hidden block's operators one at a time and keeps those changes in their
//! order: graphics and text state, colours, `q`/`Q` (so their pairing is unchanged), text objects
//! and positioning, and paths that clip (ended by `n` in place of any painting); a path nothing paints goes. Painting goes:
//! path painting, `Do`, `sh`, inline images, and any operator not known to change state. A `q`…`Q`
//! pair it keeps that holds no text operator changes nothing after its `Q`, and goes too.
//!
//! Flatten Layers uses it; Remove Hidden Information removes a hidden block whole.

use pdfcraft_content::Op;

/// The kept operators of one hidden block (module docs).
#[derive(Default)]
pub(crate) struct Residue {
    ops: Vec<Op>,
    /// The path being built, held until what ends it says whether it clips.
    path: Vec<Op>,
    clips: bool,
    /// Where each `q` this block opened and hasn't closed is in `ops`.
    saves: Vec<usize>,
}

/// Operators whose effect outlives a hidden block.
fn changes_state(op: &[u8]) -> bool {
    matches!(
        op,
        b"cm"
            | b"w"
            | b"J"
            | b"j"
            | b"M"
            | b"d"
            | b"ri"
            | b"i"
            | b"gs"
            | b"CS"
            | b"cs"
            | b"SC"
            | b"SCN"
            | b"sc"
            | b"scn"
            | b"G"
            | b"g"
            | b"RG"
            | b"rg"
            | b"K"
            | b"k"
            | b"Tc"
            | b"Tw"
            | b"Tz"
            | b"TL"
            | b"Tf"
            | b"Tr"
            | b"Ts"
            | b"d0"
            | b"d1"
    ) || is_text(op)
}

/// Text objects, positioning and showing, and compatibility sections: a `Q` doesn't undo them
/// in every reader, so a `q`…`Q` holding one is kept.
fn is_text(op: &[u8]) -> bool {
    matches!(op, b"BT" | b"ET" | b"Td" | b"TD" | b"Tm" | b"T*" | b"BX" | b"EX") || is_show(op)
}

/// The text-showing operators.
pub(crate) fn is_show(op: &[u8]) -> bool {
    matches!(op, b"Tj" | b"TJ" | b"'" | b"\"")
}

impl Residue {
    /// Takes the block's next operator (not its nested marked-content operators). A text-showing
    /// one is kept as it is when `keep_shows` (the caller replaces it, knowing the font);
    /// otherwise only its move to the next line (`'`, `"`) is.
    pub(crate) fn push(&mut self, op: Op, keep_shows: bool) {
        let o = op.op.as_slice();
        if matches!(o, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" | b"W" | b"W*") {
            self.clips |= matches!(o, b"W" | b"W*");
            self.path.push(op);
            return;
        }
        if matches!(o, b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n") {
            if self.clips {
                self.ops.append(&mut self.path);
                self.ops.push(Op::new("n", Vec::new()));
            }
            self.path.clear();
            self.clips = false;
            return;
        }
        // A path no painting operator ends draws nothing (a path object can't hold another
        // operator, nor span the block's end, ISO 32000-1 §8.2).
        self.path.clear();
        self.clips = false;
        match o {
            b"q" => {
                self.saves.push(self.ops.len());
                self.ops.push(op);
            }
            b"Q" => self.restore(op),
            _ if is_show(o) && !keep_shows => self.ops.extend(line_move(&op)),
            _ if changes_state(o) => self.ops.push(op),
            _ => {}
        }
    }

    /// `Q`: the pair goes with what it holds when that holds no text operator.
    fn restore(&mut self, op: Op) {
        if let Some(at) = self.saves.pop()
            && self.ops.get(at..).is_some_and(|held| held.iter().all(|o| !is_text(&o.op)))
        {
            self.ops.truncate(at);
            return;
        }
        self.ops.push(op);
    }

    /// The kept operators; a path still open at the block's end goes (see [`Residue::push`]).
    pub(crate) fn finish(self) -> Vec<Op> {
        self.ops
    }
}

/// What a text-showing operator does besides showing: `'` moves to the next line, and `"` sets
/// word and character spacing first.
fn line_move(op: &Op) -> Vec<Op> {
    let mut out = Vec::new();
    if op.is("\"") {
        // `aw ac string "`: the two numbers before the string.
        for (k, set) in [(2, "Tw"), (1, "Tc")] {
            if let Some(v) = op.back(k).filter(|v| v.as_f64().is_some()) {
                out.push(Op::new(set, vec![v.clone()]));
            }
        }
    }
    if op.is("'") || op.is("\"") {
        out.push(Op::new("T*", Vec::new()));
    }
    out
}
