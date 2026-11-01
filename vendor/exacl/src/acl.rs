//! Provides `Acl` and `AclOption` implementation.

use crate::aclentry::AclEntry;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use crate::aclentry::AclEntryKind;
use crate::failx::{fail_custom, path_err};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use crate::flag::Flag;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use crate::perm::Perm;
use crate::util::*;

use bitflags::bitflags;
use scopeguard::{self, ScopeGuard};
use std::io;
use std::os::fd::RawFd;
use std::path::Path;

bitflags! {
    /// Controls how ACL's are accessed.
    #[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy, Default)]
    pub struct AclOption : u32 {
        /// Get/set the access ACL only (Linux and FreeBSD only).
        const ACCESS_ACL = 0b0001;

        /// Get/set the default ACL only (Linux and FreeBSD only).
        const DEFAULT_ACL = 0b0010;

        /// Get/set the ACL of the symlink itself (macOS only).
        const SYMLINK_ACL = 0b0100;

        /// Retrieve the native, underlying system representation of each user
        /// or group. This is a numeric decimal UID/GID on Linux/FreeBSD and a
        /// GUID on macOS.
        const NATIVE_ID = 0b1000;

        /// Ignore expected error when using DEFAULT_ACL on a file.
        #[doc(hidden)]
        const IGNORE_EXPECTED_FILE_ERR = 0b10000;
    }
}

/// Access Control List native object wrapper.
///
/// Each [`Acl`] is immutable once constructed. To manipulate its contents, you
/// can retrieve a mutable vector of [`AclEntry`], modify the vector's contents,
/// then create a new [`Acl`].
pub struct Acl {
    /// Native acl.
    acl: acl_t,

    /// Set to true if `acl` was set from the default ACL for a directory
    /// using `DEFAULT_ACL` option. Used to return entries with the `DEFAULT`
    /// flag set.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    default_acl: bool,
}

impl Acl {
    /// Convenience function to construct an `Acl`.
    #[allow(unused_variables)]
    fn new(acl: acl_t, default_acl: bool) -> Acl {
        assert!(!acl.is_null());
        Acl {
            acl,
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            default_acl,
        }
    }

    /// Read ACL for the specified file.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] on failure.
    pub fn read(path: &Path, options: AclOption) -> io::Result<Acl> {
        let symlink_acl = options.contains(AclOption::SYMLINK_ACL);
        let default_acl = options.contains(AclOption::DEFAULT_ACL);

        let result = xacl_get_file(path, symlink_acl, default_acl);
        match result {
            Ok(acl) => Ok(Acl::new(acl, default_acl)),
            Err(err) => {
                // Trying to access the default ACL of a non-directory on Linux
                // will return an error. We can catch this error and return an
                // empty ACL instead; only if `IGNORE_EXPECTED_FILE_ERR` is set.
                // (Linux returns permission denied. FreeBSD returns invalid
                // argument.)
                if default_acl
                    && (err.kind() == io::ErrorKind::PermissionDenied
                        || err.kind() == io::ErrorKind::InvalidInput)
                    && options.contains(AclOption::IGNORE_EXPECTED_FILE_ERR)
                    && is_non_directory(path, symlink_acl)
                {
                    // Return an empty acl.
                    Ok(Acl::new(xacl_init(1)?, default_acl))
                } else {
                    Err(path_err(path, &err))
                }
            }
        }
    }

    pub fn read_fd(fd: RawFd, options: AclOption) -> io::Result<Acl> {
        // TODO: fix redundant source code.
        let default_acl = options.contains(AclOption::DEFAULT_ACL);

        let result = xacl_get_fd(fd, default_acl);
        match result {
            Ok(acl) => Ok(Acl::new(acl, default_acl)),
            Err(err) => {
                // Trying to access the default ACL of a non-directory on Linux
                // will return an error. We can catch this error and return an
                // empty ACL instead; only if `IGNORE_EXPECTED_FILE_ERR` is set.
                // (Linux returns permission denied. FreeBSD returns invalid
                // argument.)
                if default_acl
                    && (err.kind() == io::ErrorKind::PermissionDenied
                        || err.kind() == io::ErrorKind::InvalidInput)
                    && options.contains(AclOption::IGNORE_EXPECTED_FILE_ERR)
                {
                    // Return an empty acl.
                    Ok(Acl::new(xacl_init(1)?, default_acl))
                } else {
                    Err(err)
                }
            }
        }
    }

    /// Write ACL for the specified file.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] on failure.
    pub fn write(&self, path: &Path, options: AclOption) -> io::Result<()> {
        let symlink_acl = options.contains(AclOption::SYMLINK_ACL);
        let default_acl = options.contains(AclOption::DEFAULT_ACL);

        // If we're writing a default ACL to a non-directory, and we
        // specify the `IGNORE_EXPECTED_FILE_ERR` option, this function is a
        // no-op if the ACL is empty.
        if default_acl && is_non_directory(path, symlink_acl) {
            if self.is_empty() && options.contains(AclOption::IGNORE_EXPECTED_FILE_ERR) {
                return Ok(());
            }

            return fail_custom(&format!(
                "File {path:?}: Non-directory does not have default ACL"
            ));
        }

        // FIXME: Use map_err.
        if let Err(err) = xacl_set_file(path, self.acl, symlink_acl, default_acl) {
            return Err(path_err(path, &err));
        }

        Ok(())
    }

    /// Write ACL for the specified file.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] on failure.
    pub fn write_fd(&self, fd: RawFd, options: AclOption) -> io::Result<()> {
        // TODO: fix redundant source code.
        let default_acl = options.contains(AclOption::DEFAULT_ACL);

        // If we're writing a default ACL to a non-directory, and we
        // specify the `IGNORE_EXPECTED_FILE_ERR` option, this function is a
        // no-op if the ACL is empty.
        if default_acl && is_non_directory_fd(fd) {
            if self.is_empty() && options.contains(AclOption::IGNORE_EXPECTED_FILE_ERR) {
                return Ok(());
            }

            return fail_custom(&format!(
                "File {fd:?}: Non-directory does not have default ACL"
            ));
        }

        xacl_set_fd(fd, self.acl, default_acl)
    }

    /// Compute mask.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn compute_mask_perms(entries: &[AclEntry], filter: (Flag, Flag)) -> Option<Perm> {
        let mut perms = Perm::empty();
        let mut need_mask = false;

        for entry in entries {
            // Skip over undesired entries in a unified ACL.
            if (entry.flags & filter.1) != filter.0 {
                continue;
            }

            match entry.kind {
                AclEntryKind::Mask => return None,
                AclEntryKind::User | AclEntryKind::Group if !entry.name.is_empty() => {
                    perms |= entry.perms;
                    need_mask = true;
                }
                AclEntryKind::Group => perms |= entry.perms,
                _ => (),
            }
        }

        if !need_mask {
            return None;
        }

        Some(perms)
    }

    /// Check for required entries that are missing.
    ///
    /// It is valid for there to be zero entries.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn find_missing_entries(entries: &[AclEntry], filter: (Flag, Flag)) -> Option<AclEntryKind> {
        let mut miss_user = true;
        let mut miss_group = true;
        let mut miss_other = true;
        let mut is_empty = true;

        for entry in entries {
            // Skip over undesired entries in a unified ACL.
            if (entry.flags & filter.1) != filter.0 {
                continue;
            }

            is_empty = false;
            match entry.kind {
                AclEntryKind::User if entry.name.is_empty() => miss_user = false,
                AclEntryKind::Group if entry.name.is_empty() => miss_group = false,
                AclEntryKind::Other => miss_other = false,
                _ => (),
            }
        }

        if is_empty {
            None
        } else if miss_user {
            Some(AclEntryKind::User)
        } else if miss_group {
            Some(AclEntryKind::Group)
        } else if miss_other {
            Some(AclEntryKind::Other)
        } else {
            None
        }
    }

    /// Return an ACL from a slice of [`AclEntry`].
    ///
    /// On Linux, if there is no mask `AclEntry`, one will be computed and
    /// added, if needed.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] on failure.
    pub fn from_entries(entries: &[AclEntry]) -> io::Result<Acl> {
        let new_acl = xacl_init(entries.len())?;

        // Use the smart pointer form of scopeguard; `acl_p` can change value
        // when we create entries in it.
        let mut acl_p = scopeguard::guard(new_acl, |a| {
            xacl_free(a);
        });

        for (i, entry) in entries.iter().enumerate() {
            if let Err(err) = entry.add_to_acl(&mut acl_p) {
                return fail_custom(&format!("entry {i}: {err}"));
            }
        }

        // Check for missing required entries.
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if let Some(kind) = Acl::find_missing_entries(entries, (Flag::empty(), Flag::empty())) {
            return fail_custom(&format!("missing required entry \"{kind}\""));
        }

        // Check if we need to add a mask entry.
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if let Some(mask_perms) = Acl::compute_mask_perms(entries, (Flag::empty(), Flag::empty())) {
            let mask = AclEntry::allow_mask(mask_perms, None);
            if let Err(err) = mask.add_to_acl(&mut acl_p) {
                return fail_custom(&format!("entry -1: {err}"));
            }
        }

        Ok(Acl::new(ScopeGuard::into_inner(acl_p), false))
    }

    /// Return pair of ACL's from slice of [`AclEntry`]. This method separates
    /// regular access entries from default entries and returns two ACL's, an
    /// access ACL and default ACL. Either may be empty.
    ///
    /// If there is no mask `AclEntry` in an ACL, one will be computed and
    /// added, if needed.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] on failure.
    #[cfg(any(docsrs, target_os = "linux", target_os = "freebsd"))]
    #[cfg_attr(docsrs, doc(cfg(any(target_os = "linux", target_os = "freebsd"))))]
    pub fn from_unified_entries(entries: &[AclEntry]) -> io::Result<(Acl, Acl)> {
        let new_access = xacl_init(entries.len())?;
        let new_default = xacl_init(entries.len())?;

        // Use the smart pointer form of scopeguard; acls can change value when
        // we create entries in them.
        let mut access_p = scopeguard::guard(new_access, |a| {
            xacl_free(a);
        });

        let mut default_p = scopeguard::guard(new_default, |a| {
            xacl_free(a);
        });

        for (i, entry) in entries.iter().enumerate() {
            let result = if entry.flags.contains(Flag::DEFAULT) {
                entry.add_to_acl(&mut default_p)
            } else {
                entry.add_to_acl(&mut access_p)
            };
            if let Err(err) = result {
                return fail_custom(&format!("entry {i}: {err}"));
            }
        }

        if xacl_is_posix(*access_p) {
            // Check for missing entries in both access and default entries.
            if let Some(kind) = Acl::find_missing_entries(entries, (Flag::empty(), Flag::DEFAULT)) {
                return fail_custom(&format!("missing required entry \"{kind}\""));
            }

            if let Some(kind) = Acl::find_missing_entries(entries, (Flag::DEFAULT, Flag::DEFAULT)) {
                return fail_custom(&format!("missing required default entry \"{kind}\""));
            }

            // Check if we need to add a mask entry.
            if let Some(mask_perms) =
                Acl::compute_mask_perms(entries, (Flag::empty(), Flag::DEFAULT))
            {
                let mask = AclEntry::allow_mask(mask_perms, None);
                if let Err(err) = mask.add_to_acl(&mut access_p) {
                    return fail_custom(&format!("mask entry: {err}"));
                }
            }

            if let Some(mask_perms) =
                Acl::compute_mask_perms(entries, (Flag::DEFAULT, Flag::DEFAULT))
            {
                let mask = AclEntry::allow_mask(mask_perms, Flag::DEFAULT);
                if let Err(err) = mask.add_to_acl(&mut default_p) {
                    return fail_custom(&format!("default mask entry: {err}"));
                }
            }
        }

        let access_acl = ScopeGuard::into_inner(access_p);
        let default_acl = ScopeGuard::into_inner(default_p);

        Ok((Acl::new(access_acl, false), Acl::new(default_acl, true)))
    }

    /// Return ACL as a vector of [`AclEntry`].
    ///
    /// If `native` is true, retrieve the numeric uid/gid values instead of
    /// translating them to names.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] on failure.
    pub fn entries(&self, native: bool) -> io::Result<Vec<AclEntry>> {
        let mut entries = Vec::<AclEntry>::with_capacity(8);

        xacl_foreach(self.acl, |entry_p| {
            let entry = AclEntry::from_raw(entry_p, self.acl, native)?;
            entries.push(entry);
            Ok(())
        })?;

        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if self.default_acl {
            // Set DEFAULT flag on each entry.
            for entry in &mut entries {
                entry.flags |= Flag::DEFAULT;
            }
        }

        Ok(entries)
    }

    /// Return ACL as a string.
    ///
    /// This method is provided as a tracing/debugging aid.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] on failure.
    #[cfg(test)]
    pub fn to_string(&self) -> io::Result<String> {
        use std::io::Write;
        let mut buf = Vec::new();
        for entry in self.entries(false)? {
            writeln!(buf, "{entry}")?;
        }
        String::from_utf8(buf).map_err(io::Error::other)
    }

    /// Return true if ACL is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        xacl_is_empty(self.acl)
    }

    /// Return true if ACL is a Posix.1e ACL on Linux or `FreeBSD`.
    #[must_use]
    #[allow(clippy::missing_const_for_fn, dead_code)]
    pub fn is_posix(&self) -> bool {
        xacl_is_posix(self.acl)
    }

    /// Return true if file uses an `NFSv4` ACL (`FreeBSD` only).
    ///
    /// Only used in testing.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] on failure.
    #[cfg(any(docsrs, target_os = "freebsd"))]
    #[cfg_attr(docsrs, doc(cfg(target_os = "freebsd")))]
    #[allow(dead_code)]
    pub fn is_nfs4(path: &Path, options: AclOption) -> io::Result<bool> {
        xacl_is_nfs4(path, options.contains(AclOption::SYMLINK_ACL))
    }
}

impl Drop for Acl {
    fn drop(&mut self) {
        xacl_free(self.acl);
    }
}

/// Return true if path exists and it's not a directory.
fn is_non_directory(path: &Path, symlink: bool) -> bool {
    let result = if symlink {
        path.symlink_metadata()
    } else {
        path.metadata()
    };

    result.is_ok_and(|meta| !meta.is_dir())
}

/// Return true if path exists and it's not a directory.
fn is_non_directory_fd(fd: RawFd) -> bool {
    // FIXME: Consider using BorrowedFd here instead.
    use std::os::fd::{FromRawFd, IntoRawFd};
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let result = file.metadata();

    // Prevent File's destructor from closing the raw_fd prematurely.
    let _ = file.into_raw_fd();

    result.is_ok_and(|meta| !meta.is_dir())
}

////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod acl_tests {
    use super::*;
    use crate::flag::Flag;
    use crate::perm::Perm;
    use log::debug;

    #[test]
    fn test_read_acl() -> io::Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        let acl = Acl::read(file.as_ref(), AclOption::empty())?;
        let entries = acl.entries(false)?;

        #[cfg(target_os = "macos")]
        assert_eq!(entries.len(), 0);

        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        assert_eq!(entries.len(), 3);

        for entry in &entries {
            debug!("{entry}");
        }

        Ok(())
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_write_acl_macos() -> io::Result<()> {
        let mut entries = Vec::<AclEntry>::new();
        let rwx = Perm::READ | Perm::WRITE | Perm::EXECUTE;

        entries.push(AclEntry::allow_group("_spotlight", rwx, None));
        entries.push(AclEntry::allow_user("11501", rwx, None));
        entries.push(AclEntry::allow_user("11502", rwx, None));
        entries.push(AclEntry::allow_user("11503", rwx, None));
        entries.push(AclEntry::deny_group(
            "11504",
            rwx,
            Flag::FILE_INHERIT | Flag::DIRECTORY_INHERIT,
        ));

        let file = tempfile::NamedTempFile::new()?;
        let acl = Acl::from_entries(&entries)?;
        assert!(!acl.is_empty());
        acl.write(file.as_ref(), AclOption::empty())?;

        // Even though the last entry is a group, the `acl_to_text` representation
        // displays it as `user`. (FIXME: old comment? acl_to_text not used here)
        assert_eq!(
            acl.to_string()?,
            r"allow::group:_spotlight:read,write,execute
allow::user:11501:read,write,execute
allow::user:11502:read,write,execute
allow::user:11503:read,write,execute
deny:file_inherit,directory_inherit:group:11504:read,write,execute
"
        );

        let acl2 = Acl::read(file.as_ref(), AclOption::empty())?;
        let entries2 = acl2.entries(false)?;
        assert_eq!(entries2, entries);
        assert_eq!(entries2[0].name, "_spotlight");

        // Test native option.
        let entries3 = acl2.entries(true)?;
        assert_eq!(entries3[0].name, "{abcdefab-cdef-abcd-efab-cdef00000059}");

        Ok(())
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn test_write_acl_posix() -> io::Result<()> {
        let file = tempfile::NamedTempFile::new()?;

        // Skip the rest of the test if file uses NFSv4 ACL (FIXME).
        #[cfg(target_os = "freebsd")]
        if Acl::is_nfs4(file.as_ref(), AclOption::empty())? {
            return Ok(());
        }

        let mut entries = Vec::<AclEntry>::new();
        let rwx = Perm::READ | Perm::WRITE | Perm::EXECUTE;

        entries.push(AclEntry::allow_group("bin", rwx, None));
        entries.push(AclEntry::allow_user("11501", rwx, None));
        entries.push(AclEntry::allow_user("11502", rwx, None));
        entries.push(AclEntry::allow_user("11503", rwx, None));
        entries.push(AclEntry::allow_user("", rwx, None));
        entries.push(AclEntry::allow_group("", rwx, None));
        entries.push(AclEntry::allow_other(rwx, None));
        // We do not add a mask entry. One will be automatically added.

        let acl = Acl::from_entries(&entries)?;
        acl.write(file.as_ref(), AclOption::empty())?;

        assert_eq!(
            acl.to_string()?,
            r"allow::user::read,write,execute
allow::user:11501:read,write,execute
allow::user:11502:read,write,execute
allow::user:11503:read,write,execute
allow::group::read,write,execute
allow::group:bin:read,write,execute
allow::mask::read,write,execute
allow::other::read,write,execute
"
        );

        let acl2 = Acl::read(file.as_ref(), AclOption::empty())?;
        let mut entries2 = acl2.entries(false)?;

        // Before doing the comparison, add the mask entry.
        entries.push(AclEntry::allow_mask(rwx, None));

        entries.sort();
        entries2.sort();
        assert_eq!(entries2, entries);
        assert_eq!(entries2[5].name, "bin");

        // Test native option with "bin" group.
        entries2 = acl2.entries(true)?;
        entries2.sort();
        #[cfg(target_os = "linux")] // FIXME: test code should look these up.
        let expected_gid = "2";
        #[cfg(target_os = "freebsd")]
        let expected_gid = "7";
        assert_eq!(entries2[5].name, expected_gid);

        Ok(())
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_write_acl_big() -> io::Result<()> {
        let mut entries = Vec::<AclEntry>::new();
        let rwx = Perm::READ | Perm::WRITE | Perm::EXECUTE;

        for _ in 0..128 {
            entries.push(AclEntry::allow_user("11501", rwx, None));
        }

        let file = tempfile::NamedTempFile::new()?;
        let acl = Acl::from_entries(&entries)?;
        acl.write(file.as_ref(), AclOption::empty())?;

        let acl2 = Acl::read(file.as_ref(), AclOption::empty())?;
        let entries2 = acl2.entries(false)?;

        assert_eq!(entries2, entries);

        Ok(())
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_write_acl_too_big() {
        let mut entries = Vec::<AclEntry>::new();
        let rwx = Perm::READ | Perm::WRITE | Perm::EXECUTE;

        for _ in 0..129 {
            entries.push(AclEntry::allow_user("11501", rwx, None));
        }

        let err = Acl::from_entries(&entries).err().unwrap();
        assert_eq!(err.to_string(), "Too many ACL entries");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_read_default_acl() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let default_acl = Acl::read(dir.as_ref(), AclOption::DEFAULT_ACL)?;

        assert!(default_acl.is_empty());

        Ok(())
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn test_write_default_acl() -> io::Result<()> {
        let dir = tempfile::tempdir()?;

        // Skip the rest of the test if file uses NFSv4 ACL (FIXME).
        #[cfg(target_os = "freebsd")]
        if Acl::is_nfs4(dir.as_ref(), AclOption::empty())? {
            return Ok(());
        }

        let mut entries = Vec::<AclEntry>::new();
        let rwx = Perm::READ | Perm::WRITE | Perm::EXECUTE;

        entries.push(AclEntry::allow_user("", rwx, None));
        entries.push(AclEntry::allow_group("", rwx, None));
        entries.push(AclEntry::allow_other(rwx, None));
        entries.push(AclEntry::allow_group("bin", rwx, None));
        entries.push(AclEntry::allow_mask(rwx, None));

        let path = dir.as_ref();
        let acl = Acl::from_entries(&entries)?;
        acl.write(path, AclOption::DEFAULT_ACL)?;

        let acl2 = Acl::read(path, AclOption::empty())?;
        assert_ne!(acl.to_string()?, acl2.to_string()?);

        let default_acl = Acl::read(path, AclOption::DEFAULT_ACL)?;
        let default_entries = default_acl.entries(false)?;
        for entry in &default_entries {
            assert_eq!(entry.flags, Flag::DEFAULT);
        }

        // Test deleting a default ACL by passing an empty acl.
        debug!("Test deleting a default ACL");
        let empty_acl = Acl::from_entries(&[])?;
        empty_acl.write(path, AclOption::DEFAULT_ACL)?;
        assert!(Acl::read(path, AclOption::DEFAULT_ACL)?.is_empty());

        Ok(())
    }

    #[test]
    fn test_from_entries() {
        // 0 entries should result in empty acl.
        let acl = Acl::from_entries(&[]).unwrap();
        assert!(acl.is_empty());

        // Test named user on MacOS.
        #[cfg(target_os = "macos")]
        {
            let entries = vec![AclEntry::allow_user("500", Perm::EXECUTE, None)];
            let acl = Acl::from_entries(&entries).unwrap();
            assert_eq!(acl.to_string().unwrap(), "allow::user:500:execute\n");
        }

        // Test named user on Linux. It should add correct mask.
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let mut entries = vec![
                AclEntry::allow_group("", Perm::READ, None),
                AclEntry::allow_other(Perm::READ, None),
                AclEntry::allow_user("500", Perm::EXECUTE, None),
            ];

            let err = Acl::from_entries(&entries).err().unwrap();
            assert_eq!(err.to_string(), "missing required entry \"user\"");

            entries.push(AclEntry::allow_user("", Perm::READ, None));
            let acl = Acl::from_entries(&entries).unwrap();

            #[cfg(target_os = "linux")]
            let expected = "allow::user::read\nallow::user:500:execute\nallow::group::read\nallow::mask::read,execute\nallow::other::read\n";
            #[cfg(target_os = "freebsd")]
            let expected = "allow::group::read\nallow::other::read\nallow::user:500:execute\nallow::user::read\nallow::mask::read,execute\n";
            assert_eq!(acl.to_string().unwrap(), expected);

            entries.push(AclEntry::allow_group("", Perm::WRITE, None));
            let err = Acl::from_entries(&entries).err().unwrap();
            assert_eq!(err.to_string(), "entry 4: duplicate entry for \"group\"");
        }
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn test_from_unified_entries() {
        // 0 entries should result in empty acls.
        let (a, d) = Acl::from_unified_entries(&[]).unwrap();
        assert!(a.is_empty());
        assert!(d.is_empty());

        let mut entries = vec![
            AclEntry::allow_user("500", Perm::EXECUTE, None),
            AclEntry::allow_user("501", Perm::EXECUTE, Flag::DEFAULT),
        ];

        // Missing required entries.
        let err = Acl::from_unified_entries(&entries).err().unwrap();
        assert_eq!(err.to_string(), "missing required entry \"user\"");

        entries.push(AclEntry::allow_group("", Perm::WRITE, None));
        entries.push(AclEntry::allow_user("", Perm::READ, None));
        entries.push(AclEntry::allow_other(Perm::empty(), None));

        // Missing required default entries.
        let err = Acl::from_unified_entries(&entries).err().unwrap();
        assert_eq!(err.to_string(), "missing required default entry \"user\"");

        entries.push(AclEntry::allow_group("", Perm::WRITE, Flag::DEFAULT));
        entries.push(AclEntry::allow_user("", Perm::READ, Flag::DEFAULT));
        entries.push(AclEntry::allow_other(Perm::empty(), Flag::DEFAULT));

        let (a, d) = Acl::from_unified_entries(&entries).unwrap();

        #[cfg(target_os = "linux")]
        let expected1 = "allow::user::read\nallow::user:500:execute\nallow::group::write\nallow::mask::write,execute\nallow::other::\n";
        #[cfg(target_os = "freebsd")]
        let expected1 = "allow::user:500:execute\nallow::group::write\nallow::user::read\nallow::other::\nallow::mask::write,execute\n";
        assert_eq!(a.to_string().unwrap(), expected1);

        #[cfg(target_os = "linux")]
        let expected2 = "allow:default:user::read\nallow:default:user:501:execute\nallow:default:group::write\nallow:default:mask::write,execute\nallow:default:other::\n";
        #[cfg(target_os = "freebsd")]
        let expected2 = "allow:default:user:501:execute\nallow:default:group::write\nallow:default:user::read\nallow:default:other::\nallow:default:mask::write,execute\n";
        assert_eq!(d.to_string().unwrap(), expected2);

        entries.push(AclEntry::allow_group("", Perm::WRITE, Flag::DEFAULT));

        let err = Acl::from_unified_entries(&entries).err().unwrap();
        assert_eq!(
            err.to_string(),
            "entry 8: duplicate default entry for \"group\""
        );
    }

    #[test]
    fn test_empty_acl() -> io::Result<()> {
        let acl = Acl::from_entries(&[])?;
        assert!(acl.is_empty());
        Ok(())
    }
}
