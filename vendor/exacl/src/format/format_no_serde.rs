//! Implements helper functions for the built-in `AclEntry` format.
//! These are used when `serde` is not available

use std::fmt;
use std::io;

use crate::aclentry::AclEntryKind;
use crate::flag::FlagName;
use crate::perm::PermName;

const ACLENTRYKINDS: &'static [(AclEntryKind, &'static str)] = &[
    (AclEntryKind::User, "user"),
    (AclEntryKind::Group, "group"),
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    (AclEntryKind::Mask, "mask"),
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    (AclEntryKind::Other, "other"),
    #[cfg(target_os = "freebsd")]
    (AclEntryKind::Everyone, "everyone"),
    (AclEntryKind::Unknown, "unknown"),
];

const FLAGS: &'static [(FlagName, &'static str)] = &[
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (FlagName::inherited, "inherited"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (FlagName::file_inherit, "file_inherit"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (FlagName::directory_inherit, "directory_inherit"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (FlagName::limit_inherit, "limit_inherit"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (FlagName::only_inherit, "only_inherit"),
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    (FlagName::default, "default"),
];

const PERMS: &'static [(PermName, &'static str)] = &[
    (PermName::read, "read"),
    (PermName::write, "write"),
    (PermName::execute, "execute"),
    #[cfg(target_os = "freebsd")]
    (PermName::read_data, "read_data"),
    #[cfg(target_os = "freebsd")]
    (PermName::write_data, "write_data"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (PermName::delete, "delete"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (PermName::append, "append"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (PermName::delete_child, "delete_child"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (PermName::readattr, "readattr"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (PermName::writeattr, "writeattr"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (PermName::readextattr, "readextattr"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (PermName::writeextattr, "writeextattr"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (PermName::readsecurity, "readsecurity"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (PermName::writesecurity, "writesecurity"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (PermName::chown, "chown"),
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    (PermName::sync, "sync"),
];

/// Write value of an enum as a string using the given (enum, str) table.
fn write_enum<T: PartialEq>(
    f: &mut fmt::Formatter,
    value: T,
    table: &'static [(T, &'static str)],
) -> fmt::Result {
    match table.iter().find(|item| item.0 == value) {
        Some((_, name)) => write!(f, "{name}"),
        None => write!(f, "!!"),
    }
}

/// Read value of an enum from a string using the given (enum, str) table.
fn read_enum<T: Copy>(s: &str, table: &'static [(T, &'static str)]) -> Result<T> {
    match table.iter().find(|item| item.1 == s) {
        Some((value, _)) => Ok(*value),
        None => Err(err_enum(s, table)),
    }
}

/// Produce the error message when the variant can't be found.
fn err_enum<T>(s: &str, table: &'static [(T, &'static str)]) -> Error {
    let variants = table
        .iter()
        .map(|item| format!("`{}`", item.1))
        .collect::<Vec<String>>()
        .join(", ");

    let msg = if table.len() == 1 {
        format!("unknown variant `{s}`, expected {variants}")
    } else {
        format!("unknown variant `{s}`, expected one of {variants}")
    };

    Error::Message(msg)
}

/// Write value of an AclEntryKind.
pub fn write_aclentrykind(f: &mut fmt::Formatter, value: AclEntryKind) -> fmt::Result {
    write_enum(f, value, ACLENTRYKINDS)
}

// Read value of an AclEntryKind.
pub fn read_aclentrykind(s: &str) -> Result<AclEntryKind> {
    read_enum(s, ACLENTRYKINDS)
}

/// Write value of a FlagName.
pub fn write_flagname(f: &mut fmt::Formatter, value: FlagName) -> fmt::Result {
    write_enum(f, value, FLAGS)
}

// Read value of a FlagName.
pub fn read_flagname(s: &str) -> Result<FlagName> {
    read_enum(s, FLAGS)
}

/// Write value of a PermName.
pub fn write_permname(f: &mut fmt::Formatter, value: PermName) -> fmt::Result {
    write_enum(f, value, PERMS)
}

// Read value of a PermName.
pub fn read_permname(s: &str) -> Result<PermName> {
    read_enum(s, PERMS)
}

////////////////////////////////////////////////////////////////////////////////

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Message(String),
    NotImplemented,
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
            Error::NotImplemented => write!(f, "Not implemented"),
        }
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;
