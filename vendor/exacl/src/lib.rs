//! # exacl
//!
//! Manipulate file system access control lists (ACL) on `macOS`, `Linux`, and
//! `FreeBSD`.
//!
//! ## Example
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use exacl::{getfacl, setfacl, AclEntry, Perm};
//!
//! // Get the ACL from "./tmp/foo".
//! let mut acl = getfacl("./tmp/foo", None)?;
//!
//! // Print the contents of the ACL.
//! for entry in &acl {
//!     println!("{entry}");
//! }
//!
//! // Add an ACL entry to the end.
//! acl.push(AclEntry::allow_user("some_user", Perm::READ, None));
//!
//! // Set the ACL for "./tmp/foo".
//! setfacl(&["./tmp/foo"], &acl, None)?;
//!
//! # Ok(()) }
//! ```
//!
//! ## API
//!
//! This module provides two high level functions, [`getfacl`] and [`setfacl`].
//!
//! - [`getfacl`] retrieves the ACL for a file or directory.
//! - [`setfacl`] sets the ACL for files or directories.
//!
//! On Linux and `FreeBSD`, the ACL contains entries for the default ACL, if
//! present.
//!
//! Both [`getfacl`] and [`setfacl`] work with a `Vec<AclEntry>`. The
//! [`AclEntry`] structure contains five fields:
//!
//! - kind : [`AclEntryKind`] - the kind of entry (User, Group, Other, Mask,
//!   or Unknown).
//! - name : [`String`] - name of the principal being given access. You can
//!   use a user/group name, decimal uid/gid, or UUID (on macOS).
//! - perms : [`Perm`] - permission bits for the entry.
//! - flags : [`Flag`] - flags indicating whether an entry is inherited, etc.
//! - allow : [`bool`] - true if entry is allowed; false means deny. Linux only
//!   supports allow=true.

#![warn(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod acl;
mod aclentry;
mod aclfile;
mod bindings;
mod bititer;
mod failx;
mod flag;
mod format;
mod perm;
mod qualifier;
mod sys;
mod unix;
mod util;

// Export AclOption, AclEntry, AclEntryKind, Flag and Perm.
pub use acl::AclOption;
pub use aclentry::{AclEntry, AclEntryKind};
pub use flag::Flag;
pub use perm::Perm;

use acl::Acl;
use aclfile::{AclFile, AclFilePaths};
use failx::custom_err;
use std::io::{self, BufRead};

#[cfg(target_os = "macos")]
use std::os::fd::{AsRawFd, BorrowedFd};

#[cfg(not(target_os = "macos"))]
use failx::fail_custom;

/// Get access control list (ACL) for a file or directory.
///
/// On success, returns a vector of [`AclEntry`] with all access control entries
/// for the specified path. The semantics and permissions of the access control
/// list depend on the underlying platform.
///
/// # macOS
///
/// The ACL only includes the extended entries beyond the normal permission mode
/// of the file. macOS provides several ACL entry flags to specify how entries
/// may be inherited by directory sub-items. If there's no extended ACL for a
/// file, this function may return zero entries.
///
/// If `path` points to a symlink, `getfacl` returns the ACL of the file pointed
/// to by the symlink. Use [`AclOption::SYMLINK_ACL`] to obtain the ACL of a symlink
/// itself.
///
/// [`AclOption::DEFAULT_ACL`] option is not supported on macOS.
///
/// [`AclOption::NATIVE_ID`] option returns the `GeneratedUID` (GUID) on macOS
/// instead of translating to human-readable user/group names.
///
/// # Linux
///
/// The ACL includes entries related to the permission mode of the file. These
/// are marked with empty names ("").
///
/// Both the access ACL and the default ACL are returned in one list, with
/// the default ACL entries indicated by a [`Flag::DEFAULT`] flag.
///
/// If `path` points to a symlink, `getfacl` returns the ACL of the file pointed
/// to by the symlink. [`AclOption::SYMLINK_ACL`] is not supported on Linux.
///
/// [`AclOption::DEFAULT_ACL`] causes `getfacl` to only include entries for the
/// default ACL, if present for a directory path. When called with
/// [`AclOption::DEFAULT_ACL`], `getfacl` may return zero entries.
///
/// [`AclOption::NATIVE_ID`] option returns the numeric UID/GID instead of
/// translating to human-readable user/group names.
///
/// # Example
///
/// ```no_run
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use exacl::getfacl;
///
/// let entries = getfacl("./tmp/foo", None)?;
/// # Ok(()) }
/// ```
///
/// # Errors
///
/// Returns an [`io::Error`] on failure.
///
pub fn getfacl<'a, F, O>(file: F, options: O) -> io::Result<Vec<AclEntry>>
where
    F: Into<AclFile<'a>>,
    O: Into<Option<AclOption>>,
{
    my_getfacl(&file.into(), options.into().unwrap_or_default())
}

/// Copy the native extended ACL between two open file descriptors on macOS.
///
/// The ACL is read from `source` and written to `target` without converting it
/// to [`AclEntry`] values for writing. Before applying it, `check` validates a
/// native-ID entry view of that same captured ACL. This copies ACL data only;
/// it does not copy file contents, mode bits, or extended attributes.
///
/// # Errors
///
/// Returns an [`io::Error`] if the ACL cannot be read from `source` or written
/// to `target`, or if `check` refuses it. A refused ACL is not applied.
// PdfCraft patch: validate and copy one native ACL snapshot between open descriptors.
#[cfg(target_os = "macos")]
pub fn copyfacl_fd(
    source: BorrowedFd<'_>,
    target: BorrowedFd<'_>,
    check: impl FnOnce(&[AclEntry]) -> io::Result<()>,
) -> io::Result<()> {
    let acl = Acl::read_fd(source.as_raw_fd(), AclOption::empty())?;
    check(&acl.entries(true)?)?;
    acl.write_fd(target.as_raw_fd(), AclOption::empty())
}

#[cfg(target_os = "macos")]
fn my_getfacl(file: &AclFile, options: AclOption) -> io::Result<Vec<AclEntry>> {
    let native = options.contains(AclOption::NATIVE_ID);
    file.read(options)?.entries(native)
}

#[cfg(not(target_os = "macos"))]
fn my_getfacl(file: &AclFile, options: AclOption) -> io::Result<Vec<AclEntry>> {
    let native = options.contains(AclOption::NATIVE_ID);

    if options.contains(AclOption::ACCESS_ACL | AclOption::DEFAULT_ACL) {
        fail_custom("ACCESS_ACL and DEFAULT_ACL are mutually exclusive options")
    } else if options.intersects(AclOption::ACCESS_ACL | AclOption::DEFAULT_ACL) {
        file.read(options)?.entries(native)
    } else {
        let acl = file.read(options)?;
        let mut entries = acl.entries(native)?;

        if acl.is_posix() {
            let mut default = file
                .read(options | AclOption::DEFAULT_ACL | AclOption::IGNORE_EXPECTED_FILE_ERR)?
                .entries(native)?;

            entries.append(&mut default);
        }
        Ok(entries)
    }
}

/// Set access control list (ACL) for specified files and directories.
///
/// Sets the ACL for the specified paths using the given access control entries.
/// The semantics and permissions of the access control list depend on the
/// underlying platform.
///
/// # macOS
///
/// The ACL contains extended entries beyond the usual mode permission bits.
/// An entry may allow or deny access to a specific user or group.
/// To specify inherited entries, use the provided [Flag] values.
///
/// ### macOS Example
///
/// ```ignore
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use exacl::{setfacl, AclEntry, Flag, Perm};
///
/// let entries = vec![
///     AclEntry::allow_user("some_user", Perm::READ | Perm::WRITE, None),
///     AclEntry::deny_group("some_group", Perm::WRITE, None)
/// ];
///
/// setfacl(&["./tmp/foo"], &entries, None)?;
/// # Ok(()) }
/// ```
///
/// # Linux
///
/// Each entry can only allow access; denying access using allow=false is not
/// supported on Linux.
///
/// The ACL *must* contain entries for the permission modes of the file. Use
/// the [`AclEntry::allow_other`] and [`AclEntry::allow_mask`] functions to
/// specify the mode's other and mask permissions. Use "" as the name for the
/// file owner and group owner.
///
/// If an ACL contains a named user or group, there should be a
/// [`AclEntryKind::Mask`] entry included. If a one entry is not provided, one
/// will be computed.
///
/// The access control entries may include entries for the default ACL, if one
/// is desired. When `setfacl` is called with no [`Flag::DEFAULT`] entries, it
/// deletes the default ACL.
///
/// ### Linux Example
///
/// ```ignore
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use exacl::{setfacl, AclEntry, Flag, Perm};
///
/// let entries = vec![
///     AclEntry::allow_user("", Perm::READ | Perm::WRITE, None),
///     AclEntry::allow_group("", Perm::READ, None),
///     AclEntry::allow_other(Perm::empty(), None),
///     AclEntry::allow_user("some_user", Perm::READ | Perm::WRITE, None),
/// ];
///
/// setfacl(&["./tmp/foo"], &entries, None)?;
/// # Ok(()) }
/// ```
///
/// # Errors
///
/// Returns an [`io::Error`] on failure.
///
pub fn setfacl<'a, F, O>(files: F, entries: &[AclEntry], options: O) -> io::Result<()>
where
    F: AclFilePaths<'a>,
    O: Into<Option<AclOption>>,
{
    my_setfacl(files, entries, options.into().unwrap_or_default())
}

#[cfg(target_os = "macos")]
fn my_setfacl<'a, F>(files: F, entries: &[AclEntry], options: AclOption) -> io::Result<()>
where
    F: AclFilePaths<'a>,
{
    let acl = Acl::from_entries(entries).map_err(|err| custom_err("Invalid ACL", &err))?;
    for file in files.file_iter() {
        file.write(&acl, options)?;
    }

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn my_setfacl<'a, F>(files: F, entries: &[AclEntry], options: AclOption) -> io::Result<()>
where
    F: AclFilePaths<'a>,
{
    if options.contains(AclOption::ACCESS_ACL | AclOption::DEFAULT_ACL) {
        fail_custom("ACCESS_ACL and DEFAULT_ACL are mutually exclusive options")?;
    } else if options.intersects(AclOption::ACCESS_ACL | AclOption::DEFAULT_ACL) {
        let acl = Acl::from_entries(entries).map_err(|err| custom_err("Invalid ACL", &err))?;

        for file in files.file_iter() {
            file.write(&acl, options)?;
        }
    } else {
        let (access_acl, default_acl) =
            Acl::from_unified_entries(entries).map_err(|err| custom_err("Invalid ACL", &err))?;

        if access_acl.is_empty() {
            fail_custom("Invalid ACL: missing required entries")?;
        }

        for file in files.file_iter() {
            if access_acl.is_posix() {
                // Try to set default acl first. This will fail if file is not
                // a directory and default_acl is non-empty. This ordering
                // avoids leaving the file's ACL in a partially changed state
                // after an error (simply because it was a non-directory).
                file.write(
                    &default_acl,
                    options | AclOption::DEFAULT_ACL | AclOption::IGNORE_EXPECTED_FILE_ERR,
                )?;
            }
            file.write(&access_acl, options)?;
        }
    }

    Ok(())
}

/// Write ACL entries to text.
///
/// Each ACL entry is printed on a separate line. The five fields are separated
/// by colons:
///
/// ```text
///   <allow>:<flags>:<kind>:<name>:<perms>
///
///   <allow> - one of "allow" or "deny"
///   <flags> - comma-separated list of flags
///   <kind>  - one of "user", "group", "other", "mask", "unknown"
///   <name>  - user/group name (or decimal id if not known)
///   <perms> - comma-separated list of permissions
/// ```
///
/// Each record, including the last, is terminated by a final newline.
///
/// # Sample Output
///
/// ```text
/// allow::group:admin:read,write
/// ```
///
/// # Errors
///
/// Returns an [`io::Error`] on failure.
pub fn to_writer<W: io::Write>(mut writer: W, entries: &[AclEntry]) -> io::Result<()> {
    for entry in entries {
        writeln!(writer, "{entry}")?;
    }

    Ok(())
}

/// Read ACL entries from text.
///
/// Each ACL entry is presented on a separate line. A comment begins with `#`
/// and proceeds to the end of the line. Within a field, leading or trailing
/// white space are ignored.
///
/// ```text
///   Three allowed forms:
///
///   <allow>:<flags>:<kind>:<name>:<perms>
///   <flags>:<kind>:<name>:<perms>
///   <kind>:<name>:<perms>
///
///   <allow> - one of "allow" or "deny"
///   <flags> - comma-separated list of flags
///   <kind>  - one of "user", "group", "other", "mask", "unknown"
///   <name>  - user/group name (decimal id accepted)
///   <perms> - comma-separated list of permissions
/// ```
///
/// Supported flags and permissions vary by platform.
///
/// Supported abbreviations:  d = default, r = read, w = write, x = execute,
/// u = user, g = group, o = other, m = mask
///
/// # Sample Input
///
/// ```text
/// allow::group:admin:read,write
/// g:admin:rw  # ignored
/// d:u:chip:rw
/// deny:file_inherit:user:chet:rwx
/// ```
///
/// # Errors
///
/// Returns an [`io::Error`] on failure.
pub fn from_reader<R: io::Read>(reader: R) -> io::Result<Vec<AclEntry>> {
    let mut result = Vec::<AclEntry>::new();
    let buf = io::BufReader::new(reader);

    for line_result in buf.lines() {
        let line = line_result?;

        let src_line = trim_comment(&line).trim();
        if !src_line.is_empty() {
            result.push(src_line.parse::<AclEntry>()?);
        }
    }

    Ok(result)
}

/// Return line with end of line comment removed.
fn trim_comment(line: &str) -> &str {
    line.find('#').map_or(line, |n| &line[0..n])
}

/// Write ACL entries to text.
///
/// See `to_writer` for the format.
///
/// # Errors
///
/// Returns an [`io::Error`] on failure.
pub fn to_string(entries: &[AclEntry]) -> io::Result<String> {
    let mut buf = Vec::<u8>::with_capacity(128);
    to_writer(&mut buf, entries)?;
    String::from_utf8(buf).map_err(io::Error::other)
}

/// Read ACL entries from text.
///
/// See `from_reader` for the format.
///
/// # Errors
///
/// Returns an [`io::Error`] on failure.
pub fn from_str(s: &str) -> io::Result<Vec<AclEntry>> {
    from_reader(s.as_bytes())
}

/// Construct a minimal ACL from the traditional `mode` permission bits.
///
/// Returns a `Vec<AclEntry>` for a minimal ACL with three entries corresponding
/// to the owner/group/other permission bits given in `mode`.
///
/// Extra bits outside the mask 0o777 are ignored.
#[cfg(any(docsrs, target_os = "linux", target_os = "freebsd"))]
#[cfg_attr(docsrs, doc(cfg(any(target_os = "linux", target_os = "freebsd"))))]
#[must_use]
pub fn from_mode(mode: u32) -> Vec<AclEntry> {
    vec![
        AclEntry::allow_user("", Perm::from_bits_truncate((mode >> 6) & 7), None),
        AclEntry::allow_group("", Perm::from_bits_truncate((mode >> 3) & 7), None),
        AclEntry::allow_other(Perm::from_bits_truncate(mode & 7), None),
    ]
}
