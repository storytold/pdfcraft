//! Implements the permissions flags.

use crate::bititer::{BitIter, BitIterable};
use crate::format;
use crate::sys::*;

use bitflags::bitflags;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize, de, ser};
use std::fmt;

bitflags! {
    /// Represents file access permissions.
    #[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy, Default)]
    pub struct Perm : acl_perm_t {
        /// READ_DATA permission for a file.
        /// Same as LIST_DIRECTORY permission for a directory.
        const READ = ACL_READ;

        /// WRITE_DATA permission for a file.
        /// Same as ADD_FILE permission for a directory.
        const WRITE = ACL_WRITE;

        /// EXECUTE permission for a file.
        /// Same as SEARCH permission for a directory.
        const EXECUTE = ACL_EXECUTE;

        /// DELETE permission for a file.
        #[cfg(any(docsrs, target_os = "macos", target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(any(target_os = "macos", target_os = "freebsd"))))]
        const DELETE = np::ACL_DELETE;

        /// APPEND_DATA permission for a file.
        /// Same as ADD_SUBDIRECTORY permission for a directory.
        #[cfg(any(docsrs, target_os = "macos", target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(any(target_os = "macos", target_os = "freebsd"))))]
        const APPEND = np::ACL_APPEND_DATA;

        /// DELETE_CHILD permission for a directory.
        #[cfg(any(docsrs, target_os = "macos", target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(any(target_os = "macos", target_os = "freebsd"))))]
        const DELETE_CHILD = np::ACL_DELETE_CHILD;

        /// READ_ATTRIBUTES permission for file or directory.
        #[cfg(any(docsrs, target_os = "macos", target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(any(target_os = "macos", target_os = "freebsd"))))]
        const READATTR = np::ACL_READ_ATTRIBUTES;

        /// WRITE_ATTRIBUTES permission for a file or directory.
        #[cfg(any(docsrs, target_os = "macos", target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(any(target_os = "macos", target_os = "freebsd"))))]
        const WRITEATTR = np::ACL_WRITE_ATTRIBUTES;

        /// READ_EXTATTRIBUTES permission for a file or directory.
        #[cfg(any(docsrs, target_os = "macos", target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(any(target_os = "macos", target_os = "freebsd"))))]
        const READEXTATTR = np::ACL_READ_EXTATTRIBUTES;

        /// WRITE_EXTATTRIBUTES permission for a file or directory.
        #[cfg(any(docsrs, target_os = "macos", target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(any(target_os = "macos", target_os = "freebsd"))))]
        const WRITEEXTATTR = np::ACL_WRITE_EXTATTRIBUTES;

        /// READ_SECURITY permission for a file or directory.
        #[cfg(any(docsrs, target_os = "macos", target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(any(target_os = "macos", target_os = "freebsd"))))]
        const READSECURITY = np::ACL_READ_SECURITY;

        /// WRITE_SECURITY permission for a file or directory.
        #[cfg(any(docsrs, target_os = "macos", target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(any(target_os = "macos", target_os = "freebsd"))))]
        const WRITESECURITY = np::ACL_WRITE_SECURITY;

        /// CHANGE_OWNER permission for a file or directory.
        #[cfg(any(docsrs, target_os = "macos", target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(any(target_os = "macos", target_os = "freebsd"))))]
        const CHOWN = np::ACL_CHANGE_OWNER;

        /// SYNCHRONIZE permission (unsupported).
        #[cfg(any(docsrs, target_os = "macos", target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(any(target_os = "macos", target_os = "freebsd"))))]
        const SYNC = np::ACL_SYNCHRONIZE;

        /// NFSv4 READ_DATA permission.
        #[cfg(any(docsrs, target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(target_os = "freebsd")))]
        const READ_DATA = np::ACL_READ_DATA;

        /// NFSv4 WRITE_DATA permission.
        #[cfg(any(docsrs, target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(target_os = "freebsd")))]
        const WRITE_DATA = np::ACL_WRITE_DATA;

        /// Posix specific permissions.
        #[cfg(any(docsrs, target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(target_os = "freebsd")))]
        const POSIX_SPECIFIC = Self::READ.bits() | Self::WRITE.bits() | Self::EXECUTE.bits();

        /// All NFSv4 specific permissions.
        #[cfg(any(docsrs, target_os = "freebsd"))]
        #[cfg_attr(docsrs, doc(cfg(target_os = "freebsd")))]
        const NFS4_SPECIFIC = Self::READ_DATA.bits() | Self::WRITE_DATA.bits()
            | Self::DELETE.bits() | Self::APPEND.bits() | Self::DELETE_CHILD.bits()
            | Self::READATTR.bits() | Self::WRITEATTR.bits() | Self::READEXTATTR.bits()
            | Self::WRITEEXTATTR.bits() | Self::READSECURITY.bits()
            | Self::WRITESECURITY.bits() | Self::CHOWN.bits() | Self::SYNC.bits();
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
type RevPermIter = std::iter::Rev<BitIter<Perm>>;

// `perm_iter` returns a platform-specific iterator over the permission bits.
impl Perm {
    #[cfg(target_os = "macos")]
    fn perm_iter(self) -> BitIter<Perm> {
        BitIter(self & Perm::all())
    }

    #[cfg(target_os = "linux")]
    fn perm_iter(self) -> RevPermIter {
        BitIter(self & Perm::all()).rev()
    }

    #[cfg(target_os = "freebsd")]
    fn perm_iter(self) -> std::iter::Chain<RevPermIter, BitIter<Perm>> {
        BitIter(self & Perm::POSIX_SPECIFIC)
            .rev()
            .chain(BitIter(self & Perm::NFS4_SPECIFIC))
    }
}

impl BitIterable for Perm {
    fn lsb(self) -> Option<Self> {
        if self.is_empty() {
            return None;
        }
        let low_bit = 1u32 << self.bits().trailing_zeros();
        Some(Perm::from_bits_retain(low_bit))
    }

    fn msb(self) -> Option<Self> {
        #[allow(clippy::cast_possible_truncation)]
        const MAX_BITS: acl_perm_t = 8 * std::mem::size_of::<Perm>() as acl_perm_t - 1;

        if self.is_empty() {
            return None;
        }
        let high_bit = 1u32 << (MAX_BITS - self.bits().leading_zeros());
        Some(Perm::from_bits_retain(high_bit))
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[repr(u32)]
#[allow(non_camel_case_types)]
pub enum PermName {
    // *N.B.* Update the corresponding table in format/format_no_serde.rs
    // if any of these entries change.
    read = Perm::READ.bits(),

    write = Perm::WRITE.bits(),

    execute = Perm::EXECUTE.bits(),

    #[cfg(target_os = "freebsd")]
    read_data = Perm::READ_DATA.bits(),

    #[cfg(target_os = "freebsd")]
    write_data = Perm::WRITE_DATA.bits(),

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    delete = Perm::DELETE.bits(),

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    append = Perm::APPEND.bits(),

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    delete_child = Perm::DELETE_CHILD.bits(),

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    readattr = Perm::READATTR.bits(),

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    writeattr = Perm::WRITEATTR.bits(),

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    readextattr = Perm::READEXTATTR.bits(),

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    writeextattr = Perm::WRITEEXTATTR.bits(),

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    readsecurity = Perm::READSECURITY.bits(),

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    writesecurity = Perm::WRITESECURITY.bits(),

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    chown = Perm::CHOWN.bits(),

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    sync = Perm::SYNC.bits(),
}

impl PermName {
    const fn from_perm(perm: Perm) -> Option<PermName> {
        match perm {
            Perm::READ => Some(PermName::read),

            Perm::WRITE => Some(PermName::write),

            Perm::EXECUTE => Some(PermName::execute),

            #[cfg(target_os = "freebsd")]
            Perm::READ_DATA => Some(PermName::read_data),

            #[cfg(target_os = "freebsd")]
            Perm::WRITE_DATA => Some(PermName::write_data),

            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            Perm::DELETE => Some(PermName::delete),

            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            Perm::APPEND => Some(PermName::append),

            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            Perm::DELETE_CHILD => Some(PermName::delete_child),

            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            Perm::READATTR => Some(PermName::readattr),

            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            Perm::WRITEATTR => Some(PermName::writeattr),

            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            Perm::READEXTATTR => Some(PermName::readextattr),

            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            Perm::WRITEEXTATTR => Some(PermName::writeextattr),

            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            Perm::READSECURITY => Some(PermName::readsecurity),

            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            Perm::WRITESECURITY => Some(PermName::writesecurity),

            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            Perm::CHOWN => Some(PermName::chown),

            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            Perm::SYNC => Some(PermName::sync),

            _ => None,
        }
    }

    const fn to_perm(self) -> Perm {
        Perm::from_bits_retain(self as u32)
    }
}

impl fmt::Display for PermName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        format::write_permname(f, *self)
    }
}

impl fmt::Display for Perm {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut iter = self.perm_iter();

        if let Some(perm) = iter.next() {
            write!(f, "{}", PermName::from_perm(perm).unwrap())?;

            for perm in iter {
                write!(f, ",{}", PermName::from_perm(perm).unwrap())?;
            }
        }

        Ok(())
    }
}

/// Parse an abbreviated permission, "rwx", "wx", "r-x" etc.
///
/// Order doesn't matter. "xwr" is the same as "rwx". Allow for "r-x" by
/// ignoring any number of '-'. Don't allow r, w, or x to be repeated.
fn parse_perm_abbreviation(s: &str) -> Option<Perm> {
    let mut perms = Perm::empty();
    for ch in s.chars() {
        match ch {
            'r' if !perms.contains(Perm::READ) => perms |= Perm::READ,
            'w' if !perms.contains(Perm::WRITE) => perms |= Perm::WRITE,
            'x' if !perms.contains(Perm::EXECUTE) => perms |= Perm::EXECUTE,
            '-' => (),
            // Any other character is invalid.
            _ => return None,
        }
    }
    Some(perms)
}

impl std::str::FromStr for PermName {
    type Err = format::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        format::read_permname(s)
    }
}

impl std::str::FromStr for Perm {
    type Err = format::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut result = Perm::empty();

        for item in s.split(',') {
            let word = item.trim();
            if !word.is_empty() {
                if let Some(perms) = parse_perm_abbreviation(word) {
                    result |= perms;
                } else {
                    result |= word.parse::<PermName>()?.to_perm();
                }
            }
        }

        Ok(result)
    }
}

#[cfg(feature = "serde")]
impl ser::Serialize for Perm {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        use ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(None)?;

        for perm in self.perm_iter() {
            seq.serialize_element(&PermName::from_perm(perm))?;
        }

        seq.end()
    }
}

#[cfg(feature = "serde")]
impl<'de> de::Deserialize<'de> for Perm {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct PermVisitor;

        impl<'de> de::Visitor<'de> for PermVisitor {
            type Value = Perm;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("list of permissions")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let mut perms: Perm = Perm::empty();

                while let Some(value) = seq.next_element()? {
                    let name: PermName = value;
                    perms |= name.to_perm();
                }

                Ok(perms)
            }
        }

        deserializer.deserialize_seq(PermVisitor)
    }
}

////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod perm_tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn test_perm_equivalences() {
        assert_eq!(acl_perm_t_ACL_READ_DATA, acl_perm_t_ACL_LIST_DIRECTORY);
        assert_eq!(acl_perm_t_ACL_WRITE_DATA, acl_perm_t_ACL_ADD_FILE);
        assert_eq!(acl_perm_t_ACL_EXECUTE, acl_perm_t_ACL_SEARCH);
        assert_eq!(acl_perm_t_ACL_APPEND_DATA, acl_perm_t_ACL_ADD_SUBDIRECTORY);
    }

    #[test]
    fn test_perm_display() {
        assert_eq!(Perm::empty().to_string(), "");

        let perms = Perm::READ | Perm::EXECUTE;
        assert_eq!(perms.to_string(), "read,execute");

        let bad_perm = Perm::from_bits_retain(0x0080_0000) | Perm::READ;
        assert_eq!(bad_perm.to_string(), "read");

        #[cfg(target_os = "macos")]
        assert_eq!(
            Perm::all().to_string(),
            "read,write,execute,delete,append,delete_child,readattr,writeattr,readextattr,writeextattr,readsecurity,writesecurity,chown,sync"
        );

        #[cfg(target_os = "linux")]
        assert_eq!(Perm::all().to_string(), "read,write,execute");

        #[cfg(target_os = "freebsd")]
        assert_eq!(
            Perm::all().to_string(),
            "read,write,execute,read_data,write_data,append,readextattr,writeextattr,delete_child,readattr,writeattr,delete,readsecurity,writesecurity,chown,sync"
        );
    }

    #[test]
    fn test_perm_fromstr() {
        let flags = Perm::READ | Perm::EXECUTE;
        assert_eq!(flags, "read, execute".parse().unwrap());
        assert_eq!(flags, "rx".parse().unwrap());
        assert_eq!(flags, "r-x".parse().unwrap());
        assert_eq!(flags, "--x--r--".parse().unwrap());
        assert_eq!(flags, "xr".parse().unwrap());
        assert_eq!(Perm::WRITE, "w".parse().unwrap());
        assert_eq!(Perm::empty(), "".parse().unwrap());

        // Duplicate abbreviations not supported.
        assert!("rr".parse::<Perm>().is_err());

        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                "unknown variant `q`, expected one of `read`, `write`, `execute`, `delete`, `append`, `delete_child`, `readattr`, `writeattr`, `readextattr`, `writeextattr`, `readsecurity`, `writesecurity`, `chown`, `sync`",
                " ,q ".parse::<Perm>().unwrap_err().to_string()
            );

            assert_eq!(Perm::all(), "read,write,execute,delete,append,delete_child,readattr,writeattr,readextattr,writeextattr,readsecurity,writesecurity,chown,sync".parse().unwrap());
        }

        #[cfg(target_os = "linux")]
        {
            assert_eq!(
                "unknown variant `qq`, expected one of `read`, `write`, `execute`",
                " ,qq ".parse::<Perm>().unwrap_err().to_string()
            );

            assert_eq!(Perm::all(), "read,write,execute".parse().unwrap());
        }

        #[cfg(target_os = "freebsd")]
        {
            assert_eq!(
                "unknown variant `qq`, expected one of `read`, `write`, `execute`, `read_data`, `write_data`, `delete`, `append`, `delete_child`, `readattr`, `writeattr`, `readextattr`, `writeextattr`, `readsecurity`, `writesecurity`, `chown`, `sync`",
                " ,qq ".parse::<Perm>().unwrap_err().to_string()
            );

            assert_eq!(Perm::all(), "read,write,execute,delete,append,delete_child,readattr,writeattr,readextattr,writeextattr,readsecurity,writesecurity,chown,sync,read_data,write_data".parse().unwrap());
        }
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn test_perm_unix_permission() {
        // Test that READ, WRITE, EXECUTE constant correspond to the same bits
        // as the permissions in unix mode.

        assert_eq!(Perm::READ.bits(), 0x04);
        assert_eq!(Perm::WRITE.bits(), 0x02);
        assert_eq!(Perm::EXECUTE.bits(), 0x01);

        assert_eq!(
            Perm::from_bits(0x07),
            Some(Perm::READ | Perm::WRITE | Perm::EXECUTE)
        );
    }

    #[test]
    fn test_perm_comparisons() {
        // Test that Perm has backward-compatible Ord/PartialOrd traits.
        assert_eq!(Perm::READ, Perm::READ);
        assert_ne!(Perm::READ, Perm::WRITE);
        assert!(Perm::EXECUTE >= Perm::EXECUTE);

        #[cfg(target_os = "macos")]
        assert!(Perm::READ < Perm::WRITE);
        #[cfg(target_os = "linux")]
        assert!(Perm::READ > Perm::WRITE);
        #[cfg(target_os = "freebsd")]
        assert!(Perm::READ > Perm::WRITE);
    }
}
