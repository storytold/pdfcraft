//! Implements helper functions for the built-in `AclEntry` format.
//! These are used when `serde` is available

use serde::de::{self, IntoDeserializer, Visitor};
use serde::{Deserialize, Serialize, ser};
use std::fmt;
use std::io;

use crate::aclentry::AclEntryKind;
use crate::flag::FlagName;
use crate::perm::PermName;

/// Write value of a simple enum as a `serde` serialized string.
#[allow(clippy::unnecessary_wraps)]
fn write_enum<T: Serialize>(f: &mut fmt::Formatter, value: T) -> fmt::Result {
    let mut serializer = EnumSerializer(f);
    value
        .serialize(&mut serializer)
        .expect("can't serialize value");
    Ok(())
}

// Read value of a simple enum using a stub `serde` deserializer.
fn read_enum<'a, T>(s: &'a str) -> Result<T>
where
    T: Deserialize<'a>,
{
    let mut deserializer = EnumDeserializer(s);
    T::deserialize(&mut deserializer)
}

/// Write value of an `AclEntryKind`.
pub fn write_aclentrykind(f: &mut fmt::Formatter, value: AclEntryKind) -> fmt::Result {
    write_enum(f, value)
}

// Read value of an `AclEntryKind`.
pub fn read_aclentrykind(s: &str) -> Result<AclEntryKind> {
    read_enum(s)
}

/// Write value of a `FlagName`.
pub fn write_flagname(f: &mut fmt::Formatter, value: FlagName) -> fmt::Result {
    write_enum(f, value)
}

// Read value of a `FlagName`.
pub fn read_flagname(s: &str) -> Result<FlagName> {
    read_enum(s)
}

/// Write value of a `PermName`.
pub fn write_permname(f: &mut fmt::Formatter, value: PermName) -> fmt::Result {
    write_enum(f, value)
}

// Read value of a `PermName`.
pub fn read_permname(s: &str) -> Result<PermName> {
    read_enum(s)
}

////////////////////////////////////////////////////////////////////////////////

// This is a simple serializer class for enums.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Message(String),
    NotImplemented,
}

impl ser::Error for Error {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Error::Message(msg.to_string())
    }
}

impl de::Error for Error {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Error::Message(msg.to_string())
    }
}

impl From<Error> for io::Error {
    fn from(err: Error) -> Self {
        io::Error::new(io::ErrorKind::InvalidInput, err)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Message(msg) => write!(f, "{msg}"),
            Error::NotImplemented => write!(f, "not implemented"),
        }
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

////////////////////////////////////////////////////////////////////////////////

struct EnumSerializer<'a, 'b>(&'a mut fmt::Formatter<'b>);

const fn not_implemented<T>() -> Result<T> {
    Err(Error::NotImplemented)
}

impl ser::Serializer for &mut EnumSerializer<'_, '_> {
    type Ok = ();
    type Error = Error;

    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    // Serialize a simple enum.
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<()> {
        self.0.write_str(variant).expect("can't format enum");
        Ok(())
    }

    fn serialize_bool(self, _v: bool) -> Result<()> {
        not_implemented()
    }

    fn serialize_i8(self, _v: i8) -> Result<()> {
        not_implemented()
    }

    fn serialize_i16(self, _v: i16) -> Result<()> {
        not_implemented()
    }

    fn serialize_i32(self, _v: i32) -> Result<()> {
        not_implemented()
    }

    fn serialize_i64(self, _v: i64) -> Result<()> {
        not_implemented()
    }

    fn serialize_u8(self, _v: u8) -> Result<()> {
        not_implemented()
    }

    fn serialize_u16(self, _v: u16) -> Result<()> {
        not_implemented()
    }

    fn serialize_u32(self, _v: u32) -> Result<()> {
        not_implemented()
    }

    fn serialize_u64(self, _v: u64) -> Result<()> {
        not_implemented()
    }

    fn serialize_f32(self, _v: f32) -> Result<()> {
        not_implemented()
    }

    fn serialize_f64(self, _v: f64) -> Result<()> {
        not_implemented()
    }

    fn serialize_char(self, _v: char) -> Result<()> {
        not_implemented()
    }

    fn serialize_str(self, _v: &str) -> Result<()> {
        not_implemented()
    }

    fn serialize_bytes(self, _v: &[u8]) -> Result<()> {
        not_implemented()
    }

    fn serialize_none(self) -> Result<()> {
        not_implemented()
    }

    fn serialize_some<T>(self, _v: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        not_implemented()
    }

    fn serialize_unit(self) -> Result<()> {
        not_implemented()
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<()> {
        not_implemented()
    }

    fn serialize_newtype_struct<T>(self, _name: &'static str, _value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        not_implemented()
    }

    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        not_implemented()
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq> {
        not_implemented()
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple> {
        not_implemented()
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        not_implemented()
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        not_implemented()
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap> {
        not_implemented()
    }

    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeStruct> {
        not_implemented()
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant> {
        not_implemented()
    }
}

impl ser::SerializeSeq for &mut EnumSerializer<'_, '_> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T>(&mut self, _value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        not_implemented()
    }

    fn end(self) -> Result<()> {
        not_implemented()
    }
}

impl ser::SerializeTuple for &mut EnumSerializer<'_, '_> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T>(&mut self, _value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        not_implemented()
    }

    fn end(self) -> Result<()> {
        not_implemented()
    }
}

impl ser::SerializeTupleStruct for &mut EnumSerializer<'_, '_> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T>(&mut self, _value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        not_implemented()
    }

    fn end(self) -> Result<()> {
        not_implemented()
    }
}

impl ser::SerializeTupleVariant for &mut EnumSerializer<'_, '_> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T>(&mut self, _value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        not_implemented()
    }

    fn end(self) -> Result<()> {
        not_implemented()
    }
}

impl ser::SerializeMap for &mut EnumSerializer<'_, '_> {
    type Ok = ();
    type Error = Error;

    fn serialize_key<T>(&mut self, _key: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        not_implemented()
    }

    fn serialize_value<T>(&mut self, _value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        not_implemented()
    }

    fn end(self) -> Result<()> {
        not_implemented()
    }
}

impl ser::SerializeStruct for &mut EnumSerializer<'_, '_> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T>(&mut self, _key: &'static str, _value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        not_implemented()
    }

    fn end(self) -> Result<()> {
        not_implemented()
    }
}

impl ser::SerializeStructVariant for &mut EnumSerializer<'_, '_> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T>(&mut self, _key: &'static str, _value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        not_implemented()
    }

    fn end(self) -> Result<()> {
        not_implemented()
    }
}

////////////////////////////////////////////////////////////////////////////////

// Use this `not_impl!` macro to test that a result is NOT IMPLEMENTED.
#[cfg(test)]
macro_rules! not_impl {
    ($ret:expr) => {
        assert_eq!($ret.err().unwrap(), Error::NotImplemented)
    };
}

#[cfg(test)]
mod serialize_tests {
    use super::*;

    #[test]
    fn test_error() {
        let err = Error::NotImplemented;
        assert_eq!(format!("{err}"), "not implemented");

        let result = <Error as ser::Error>::custom("foo");
        assert_eq!(format!("{result}"), "foo");
    }

    #[test]
    fn test_enum() {
        #[derive(Serialize)]
        enum E {
            Unit,
        }

        impl fmt::Display for E {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write_enum(f, self)
            }
        }

        let u = E::Unit;
        let expected = "Unit";
        assert_eq!(format!("{u}"), expected);
    }

    #[test]
    fn test_enum_serializer_unimplemented_methods() {
        struct Test;

        fn seq_not_impl(mut es: &mut EnumSerializer) {
            use ser::SerializeSeq;
            not_impl!(es.serialize_element(&()));
            not_impl!(es.end());
        }

        fn tuple_not_impl(mut es: &mut EnumSerializer) {
            use ser::SerializeTuple;
            not_impl!(es.serialize_element(&()));
            not_impl!(es.end());
        }

        fn tuple_struct_not_impl(mut es: &mut EnumSerializer) {
            use ser::SerializeTupleStruct;
            not_impl!(es.serialize_field(&()));
            not_impl!(es.end());
        }

        fn tuple_variant_not_impl(mut es: &mut EnumSerializer) {
            use ser::SerializeTupleVariant;
            not_impl!(es.serialize_field(&()));
            not_impl!(es.end());
        }
        fn map_not_impl(mut es: &mut EnumSerializer) {
            use ser::SerializeMap;
            not_impl!(es.serialize_key(""));
            not_impl!(es.serialize_value(&()));
            not_impl!(es.end());
        }

        fn struct_not_impl(mut es: &mut EnumSerializer) {
            use ser::SerializeStruct;
            not_impl!(es.serialize_field("", &()));
            not_impl!(es.end());
        }

        fn struct_variant_not_impl(mut es: &mut EnumSerializer) {
            use ser::SerializeStructVariant;
            not_impl!(es.serialize_field("", &()));
            not_impl!(es.end());
        }

        impl fmt::Debug for Test {
            // Implement Debug for an empty "Test" class just to get a quick
            // `fmt::Formatter`. To test `EnumSerializer`, it needs to be
            // constructed using a dummy `fmt::Formatter` argument.

            #[allow(clippy::cognitive_complexity)]
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                use ser::Serializer;

                let mut es = EnumSerializer(f);
                not_impl!(es.serialize_unit());
                not_impl!(es.serialize_bool(true));
                not_impl!(es.serialize_char('x'));
                not_impl!(es.serialize_str(""));
                not_impl!(es.serialize_bytes(&[]));
                not_impl!(es.serialize_u8(0));
                not_impl!(es.serialize_u16(0));
                not_impl!(es.serialize_u32(0));
                not_impl!(es.serialize_u64(0));
                not_impl!(es.serialize_i8(0));
                not_impl!(es.serialize_i16(0));
                not_impl!(es.serialize_i32(0));
                not_impl!(es.serialize_i64(0));
                not_impl!(es.serialize_f32(0.0));
                not_impl!(es.serialize_f64(0.0));
                not_impl!(es.serialize_none());
                not_impl!(es.serialize_some(&0));
                not_impl!(es.serialize_unit_struct(""));
                not_impl!(es.serialize_newtype_struct("", &0));
                not_impl!(es.serialize_newtype_variant("", 0, "", &0));
                not_impl!(es.serialize_seq(None));
                not_impl!(es.serialize_tuple(0));
                not_impl!(es.serialize_tuple_struct("", 0));
                not_impl!(es.serialize_tuple_variant("", 0, "", 0));
                not_impl!(es.serialize_map(None));
                not_impl!(es.serialize_struct("", 0));
                not_impl!(es.serialize_struct_variant("", 0, "", 0));
                seq_not_impl(&mut es);
                tuple_not_impl(&mut es);
                tuple_struct_not_impl(&mut es);
                tuple_variant_not_impl(&mut es);
                map_not_impl(&mut es);
                struct_not_impl(&mut es);
                struct_variant_not_impl(&mut es);

                f.write_str("Done.")
            }
        }

        // Run the tests above...
        assert_eq!(format!("{:?}", Test {}), "Done.");
    }
}

////////////////////////////////////////////////////////////////////////////////

struct EnumDeserializer<'de>(&'de str);

impl<'de> de::Deserializer<'de> for &mut EnumDeserializer<'de> {
    type Error = Error;

    fn deserialize_any<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_bool<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_i8<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_i16<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_i32<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_i64<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_u8<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_u16<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_u32<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_u64<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_f32<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_f64<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_char<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_str<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_string<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_bytes<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_byte_buf<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_option<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_unit<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_unit_struct<V>(self, _name: &'static str, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_newtype_struct<V>(self, _name: &'static str, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_seq<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_tuple<V>(self, _len: usize, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        _visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_map<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let value = self.0;
        self.0 = "";
        visitor.visit_enum(value.into_deserializer())
    }

    fn deserialize_identifier<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }

    fn deserialize_ignored_any<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        not_implemented()
    }
}

////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod deserialize_tests {
    use super::*;

    #[test]
    fn test_enum() {
        #[derive(Deserialize, PartialEq, Debug)]
        enum E {
            Unit,
        }

        assert_eq!(E::Unit, read_enum("Unit").unwrap());

        let res: Result<E> = read_enum("Unitx");
        assert_eq!(
            "unknown variant `Unitx`, expected `Unit`",
            res.unwrap_err().to_string()
        );
    }

    #[test]
    fn test_enum_deserializer_unimplemented_methods() {
        use de::Deserializer;

        struct Test;

        impl Visitor<'_> for Test {
            type Value = ();

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "failed")
            }
        }

        let mut ed = EnumDeserializer("");
        not_impl!(ed.deserialize_any(Test));
        not_impl!(ed.deserialize_bool(Test));
        not_impl!(ed.deserialize_byte_buf(Test));
        not_impl!(ed.deserialize_bytes(Test));
        not_impl!(ed.deserialize_char(Test));
        not_impl!(ed.deserialize_f32(Test));
        not_impl!(ed.deserialize_f64(Test));
        not_impl!(ed.deserialize_i16(Test));
        not_impl!(ed.deserialize_i32(Test));
        not_impl!(ed.deserialize_i64(Test));
        not_impl!(ed.deserialize_i8(Test));
        not_impl!(ed.deserialize_identifier(Test));
        not_impl!(ed.deserialize_ignored_any(Test));
        not_impl!(ed.deserialize_map(Test));
        not_impl!(ed.deserialize_newtype_struct("", Test));
        not_impl!(ed.deserialize_option(Test));
        not_impl!(ed.deserialize_seq(Test));
        not_impl!(ed.deserialize_str(Test));
        not_impl!(ed.deserialize_string(Test));
        not_impl!(ed.deserialize_struct("", &[], Test));
        not_impl!(ed.deserialize_tuple(0, Test));
        not_impl!(ed.deserialize_tuple_struct("", 0, Test));
        not_impl!(ed.deserialize_u16(Test));
        not_impl!(ed.deserialize_u32(Test));
        not_impl!(ed.deserialize_u64(Test));
        not_impl!(ed.deserialize_u8(Test));
        not_impl!(ed.deserialize_unit(Test));
        not_impl!(ed.deserialize_unit_struct("", Test));
    }
}
