use crate::bititer::BitIter;
use crate::failx::*;
use crate::flag::Flag;
use crate::perm::Perm;
use crate::qualifier::Qualifier;
use crate::sys::*;
use crate::util::util_common;

use log::debug;
use scopeguard::defer;
use std::ffi::{CString, c_void};
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

pub use util_common::{xacl_create_entry, xacl_foreach, xacl_free, xacl_init, xacl_is_empty};

use util_common::*;

fn get_acl_type(acl: acl_t, default_acl: bool) -> acl_type_t {
    if !acl.is_null() && !xacl_is_posix(acl) {
        sg::ACL_TYPE_NFS4
    } else if default_acl {
        sg::ACL_TYPE_DEFAULT
    } else {
        sg::ACL_TYPE_ACCESS
    }
}

/// Get ACL from file path, don't follow symbolic links.
fn xacl_get_link(path: &Path, default_acl: bool) -> io::Result<acl_t> {
    let mut acl_type = get_acl_type(ptr::null_mut(), default_acl);
    let c_path = CString::new(path.as_os_str().as_bytes())?;
    let acl = unsafe { acl_get_link_np(c_path.as_ptr(), acl_type) };

    if !acl.is_null() {
        return Ok(acl);
    }

    // `acl_get_link_np` returns EINVAL when the ACL type is not appropriate for
    // the file system object. Retry with NFSv4 type.
    // FIXME: `default_acl` setting is currently ignored!
    if io::Error::last_os_error().raw_os_error() == Some(sg::EINVAL) {
        acl_type = sg::ACL_TYPE_NFS4;
        let nfs_acl = unsafe { acl_get_link_np(c_path.as_ptr(), acl_type) };
        if !nfs_acl.is_null() {
            return Ok(nfs_acl);
        }
    }

    // Report acl_type and path to file that failed.
    let func = match acl_type {
        sg::ACL_TYPE_ACCESS => "acl_get_link_np/access",
        sg::ACL_TYPE_DEFAULT => "acl_get_link_np/default",
        sg::ACL_TYPE_NFS4 => "acl_get_link_np/nfs4",
        _ => "acl_get_link_np/?",
    };

    fail_err("null", func, &c_path)
}

/// Get ACL from file path.
///
/// This code first tries to obtain the Posix.1e ACL. If that's not appropriate
/// for the file system object, we try to access the NFS4 ACL.
pub fn xacl_get_file(path: &Path, symlink_acl: bool, default_acl: bool) -> io::Result<acl_t> {
    // Symlinks will use `acl_get_link_np` instead of `acl_get_file`.
    if symlink_acl {
        return xacl_get_link(path, default_acl);
    }

    let mut acl_type = get_acl_type(ptr::null_mut(), default_acl);
    let c_path = CString::new(path.as_os_str().as_bytes())?;
    let acl = unsafe { acl_get_file(c_path.as_ptr(), acl_type) };

    if !acl.is_null() {
        return Ok(acl);
    }

    // `acl_get_file` returns EINVAL when the ACL type is not appropriate for
    // the file system object. Retry with NFSv4 type.
    if io::Error::last_os_error().raw_os_error() == Some(sg::EINVAL)
        && xacl_is_nfs4(path, symlink_acl)?
    {
        // NFSv4 does not support default ACL.
        if default_acl {
            return fail_custom("Default ACL not supported");
        }

        acl_type = sg::ACL_TYPE_NFS4;
        let nfs_acl = unsafe { acl_get_file(c_path.as_ptr(), acl_type) };
        if !nfs_acl.is_null() {
            return Ok(nfs_acl);
        }
    }

    // Report acl_type and path to file that failed.
    let func = match acl_type {
        sg::ACL_TYPE_ACCESS => "acl_get_file/access",
        sg::ACL_TYPE_DEFAULT => "acl_get_file/default",
        sg::ACL_TYPE_NFS4 => "acl_get_file/nfs4",
        _ => "acl_get_file/?",
    };

    fail_err("null", func, &c_path)
}

/// Get ACL from file descriptor.
///
/// This code first tries to obtain the Posix.1e ACL. If that's not appropriate
/// for the file system object, we try to access the NFS4 ACL.
pub fn xacl_get_fd(fd: RawFd, default_acl: bool) -> io::Result<acl_t> {
    let mut acl_type = get_acl_type(ptr::null_mut(), default_acl);
    let acl = unsafe { acl_get_fd_np(fd, acl_type) };

    if !acl.is_null() {
        return Ok(acl);
    }

    // `acl_get_fd_np` returns EINVAL when the ACL type is not appropriate for
    // the file system object. Retry with NFSv4 type.
    if io::Error::last_os_error().raw_os_error() == Some(sg::EINVAL) && xacl_is_nfs4_fd(fd)? {
        // NFSv4 does not support default ACL.
        if default_acl {
            return fail_custom("Default ACL not supported");
        }

        acl_type = sg::ACL_TYPE_NFS4;
        let nfs_acl = unsafe { acl_get_fd_np(fd, acl_type) };
        if !nfs_acl.is_null() {
            return Ok(nfs_acl);
        }
    }

    // Report acl_type and path to file that failed.
    let func = match acl_type {
        sg::ACL_TYPE_ACCESS => "acl_get_fd_np/access",
        sg::ACL_TYPE_DEFAULT => "acl_get_fd_np/default",
        sg::ACL_TYPE_NFS4 => "acl_get_fd_np/nfs4",
        _ => "acl_get_fd_np/?",
    };

    fail_err("null", func, fd)
}

fn xacl_set_file_symlink(path: &Path, acl: acl_t, default_acl: bool) -> io::Result<()> {
    let c_path = CString::new(path.as_os_str().as_bytes())?;

    if default_acl && xacl_is_empty(acl) {
        // Special case to delete the ACL. The FreeBSD version of
        // acl_set_link_np does not handle this case.
        let ret = unsafe { acl_delete_def_link_np(c_path.as_ptr()) };
        if ret != 0 {
            return fail_err(ret, "acl_delete_def_link_np", &c_path);
        }
        return Ok(());
    }

    let acl_type = get_acl_type(acl, default_acl);
    let ret = unsafe { acl_set_link_np(c_path.as_ptr(), acl_type, acl) };
    if ret != 0 {
        let func = if default_acl {
            "acl_set_link_np/default"
        } else {
            "acl_set_link_np/access"
        };
        return fail_err(ret, func, &c_path);
    }

    Ok(())
}

fn xacl_repair_nfs4(acl: acl_t) -> io::Result<()> {
    xacl_foreach(acl, |entry| {
        let entry_type = xacl_get_entry_type(entry)?;
        if entry_type == 0 {
            xacl_set_entry_type(entry, sg::ACL_ENTRY_TYPE_ALLOW)?;
        }
        // Translate READ -> READ_DATA and WRITE -> WRITE_DATA.
        let mut perm = xacl_get_perm(entry)?;
        let orig_perm = perm;
        if perm.intersects(Perm::READ) {
            perm.remove(Perm::READ);
            perm.insert(Perm::READ_DATA);
        }
        if perm.intersects(Perm::WRITE) {
            perm.remove(Perm::WRITE);
            perm.insert(Perm::WRITE_DATA);
        }
        if perm != orig_perm {
            xacl_set_perm(entry, perm)?;
        }
        Ok(())
    })
}

pub fn xacl_set_file(
    path: &Path,
    acl: acl_t,
    symlink_acl: bool,
    default_acl: bool,
) -> io::Result<()> {
    let is_nfs4 = xacl_is_nfs4(path, symlink_acl)?;

    if default_acl && is_nfs4 {
        return fail_custom("Default ACL not supported");
    }

    if !xacl_is_posix(acl) || is_nfs4 {
        // Fix up the ACL to make sure that all entry types are set.
        // FIXME: This mutates the acl, violating the immutable invariant.
        xacl_repair_nfs4(acl)?;
    }

    log_brand("xacl_set_file", acl)?;

    if symlink_acl {
        return xacl_set_file_symlink(path, acl, default_acl);
    }

    let c_path = CString::new(path.as_os_str().as_bytes())?;

    if default_acl && xacl_is_empty(acl) {
        // Special case to delete the ACL. The FreeBSD version of
        // acl_set_file does not handle this case.
        let ret = unsafe { acl_delete_def_file(c_path.as_ptr()) };
        if ret != 0 {
            return fail_err(ret, "acl_delete_def_file", &c_path);
        }
        return Ok(());
    }

    let acl_type = get_acl_type(acl, default_acl);
    let ret = unsafe { acl_set_file(c_path.as_ptr(), acl_type, acl) };
    if ret != 0 {
        let func = match acl_type {
            sg::ACL_TYPE_ACCESS => "acl_set_file/access",
            sg::ACL_TYPE_DEFAULT => "acl_set_file/default",
            sg::ACL_TYPE_NFS4 => "acl_set_file/nfs4",
            _ => "acl_set_file/?",
        };
        return fail_err(ret, func, &c_path);
    }

    Ok(())
}

pub fn xacl_set_fd(fd: RawFd, acl: acl_t, default_acl: bool) -> io::Result<()> {
    let is_nfs4 = xacl_is_nfs4_fd(fd)?;

    if default_acl && is_nfs4 {
        return fail_custom("Default ACL not supported");
    }

    if !xacl_is_posix(acl) || is_nfs4 {
        // Fix up the ACL to make sure that all entry types are set.
        // FIXME: This mutates the acl, violating the immutable invariant.
        xacl_repair_nfs4(acl)?;
    }

    log_brand("xacl_set_fd", acl)?;

    if default_acl && xacl_is_empty(acl) {
        // Special case to delete the ACL. The FreeBSD version of
        // acl_set_file does not handle this case. (FIXME: Verify?)
        let ret = unsafe { acl_delete_fd_np(fd, sg::ACL_TYPE_DEFAULT) };
        if ret != 0 {
            return fail_err(ret, "acl_delete_fd_np", fd);
        }
        return Ok(());
    }

    let acl_type = get_acl_type(acl, default_acl);
    let ret = unsafe { acl_set_fd_np(fd, acl, acl_type) };
    if ret != 0 {
        let func = match acl_type {
            sg::ACL_TYPE_ACCESS => "acl_set_fd_np/access",
            sg::ACL_TYPE_DEFAULT => "acl_set_fd_np/default",
            sg::ACL_TYPE_NFS4 => "acl_set_fd_np/nfs4",
            _ => "acl_set_fd_np/?",
        };
        return fail_err(ret, func, fd);
    }

    Ok(())
}

fn xacl_get_qualifier(entry: acl_entry_t) -> io::Result<Qualifier> {
    let tag = xacl_get_tag_type(entry)?;

    let id = if tag == sg::ACL_USER || tag == sg::ACL_GROUP {
        let id_ptr = unsafe { acl_get_qualifier(entry).cast::<uid_t>() };
        if id_ptr.is_null() {
            return fail_err("null", "acl_get_qualifier", ());
        }
        defer! { xacl_free(id_ptr) };
        Some(unsafe { *id_ptr })
    } else {
        None
    };

    let result = match tag {
        sg::ACL_USER => Qualifier::User(id.unwrap()),
        sg::ACL_GROUP => Qualifier::Group(id.unwrap()),
        sg::ACL_USER_OBJ => Qualifier::UserObj,
        sg::ACL_GROUP_OBJ => Qualifier::GroupObj,
        sg::ACL_OTHER => Qualifier::Other,
        sg::ACL_MASK => Qualifier::Mask,
        sg::ACL_EVERYONE => Qualifier::Everyone,
        tag => Qualifier::Unknown(format!("@tag {tag}")),
    };

    Ok(result)
}

fn xacl_get_entry_type(entry: acl_entry_t) -> io::Result<acl_entry_type_t> {
    let mut entry_type: acl_entry_type_t = 0;

    let ret = unsafe { acl_get_entry_type_np(entry, &raw mut entry_type) };
    if ret != 0 {
        return fail_err(ret, "acl_get_entry_type_np", ());
    }

    // FIXME: AUDIT, ALARM entry types are not supported.
    debug_assert!(
        entry_type == 0
            || entry_type == sg::ACL_ENTRY_TYPE_ALLOW
            || entry_type == sg::ACL_ENTRY_TYPE_DENY
    );

    Ok(entry_type)
}

fn xacl_get_tag_qualifier(acl: acl_t, entry: acl_entry_t) -> io::Result<(bool, Qualifier)> {
    let qualifier = xacl_get_qualifier(entry)?;

    let allow = if xacl_is_posix(acl) {
        true
    } else {
        xacl_get_entry_type(entry)? == sg::ACL_ENTRY_TYPE_ALLOW
    };

    Ok((allow, qualifier))
}

fn xacl_get_flags(acl: acl_t, entry: acl_entry_t) -> io::Result<Flag> {
    if xacl_is_posix(acl) {
        return Ok(Flag::empty());
    }

    let mut flagset: acl_flagset_t = std::ptr::null_mut();
    let ret = unsafe { acl_get_flagset_np(entry, &raw mut flagset) };
    if ret != 0 {
        return fail_err(ret, "acl_get_flagset_np", ());
    }

    assert!(!flagset.is_null());

    let mut flags = Flag::empty();
    for flag in BitIter(Flag::all() - Flag::DEFAULT) {
        let res = unsafe { acl_get_flag_np(flagset, flag.bits()) };
        debug_assert!((0..=1).contains(&res));
        if res == 1 {
            flags |= flag;
        }
    }

    Ok(flags)
}

pub fn xacl_get_entry(acl: acl_t, entry: acl_entry_t) -> io::Result<(bool, Qualifier, Perm, Flag)> {
    let (allow, qualifier) = xacl_get_tag_qualifier(acl, entry)?;
    let perms = xacl_get_perm(entry)?;
    let flags = xacl_get_flags(acl, entry)?;

    Ok((allow, qualifier, perms, flags))
}

pub fn xacl_set_qualifier(entry: acl_entry_t, mut id: uid_t) -> io::Result<()> {
    let id_ptr = std::ptr::addr_of_mut!(id);

    let ret = unsafe { acl_set_qualifier(entry, id_ptr.cast::<c_void>()) };
    if ret != 0 {
        return fail_err(ret, "acl_set_qualifier", ());
    }

    Ok(())
}

fn xacl_set_entry_type(entry: acl_entry_t, entry_type: acl_entry_type_t) -> io::Result<()> {
    let ret = unsafe { acl_set_entry_type_np(entry, entry_type) };
    if ret != 0 {
        return fail_err(ret, "acl_set_entry_type_np", ());
    }

    Ok(())
}

pub fn xacl_set_tag_qualifier(
    entry: acl_entry_t,
    allow: bool,
    qualifier: &Qualifier,
) -> io::Result<()> {
    if !allow {
        xacl_set_entry_type(entry, sg::ACL_ENTRY_TYPE_DENY)?;
    }

    match qualifier {
        Qualifier::User(uid) => {
            xacl_set_tag_type(entry, sg::ACL_USER)?;
            xacl_set_qualifier(entry, *uid)?;
        }
        Qualifier::Group(gid) => {
            xacl_set_tag_type(entry, sg::ACL_GROUP)?;
            xacl_set_qualifier(entry, *gid)?;
        }
        Qualifier::UserObj => {
            xacl_set_tag_type(entry, sg::ACL_USER_OBJ)?;
        }
        Qualifier::GroupObj => {
            xacl_set_tag_type(entry, sg::ACL_GROUP_OBJ)?;
        }
        Qualifier::Other => {
            xacl_set_tag_type(entry, sg::ACL_OTHER)?;
        }
        Qualifier::Mask => {
            xacl_set_tag_type(entry, sg::ACL_MASK)?;
        }
        Qualifier::Everyone => {
            xacl_set_tag_type(entry, sg::ACL_EVERYONE)?;
        }
        Qualifier::Unknown(tag) => {
            return fail_custom(&format!("unknown tag: {tag}"));
        }
    }

    Ok(())
}

fn xacl_set_flags(entry: acl_entry_t, flags: Flag) -> io::Result<()> {
    if flags.is_empty() || flags == Flag::DEFAULT {
        return Ok(());
    }

    let mut flagset: acl_flagset_t = std::ptr::null_mut();
    let ret_get = unsafe { acl_get_flagset_np(entry, &raw mut flagset) };
    if ret_get != 0 {
        return fail_err(ret_get, "acl_get_flagset_np", ());
    }

    assert!(!flagset.is_null());

    let ret_clear = unsafe { acl_clear_flags_np(flagset) };
    if ret_clear != 0 {
        return fail_err(ret_clear, "acl_clear_flags_np", ());
    }

    for flag in BitIter(flags) {
        let ret = unsafe { acl_add_flag_np(flagset, flag.bits()) };
        debug_assert_eq!(ret, 0);
    }

    Ok(())
}

pub fn xacl_add_entry(
    acl: &mut acl_t,
    allow: bool,
    qualifier: &Qualifier,
    perms: Perm,
    flags: Flag,
) -> io::Result<acl_entry_t> {
    let nfs4_specific =
        perms.intersects(Perm::NFS4_SPECIFIC) || flags.intersects(Flag::NFS4_SPECIFIC);

    if allow && xacl_is_posix(*acl) && !nfs4_specific {
        // Check for duplicates already in the list.
        xacl_foreach(*acl, |entry| {
            let (_, prev) = xacl_get_tag_qualifier(*acl, entry)?;
            if prev == *qualifier {
                let default = if flags.contains(Flag::DEFAULT) {
                    "default "
                } else {
                    ""
                };
                fail_custom(&format!("duplicate {default}entry for \"{prev}\""))?;
            }
            Ok(())
        })?;
    }

    let entry = xacl_create_entry(acl)?;
    xacl_set_tag_qualifier(entry, allow, qualifier)?;
    xacl_set_perm(entry, perms)?;
    xacl_set_flags(entry, flags)?;

    // If permissions allow NFSv4 specific bits, set entry type `allow` to force
    // the ACL brand to NFS4.
    if allow && nfs4_specific {
        xacl_set_entry_type(entry, sg::ACL_ENTRY_TYPE_ALLOW)?;
    }

    Ok(entry)
}

fn xacl_get_brand(acl: acl_t) -> io::Result<i32> {
    let mut brand: i32 = 0;
    let ret = unsafe { acl_get_brand_np(acl, &raw mut brand) };
    if ret != 0 {
        return fail_err(ret, "acl_get_brand_np", ());
    }

    Ok(brand)
}

pub fn xacl_is_posix(acl: acl_t) -> bool {
    let brand = xacl_get_brand(acl).expect("xacl_get_brand failed");
    debug_assert!(
        brand == sg::ACL_BRAND_UNKNOWN
            || brand == sg::ACL_BRAND_POSIX
            || brand == sg::ACL_BRAND_NFS4
    );

    // Treat an Unknown branded ACL as Posix.
    brand == sg::ACL_BRAND_POSIX || brand == sg::ACL_BRAND_UNKNOWN
}

fn log_brand(func: &str, acl: acl_t) -> io::Result<()> {
    let brand = match xacl_get_brand(acl)? {
        sg::ACL_BRAND_UNKNOWN => "brand_unknown".to_owned(),
        sg::ACL_BRAND_POSIX => "brand_posix".to_owned(),
        sg::ACL_BRAND_NFS4 => "brand_nfs4".to_owned(),
        value => value.to_string(),
    };

    debug!("{func}: acl {brand}");

    Ok(())
}

pub fn xacl_is_nfs4(path: &Path, symlink: bool) -> io::Result<bool> {
    let c_path = CString::new(path.as_os_str().as_bytes())?;
    let ret = if symlink {
        unsafe { lpathconf(c_path.as_ptr(), sg::PC_ACL_NFS4) }
    } else {
        unsafe { pathconf(c_path.as_ptr(), sg::PC_ACL_NFS4) }
    };

    if ret < 0 {
        return fail_err(ret, "pathconf", symlink);
    }

    assert!(ret == 0 || ret == 1);
    Ok(ret == 1)
}

pub fn xacl_is_nfs4_fd(fd: RawFd) -> io::Result<bool> {
    let ret = unsafe { fpathconf(fd, sg::PC_ACL_NFS4) };
    if ret < 0 {
        return fail_err(ret, "fpathconf", fd);
    }

    assert!(ret == 0 || ret == 1);
    Ok(ret == 1)
}

////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod util_freebsd_test {
    use super::*;

    #[test]
    fn test_acl_api_misuse() {
        // Create empty list and add an entry.
        let mut acl = xacl_init(1).unwrap();
        let entry = xacl_create_entry(&mut acl).unwrap();

        // Setting tag other than 1 or 2 results in EINVAL error.
        let err = xacl_set_tag_type(entry, 0).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(sg::EINVAL));

        // Setting qualifier without first setting tag to a valid value results in EINVAL.
        let err = xacl_set_qualifier(entry, 500).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(sg::EINVAL));

        // Try to set entry using unknown qualifier -- this should fail.
        let err =
            xacl_set_tag_qualifier(entry, true, &Qualifier::Unknown("x".to_string())).unwrap_err();
        assert!(err.to_string().contains("unknown tag: x"));

        // Add another entry and set it to a valid value.
        let entry2 = xacl_create_entry(&mut acl).unwrap();
        xacl_set_tag_type(entry2, sg::ACL_USER_OBJ).unwrap();

        xacl_free(acl);
    }

    #[test]
    fn test_empty_acl() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let dir = tempfile::TempDir::new().unwrap();

        let acl = xacl_init(1).unwrap();
        assert!(xacl_is_empty(acl));

        // Empty acl is not "valid".
        let ret = unsafe { acl_valid(acl) };
        assert_eq!(ret, -1);

        // Not on FreeBSD.
        let err = xacl_set_file(file.as_ref(), acl, false, false)
            .err()
            .unwrap();
        assert_eq!(err.to_string(), "Invalid argument (os error 22)");

        // Write an empty default ACL to a directory. Okay with Posix.1e ACL
        // but fails on NFSv4, because default ACL is not supported.
        let result = xacl_set_file(dir.as_ref(), acl, false, true);
        if xacl_is_nfs4(dir.as_ref(), false).unwrap() {
            assert_eq!(
                result.err().unwrap().to_string(),
                "Default ACL not supported"
            );
        } else {
            result.ok().unwrap();
        }

        xacl_free(acl);
    }

    #[test]
    fn test_uninitialized_entry() {
        let mut acl = xacl_init(1).unwrap();
        let entry_p = xacl_create_entry(&mut acl).unwrap();

        let (allow, qualifier) = xacl_get_tag_qualifier(acl, entry_p).unwrap();
        assert_eq!(qualifier.name(false).unwrap(), "@tag 0");
        // FreeBSD: Unbranded entry is treated as Posix.
        assert!(allow);

        let brand = xacl_get_brand(acl).unwrap();
        assert_eq!(brand, sg::ACL_BRAND_UNKNOWN);

        xacl_free(acl);
    }
}
