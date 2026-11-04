//! `FileStructureSchema` and its token interpreter [WD 6.3, 9].
//!
//! A producer newer than the reader describes, per entity type, the fields it appended; the
//! reader runs that program after the fields it knows, to step over the rest [WD 9.1]. Token
//! values follow the WD 9.2 table as corrected by PDF Association pdf-issues #407
//! (`Block_Start` = 19; 39 and 40 swallow one token).
//!
//! Choices the sources leave open, made here:
//! - `SimpleFor` reads its count as an `UnsignedInteger` [WD 9.3.17 pseudocode; pdf-issues
//!   #575]: that read also decodes an `Integer`-written non-negative count, while a signed
//!   read does not decode an unsigned one whose top group is 0x80 or higher.
//! - `Block_Version V` runs its block only when `V` is newer than
//!   [`crate::PRC_READER_VERSION`]: an older block holds fields this reader already reads
//!   natively [WD 9.3.20].
//! - `Extent_1D` and `Extent_2D` read 2 and 4 `Double`s: `Interval` and `Domain`
//!   [WD 8.2.2, 8.2.4].
//! - Expression values are `i64`; a `Double` read as an operand truncates (NaN reads as 0).
//! - Variables are not block-scoped: a program that reuses an index after its block ends
//!   reads the stale value instead of an error.
//! - Pointer tokens (12-14) and `Value_CurveIs3D` (28) need the exact-geometry reader and are
//!   refused with [`PrcError::Unsupported`].

use std::collections::HashMap;

use crate::PrcError;
use crate::bits::BitReader;

/// Nesting depth of blocks, loops, expressions and `Father_Type` calls.
const MAX_DEPTH: u32 = 64;

/// The per-entity-type programs at the start of a globals or model-file
/// bitstream [WD 6.3.1].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Schema {
    programs: HashMap<u32, Vec<u32>>,
}

impl Schema {
    /// Read a schema: `UnsignedInteger` count, then per entry the entity
    /// type, a token count and the tokens [WD 6.3.2].
    ///
    /// # Errors
    /// [`PrcError::Truncated`] when a count runs past the data;
    /// [`PrcError::Malformed`] for an entity type listed twice; [`PrcError::TooManyElements`]
    /// past the reader's allowance, one element per entry and per token.
    pub fn read(r: &mut BitReader<'_>) -> Result<Self, PrcError> {
        let n = r.unsigned_integer()? as usize;
        // Every entry takes at least two bits.
        if n > r.remaining() / 2 {
            return Err(PrcError::Truncated("the schema"));
        }
        let mut programs = HashMap::new();
        for _ in 0..n {
            // The entry itself costs one element, so empty programs are not free.
            r.take_elements(1)?;
            let entity_type = r.unsigned_integer()?;
            let len = r.unsigned_integer()? as usize;
            if len > r.remaining() {
                return Err(PrcError::Truncated("a schema program"));
            }
            r.take_elements(len)?;
            let tokens = (0..len).map(|_| r.unsigned_integer()).collect::<Result<Vec<_>, _>>()?;
            if programs.insert(entity_type, tokens).is_some() {
                return Err(PrcError::Malformed(format!("schema lists entity type {entity_type} twice")));
            }
        }
        Ok(Self { programs })
    }

    /// True when no entity type has added fields (the common case: the file
    /// was written at the version this reader implements).
    pub fn is_empty(&self) -> bool {
        self.programs.is_empty()
    }

    /// The entity types the schema describes, unordered.
    pub fn entity_types(&self) -> impl Iterator<Item = u32> + '_ {
        self.programs.keys().copied()
    }

    /// Consume the fields a newer producer appended to `entity_type`, if the
    /// schema describes it; a no-op otherwise. Each step is charged to `r`, so every
    /// program run on one stream shares [`crate::MAX_SCHEMA_STEPS`].
    ///
    /// # Errors
    /// [`PrcError::Unsupported`] for a token that needs exact geometry;
    /// [`PrcError::Malformed`] for an ill-formed program, one past the depth ceiling, or
    /// one that runs out of the stream's steps; any read error from `r`.
    pub fn skip_added_fields(&self, entity_type: u32, r: &mut BitReader<'_>) -> Result<(), PrcError> {
        let Some(program) = self.programs.get(&entity_type) else {
            return Ok(());
        };
        let mut exec = Exec { schema: self, r, vars: HashMap::new(), loop_counter: 0 };
        exec.program(program, 0)
    }
}

struct Exec<'s, 'r, 'd> {
    schema: &'s Schema,
    r: &'r mut BitReader<'d>,
    vars: HashMap<u32, i64>,
    loop_counter: i64,
}

fn malformed(what: &str) -> PrcError {
    PrcError::Malformed(format!("schema program: {what}"))
}

/// The token at `*pos`, advancing past it.
fn next(prog: &[u32], pos: &mut usize) -> Result<u32, PrcError> {
    let t = prog.get(*pos).copied().ok_or_else(|| malformed("ends mid-statement"))?;
    *pos += 1;
    Ok(t)
}

impl Exec<'_, '_, '_> {
    fn tick(&mut self, depth: u32) -> Result<(), PrcError> {
        if !self.r.take_step() {
            return Err(malformed("step ceiling reached"));
        }
        if depth > MAX_DEPTH {
            return Err(malformed("nesting ceiling reached"));
        }
        Ok(())
    }

    fn program(&mut self, prog: &[u32], depth: u32) -> Result<(), PrcError> {
        let mut pos = 0;
        while pos < prog.len() {
            self.stmt(prog, &mut pos, true, depth)?;
        }
        Ok(())
    }

    /// One data read for tokens 0-5 and 7-11, returning its value as an
    /// operand.
    fn read(&mut self, token: u32) -> Result<i64, PrcError> {
        let doubles = match token {
            0 => return Ok(i64::from(self.r.bit()?)),
            1 => return Ok(self.r.double()? as i64),
            2 => return Ok(i64::from(self.r.character()?)),
            3 => return Ok(i64::from(self.r.unsigned_integer()?)),
            4 => return Ok(i64::from(self.r.integer()?)),
            5 => {
                self.r.string()?;
                return Ok(0);
            }
            7 => 2,
            8 => 3,
            9 => 2,
            10 => 4,
            11 => 6,
            _ => return Err(malformed("not a data token")),
        };
        for _ in 0..doubles {
            self.r.double()?;
        }
        Ok(0)
    }

    /// Statements until `Block_End`, which is consumed.
    fn block(&mut self, prog: &[u32], pos: &mut usize, run: bool, depth: u32) -> Result<(), PrcError> {
        loop {
            if prog.get(*pos) == Some(&21) {
                *pos += 1;
                return Ok(());
            }
            self.stmt(prog, pos, run, depth + 1)?;
        }
    }

    /// Run `body` (one statement at `*pos`) `count` times; `*pos` ends past it.
    fn repeat(&mut self, prog: &[u32], pos: &mut usize, count: i64, run: bool, depth: u32) -> Result<(), PrcError> {
        let start = *pos;
        self.stmt(prog, pos, false, depth + 1)?;
        if run {
            let saved = self.loop_counter;
            for i in 0..count.max(0) {
                self.tick(depth)?;
                self.loop_counter = i;
                let mut p = start;
                self.stmt(prog, &mut p, true, depth + 1)?;
            }
            self.loop_counter = saved;
        }
        Ok(())
    }

    /// One statement; with `run` false, only step over its tokens.
    fn stmt(&mut self, prog: &[u32], pos: &mut usize, run: bool, depth: u32) -> Result<(), PrcError> {
        self.tick(depth)?;
        match next(prog, pos)? {
            t @ (0..=5 | 7..=11) => {
                if run {
                    self.read(t)?;
                }
            }
            6 => self.parent_program(prog, pos, run, depth)?,
            12 => {
                next(prog, pos)?;
                if run {
                    return Err(PrcError::Unsupported("a schema pointer field"));
                }
            }
            13 | 14 => {
                if run {
                    return Err(PrcError::Unsupported("a schema pointer field"));
                }
            }
            15 => {
                let count = self.expr(prog, pos, run, depth + 1)?;
                self.repeat(prog, pos, count, run, depth)?;
            }
            16 => {
                // prc-rs: reads SimpleFor's count as an Integer; this reader deliberately reads an
                // UnsignedInteger (module doc).
                let count = if run { i64::from(self.r.unsigned_integer()?) } else { 0 };
                self.repeat(prog, pos, count, run, depth)?;
            }
            17 => self.conditional(prog, pos, run, depth)?,
            19 => self.block(prog, pos, run, depth)?,
            20 => {
                let version = next(prog, pos)?;
                let newer = version > crate::PRC_READER_VERSION;
                self.block(prog, pos, run && newer, depth)?;
            }
            22 => {
                let v = next(prog, pos)?;
                if run {
                    self.vars.insert(v, 0);
                }
            }
            23 | 24 => {
                let v = next(prog, pos)?;
                let value = self.expr(prog, pos, run, depth + 1)?;
                if run {
                    self.vars.insert(v, value);
                }
            }
            39 | 40 => {
                next(prog, pos)?;
            }
            t => return Err(malformed(&format!("token {t} where a statement belongs"))),
        }
        Ok(())
    }

    /// Token 6: run the named type's own program, the fields it inherits.
    fn parent_program(&mut self, prog: &[u32], pos: &mut usize, run: bool, depth: u32) -> Result<(), PrcError> {
        let parent = next(prog, pos)?;
        if run && let Some(p) = self.schema.programs.get(&parent) {
            self.tick(depth + 1)?;
            let mut q = 0;
            while q < p.len() {
                self.stmt(p, &mut q, true, depth + 1)?;
            }
        }
        Ok(())
    }

    /// Token 17: `if` with an optional token-18 `else`.
    fn conditional(&mut self, prog: &[u32], pos: &mut usize, run: bool, depth: u32) -> Result<(), PrcError> {
        let cond = self.expr(prog, pos, run, depth + 1)? != 0;
        self.stmt(prog, pos, run && cond, depth + 1)?;
        if prog.get(*pos) == Some(&18) {
            *pos += 1;
            self.stmt(prog, pos, run && !cond, depth + 1)?;
        }
        Ok(())
    }

    /// One expression; with `run` false its value is 0 and nothing is read.
    fn expr(&mut self, prog: &[u32], pos: &mut usize, run: bool, depth: u32) -> Result<i64, PrcError> {
        self.tick(depth)?;
        let t = next(prog, pos)?;
        match t {
            0..=4 => {
                if run {
                    self.read(t)
                } else {
                    Ok(0)
                }
            }
            25 => {
                let v = next(prog, pos)?;
                if !run {
                    return Ok(0);
                }
                self.vars.get(&v).copied().ok_or_else(|| malformed(&format!("variable {v} read before it is set")))
            }
            26 => Ok(i64::from(next(prog, pos)?)),
            27 => Ok(self.loop_counter),
            28 => {
                if run {
                    Err(PrcError::Unsupported("Value_CurveIs3D"))
                } else {
                    Ok(0)
                }
            }
            29..=38 => {
                let a = self.expr(prog, pos, run, depth + 1)?;
                let b = self.expr(prog, pos, run, depth + 1)?;
                if !run {
                    return Ok(0);
                }
                let overflow = || malformed("a schema expression overflows");
                Ok(match t {
                    29 => a.checked_mul(b).ok_or_else(overflow)?,
                    30 => a.checked_div(b).ok_or_else(|| malformed("division by zero"))?,
                    31 => a.checked_add(b).ok_or_else(overflow)?,
                    32 => a.checked_sub(b).ok_or_else(overflow)?,
                    33 => i64::from(a < b),
                    34 => i64::from(a <= b),
                    35 => i64::from(a > b),
                    36 => i64::from(a >= b),
                    37 => i64::from(a == b),
                    _ => i64::from(a != b),
                })
            }
            t => Err(malformed(&format!("token {t} where a value belongs"))),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::testw::W;

    fn schema(entries: &[(u32, &[u32])]) -> W {
        let mut w = W::default();
        w.uint(entries.len() as u32);
        for (t, toks) in entries {
            w.uint(*t).uint(toks.len() as u32);
            for k in *toks {
                w.uint(*k);
            }
        }
        w
    }

    /// Each schema entry and its program's tokens are charged against the reader's element
    /// allowance.
    #[test]
    fn program_tokens_are_charged_against_the_allowance() {
        let bytes = schema(&[(172, &[1, 2, 3])]).bytes();
        assert!(matches!(Schema::read(&mut BitReader::with_elements(&bytes, 3)), Err(PrcError::TooManyElements { .. })));
        assert!(Schema::read(&mut BitReader::with_elements(&bytes, 4)).is_ok());
        let empty = schema(&[(172, &[]), (173, &[]), (174, &[])]).bytes();
        assert!(matches!(Schema::read(&mut BitReader::with_elements(&empty, 2)), Err(PrcError::TooManyElements { .. })));
        assert!(Schema::read(&mut BitReader::with_elements(&empty, 3)).is_ok());
    }

    fn run(entries: &[(u32, &[u32])], data: &W) -> (Result<(), PrcError>, usize) {
        let mut w = schema(entries);
        w.append(data);
        let bytes = w.bytes();
        let mut r = BitReader::new(&bytes);
        let s = Schema::read(&mut r).unwrap();
        let start = r.position();
        let res = s.skip_added_fields(172, &mut r);
        (res, r.position() - start)
    }

    #[test]
    fn integer_string_and_boolean_tokens_consume_their_fields() {
        let mut d = W::default();
        d.bit(true).uint(300).int(-7).string(Some("ab")).put(0x41, 8);
        let want = d.len();
        d.uint(99); // not part of the added fields
        let (res, used) = run(&[(172, &[0, 3, 4, 5, 2])], &d);
        res.unwrap();
        assert_eq!(used, want);
    }

    #[test]
    fn data_tokens_consume_their_fields() {
        let mut d = W::default();
        d.bit(true).double(1.5).uint(300).int(-7).string(Some("ab")).double(2.0).double(3.0);
        let want = d.len();
        d.uint(99); // not part of the added fields
        let (res, used) = run(&[(172, &[0, 1, 3, 4, 5, 7])], &d);
        res.unwrap();
        assert_eq!(used, want);
    }

    #[test]
    fn vector_extent_and_box_tokens_read_their_doubles() {
        // Vec3, Extent1D, Extent2D, BBox: 3 + 2 + 4 + 6 Doubles.
        let mut d = W::default();
        for i in 0..15 {
            d.double(f64::from(i) + 0.5);
        }
        let want = d.len();
        d.uint(99);
        let (res, used) = run(&[(172, &[8, 9, 10, 11])], &d);
        res.unwrap();
        assert_eq!(used, want);
    }

    #[test]
    fn arithmetic_and_comparison_operators() {
        // For (5 - 3) { UInt }, then If (2 < 2) { Bool } Else { Char }.
        let mut d = W::default();
        d.uint(7).uint(8).put(0x41, 8);
        let want = d.len();
        d.uint(99);
        let prog = [15, 32, 26, 5, 26, 3, 3, 17, 33, 26, 2, 26, 2, 0, 18, 2];
        let (res, used) = run(&[(172, &prog)], &d);
        res.unwrap();
        assert_eq!(used, want);
    }

    #[test]
    fn an_absent_type_is_a_no_op() {
        let mut w = schema(&[(300, &[3])]);
        w.uint(5);
        let bytes = w.bytes();
        let mut r = BitReader::new(&bytes);
        let s = Schema::read(&mut r).unwrap();
        let at = r.position();
        s.skip_added_fields(172, &mut r).unwrap();
        assert_eq!(r.position(), at);
        assert!(!s.is_empty());
    }

    #[test]
    fn simple_for_reads_its_count_then_repeats_the_body() {
        let mut d = W::default();
        d.int(3).uint(1).uint(2).uint(3);
        let want = d.len();
        d.uint(77);
        let (res, used) = run(&[(172, &[16, 3])], &d);
        res.unwrap();
        assert_eq!(used, want);

        // A block body.
        let mut d = W::default();
        d.int(2).bit(true).uint(9).bit(false).uint(8);
        let want = d.len();
        let (res, used) = run(&[(172, &[16, 19, 0, 3, 21])], &d);
        res.unwrap();
        assert_eq!(used, want);

        // A count whose top byte is 0x80 or higher, written either way.
        for signed in [false, true] {
            let mut d = W::default();
            if signed {
                d.int(200);
            } else {
                d.uint(200);
            }
            (0..200).for_each(|_| {
                d.bit(true);
            });
            let want = d.len();
            d.uint(77);
            let (res, used) = run(&[(172, &[16, 0])], &d);
            res.unwrap();
            assert_eq!(used, want, "signed={signed}");
        }
    }

    #[test]
    fn if_else_and_variables_choose_the_branch_by_file_data() {
        // DeclareAndSet v0 = <UInt from file>; if v0 > 1 { UInt } else { Character }.
        let prog: &[u32] = &[24, 0, 3, 17, 35, 25, 0, 26, 1, 3, 18, 2];
        let mut d = W::default();
        d.uint(5).uint(4250);
        let want = d.len();
        let (res, used) = run(&[(172, prog)], &d);
        res.unwrap();
        assert_eq!(used, want);

        let mut d = W::default();
        d.uint(0).put(0xAB, 8);
        let want = d.len();
        let (res, used) = run(&[(172, prog)], &d);
        res.unwrap();
        assert_eq!(used, want);
    }

    #[test]
    fn for_counts_by_expression_and_exposes_the_counter() {
        // For (2 * 2) { if counter == 2 { UInt } }  => one UInt read.
        let prog: &[u32] = &[15, 29, 26, 2, 26, 2, 17, 37, 27, 26, 2, 3];
        let mut d = W::default();
        d.uint(1234);
        let want = d.len();
        let (res, used) = run(&[(172, prog)], &d);
        res.unwrap();
        assert_eq!(used, want);
    }

    #[test]
    fn an_expression_that_overflows_is_malformed() {
        // For ((2^31 * 2^31) * 4) { UInt }: 2^64 would wrap to 0 and read nothing.
        let prog: &[u32] = &[15, 29, 29, 26, 1 << 31, 26, 1 << 31, 26, 4, 3];
        let (res, _) = run(&[(172, prog)], &W::default());
        assert!(matches!(res, Err(PrcError::Malformed(m)) if m.contains("overflows")));
    }

    #[test]
    fn version_blocks_run_only_when_newer_than_the_reader() {
        let mut d = W::default();
        d.uint(42);
        let want = d.len();
        let (res, used) = run(&[(172, &[20, 8137, 3, 21, 20, 9000, 3, 21])], &d);
        res.unwrap();
        assert_eq!(used, want);
    }

    #[test]
    fn father_type_runs_the_parent_program_and_obsolete_tokens_swallow_one() {
        let mut d = W::default();
        d.uint(1).uint(2);
        let want = d.len();
        let (res, used) = run(&[(172, &[39, 3, 6, 50, 40, 8]), (50, &[3, 3])], &d);
        res.unwrap();
        assert_eq!(used, want);
    }

    #[test]
    fn hostile_programs_are_refused_not_run_forever() {
        let d = W::default();
        // For (1000000 * 1000000) { empty block }.
        let prog: &[u32] = &[15, 29, 26, 1_000_000, 26, 1_000_000, 19, 21];
        assert!(matches!(run(&[(172, prog)], &d).0, Err(PrcError::Malformed(_))));
        // Father_Type recursing into itself.
        assert!(matches!(run(&[(172, &[6, 172])], &d).0, Err(PrcError::Malformed(_))));
        // A pointer field.
        assert!(matches!(run(&[(172, &[13])], &d).0, Err(PrcError::Unsupported(_))));
        // A stray Block_End, an unterminated block, a division by zero.
        assert!(matches!(run(&[(172, &[21])], &d).0, Err(PrcError::Malformed(_))));
        assert!(matches!(run(&[(172, &[19, 0])], &d).0, Err(PrcError::Malformed(_))));
        assert!(matches!(run(&[(172, &[23, 0, 30, 26, 1, 26, 0])], &d).0, Err(PrcError::Malformed(_))));
    }

    /// Every program run on one reader draws on one step allowance.
    #[test]
    fn programs_on_one_stream_share_the_step_allowance() {
        // For (2^19) { empty block }: about 2^20 steps, so the 2^22 allowance fits three runs.
        let bytes = schema(&[(172, &[15, 26, 1 << 19, 19, 21])]).bytes();
        let mut r = BitReader::new(&bytes);
        let s = Schema::read(&mut r).unwrap();
        let runs = (0..5).take_while(|_| s.skip_added_fields(172, &mut r).is_ok()).count();
        assert_eq!(runs, 3);
        assert!(matches!(s.skip_added_fields(172, &mut r), Err(PrcError::Malformed(m)) if m.contains("step ceiling")));
    }

    #[test]
    fn a_duplicated_entity_type_is_malformed() {
        let bytes = schema(&[(172, &[3]), (172, &[3])]).bytes();
        assert!(matches!(Schema::read(&mut BitReader::new(&bytes)), Err(PrcError::Malformed(_))));
    }

    #[test]
    fn an_entry_count_past_the_data_is_refused_before_any_entry_is_read() {
        let mut w = W::default();
        w.uint(1_000).uint(172).uint(0);
        let bytes = w.bytes();
        assert_eq!(Schema::read(&mut BitReader::new(&bytes)).unwrap_err(), PrcError::Truncated("the schema"));
    }
}
