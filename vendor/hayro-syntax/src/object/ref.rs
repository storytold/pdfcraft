//! Object references.

use crate::object::ObjectIdentifier;
use crate::object::ObjectLike;
use crate::reader::Reader;
use crate::reader::{Readable, ReaderContext, ReaderExt, Skippable};
use core::fmt::{Debug, Formatter};

/// A reference to an object.
#[derive(PartialEq, Eq, Debug, Clone, Copy, Hash)]
pub struct ObjRef {
    /// The object number.
    pub obj_number: i32,
    /// The generation number.
    pub gen_number: i32,
}

impl ObjRef {
    /// Create a new object reference.
    pub fn new(obj_number: i32, gen_number: i32) -> Self {
        Self {
            obj_number,
            gen_number,
        }
    }
}

impl From<ObjRef> for ObjectIdentifier {
    fn from(value: ObjRef) -> Self {
        Self::new(value.obj_number, value.gen_number)
    }
}

impl From<ObjectIdentifier> for ObjRef {
    fn from(value: ObjectIdentifier) -> Self {
        Self::new(value.obj_number, value.gen_number)
    }
}

impl Skippable for ObjRef {
    fn skip(r: &mut Reader<'_>, _: bool) -> Option<()> {
        // PdfCraft patch: (hayro-syntax (33)) a comment counts as white space, here too.
        r.skip::<i32>(false)?;
        r.skip_white_spaces_and_comments();
        // PdfCraft patch (48): the generation is stepped over as `read_generation` reads it.
        read_generation(r)?;
        r.skip_white_spaces_and_comments();
        r.forward_tag(b"R")?;

        Some(())
    }
}

impl Readable<'_> for ObjRef {
    fn read(r: &mut Reader<'_>, _: &ReaderContext<'_>) -> Option<Self> {
        let obj_ref = r.read_without_context::<i32>()?;
        r.skip_white_spaces_and_comments();
        // PdfCraft patch (48): see `read_generation`.
        let gen_num = read_generation(r)?;
        r.skip_white_spaces_and_comments();
        r.forward_tag(b"R")?;

        Some(Self::new(obj_ref, gen_num))
    }
}

/// PdfCraft patch (48): the generation number of a reference `N G R`, with one that does not fit
/// an `i32` read as 0.
///
/// A generation number is at most 65,535 (ISO 32000-2 §7.5.4), so `58 4285904753 R` is
/// malformed. Skipping steps over any run of digits as a number, though, so the first pass of an
/// array or dictionary took it for a reference, and reading it as one then failed: the item was
/// lost, or read as the three items `58`, `4285904753` and `R`. A page's `/Contents` array
/// written that way lost the streams it names, and with them whatever they drew
/// (`GHOSTSCRIPT-691345-0.pdf`, attached to PDFBOX-5456: the black boxes a later update draws
/// over a fax cover's text). No object can carry such a generation, so the object number is
/// what identifies the object: generation 0 is the key a rebuilt table files such a header under
/// (see `read_object_header` in `xref.rs`, #5580), and the generation of almost every object.
///
/// Only the digits skipping already accepts are read this way, so reading and skipping agree.
fn read_generation(r: &mut Reader<'_>) -> Option<i32> {
    let mut strict = r.clone();
    if let Some(generation) = strict.read_without_context::<i32>() {
        *r = strict;
        return Some(generation);
    }
    let mut lenient = r.clone();
    lenient.skip::<i32>(false)?;
    *r = lenient;
    Some(0)
}

/// A struct that is either an object or a reference to an object.
#[derive(PartialEq, Eq)]
pub enum MaybeRef<T> {
    /// A reference to an object.
    Ref(ObjRef),
    /// An object.
    NotRef(T),
}

impl<T> MaybeRef<T> {
    /// If the object is an object reference, return it.
    pub fn as_obj_ref(&self) -> Option<ObjRef> {
        match self {
            Self::Ref(r) => Some(*r),
            Self::NotRef(_) => None,
        }
    }
}

#[allow(private_bounds)]
impl<'a, T> MaybeRef<T>
where
    T: ObjectLike<'a>,
{
    /// Resolve the `MaybeRef` object with the given xref table.
    pub(crate) fn resolve(self, ctx: &ReaderContext<'a>) -> Option<T> {
        match self {
            Self::Ref(r) => ctx.xref().get_with::<T>(r.into(), ctx),
            Self::NotRef(t) => Some(t),
        }
    }
}

impl<T> TryFrom<MaybeRef<T>> for ObjRef {
    type Error = ();

    fn try_from(value: MaybeRef<T>) -> Result<Self, Self::Error> {
        match value {
            MaybeRef::Ref(r) => Ok(r),
            MaybeRef::NotRef(_) => Err(()),
        }
    }
}

impl<T> Debug for MaybeRef<T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Ref(r) => write!(f, "{r:?}"),
            Self::NotRef(nr) => write!(f, "{nr:?}"),
        }
    }
}

impl<T> Skippable for MaybeRef<T>
where
    T: Skippable,
{
    fn skip(r: &mut Reader<'_>, is_content_stream: bool) -> Option<()> {
        r.skip::<ObjRef>(is_content_stream)
            .or_else(|| r.skip::<T>(is_content_stream))
            .map(|_| {})
    }
}

impl<'a, T> Readable<'a> for MaybeRef<T>
where
    T: Readable<'a>,
{
    fn read(r: &mut Reader<'a>, ctx: &ReaderContext<'a>) -> Option<Self> {
        if let Some(obj) = r.read::<ObjRef>(ctx) {
            Some(Self::Ref(obj))
        } else {
            Some(Self::NotRef(r.read::<T>(ctx)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::object::ObjRef;
    use crate::reader::Reader;
    use crate::reader::ReaderExt;

    /// PdfCraft patch (48): a generation past 32 bits reads as 0, where skipping already took the
    /// whole reference; one that is not an integer is still no reference.
    #[test]
    fn a_generation_past_32_bits_reads_as_0() {
        for text in ["58 4285904753 R", "58 18446744073709551616 R", "58 4285904753\n%c\nR"] {
            let mut read = Reader::new(text.as_bytes());
            assert_eq!(read.read_without_context::<ObjRef>(), Some(ObjRef::new(58, 0)), "{text:?}");
            let mut skipped = Reader::new(text.as_bytes());
            assert!(skipped.skip::<ObjRef>(false).is_some(), "{text:?}");
            assert_eq!(read.offset(), skipped.offset(), "{text:?}: read and skip end in the same place");
        }
        for text in ["58 4285904753.5 R", "58 4285904753x R", "58 4285904753"] {
            assert_eq!(Reader::new(text.as_bytes()).read_without_context::<ObjRef>(), None, "{text:?}");
            assert!(Reader::new(text.as_bytes()).skip::<ObjRef>(false).is_none(), "{text:?}");
        }
        assert_eq!(Reader::new(b"58 65535 R").read_without_context::<ObjRef>(), Some(ObjRef::new(58, 65535)));
    }

    #[test]
    fn ref_1() {
        assert_eq!(
            Reader::new("34 1 R".as_bytes())
                .read_without_context::<ObjRef>()
                .unwrap(),
            ObjRef::new(34, 1)
        );
    }

    #[test]
    fn ref_trailing() {
        assert_eq!(
            Reader::new("256 0 R (hi)".as_bytes())
                .read_without_context::<ObjRef>()
                .unwrap(),
            ObjRef::new(256, 0)
        );
    }

    #[test]
    fn ref_invalid_1() {
        assert!(
            Reader::new("256 R".as_bytes())
                .read_without_context::<ObjRef>()
                .is_none()
        );
    }

    #[test]
    fn ref_invalid_2() {
        assert!(
            Reader::new("256 257".as_bytes())
                .read_without_context::<ObjRef>()
                .is_none()
        );
    }
}
