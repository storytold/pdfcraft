use crate::failx::*;
use crate::flag::Flag;
use crate::perm::Perm;
use crate::qualifier::Qualifier;
use crate::sys::*;
use crate::util::util_common;

use scopeguard::defer;
use std::ffi::{CString, c_void};
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub use util_common::{xacl_create_entry, xacl_foreach, xacl_free, xacl_init, xacl_is_empty};

use util_common::*;

const fn get_acl_type(default_acl: bool) -> acl_type_t {
    if default_acl {
        sg::ACL_TYPE_DEFAULT
    } else {
        sg::ACL_TYPE_ACCESS
    }
}

pub fn xacl_get_file(path: &Path, symlink_acl: bool, default_acl: bool) -> io::Result<acl_t> {
    if symlink_acl {
        return fail_custom("Linux does not support symlinks with ACL's.");
    }

    let acl_type = get_acl_type(default_acl);
    let c_path = CString::new(path.as_os_str().as_bytes())?;
    let acl = unsafe { acl_get_file(c_path.as_ptr(), acl_type) };

    if acl.is_null() {
        let func = if default_acl {
            "acl_get_file/default"
        } else {
            "acl_get_file/access"
        };
        return fail_err("null", func, &c_path);
    }

    Ok(acl)
}

pub fn xacl_get_fd(fd: RawFd, default_acl: bool) -> io::Result<acl_t> {
    if default_acl {
        // FIXME: acl_get_fd() doesn't let you retrieve the default ACL
        // of a directory, so try using /dev/fd/NN for now...
        let path = format!("/dev/fd/{fd}");
        return xacl_get_file(path.as_ref(), false, default_acl);
    }

    let acl = unsafe { acl_get_fd(fd) };
    if acl.is_null() {
        return fail_err("null", "acl_get_fd/access", fd);
    }

    Ok(acl)
}

pub fn xacl_set_file(
    path: &Path,
    acl: acl_t,
    symlink_acl: bool,
    default_acl: bool,
) -> io::Result<()> {
    if symlink_acl {
        return fail_custom("Linux does not support symlinks with ACL's");
    }

    let c_path = CString::new(path.as_os_str().as_bytes())?;
    let acl_type = get_acl_type(default_acl);
    let ret = unsafe { acl_set_file(c_path.as_ptr(), acl_type, acl) };
    if ret != 0 {
        let func = if default_acl {
            "acl_set_file/default"
        } else {
            "acl_set_file/access"
        };
        return fail_err(ret, func, &c_path);
    }

    Ok(())
}

pub fn xacl_set_fd(fd: RawFd, acl: acl_t, default_acl: bool) -> io::Result<()> {
    // FIXME: This is a cheat. acl_set_fd() doesn't let you set the
    // default ACL of a directory, so try using /dev/fd/NN for now...
    let path = format!("/dev/fd/{fd}");
    xacl_set_file(path.as_ref(), acl, false, default_acl)
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
        tag => Qualifier::Unknown(format!("@tag {tag}")),
    };

    Ok(result)
}

fn xacl_get_tag_qualifier(_acl: acl_t, entry: acl_entry_t) -> io::Result<(bool, Qualifier)> {
    let qualifier = xacl_get_qualifier(entry)?;
    Ok((true, qualifier))
}

#[allow(clippy::unnecessary_wraps)]
const fn xacl_get_flags(_acl: acl_t, _entry: acl_entry_t) -> io::Result<Flag> {
    Ok(Flag::empty()) // noop
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

pub fn xacl_set_tag_qualifier(
    entry: acl_entry_t,
    allow: bool,
    qualifier: &Qualifier,
) -> io::Result<()> {
    if !allow {
        return fail_custom("allow=false is not supported on Linux");
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
        Qualifier::Unknown(tag) => {
            return fail_custom(&format!("unknown tag: {tag}"));
        }
    }

    Ok(())
}

#[allow(clippy::unnecessary_wraps)]
const fn xacl_set_flags(_entry: acl_entry_t, _flags: Flag) -> io::Result<()> {
    Ok(()) // noop
}

pub fn xacl_add_entry(
    acl: &mut acl_t,
    allow: bool,
    qualifier: &Qualifier,
    perms: Perm,
    flags: Flag,
) -> io::Result<acl_entry_t> {
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

    let entry = xacl_create_entry(acl)?;
    xacl_set_tag_qualifier(entry, allow, qualifier)?;
    xacl_set_perm(entry, perms)?;
    xacl_set_flags(entry, flags)?;

    Ok(entry)
}

pub const fn xacl_is_posix(_acl: acl_t) -> bool {
    true
}

#[cfg(test)]
mod util_linux_test {
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

        // Write an empty access ACL to a file. Still works?
        xacl_set_file(file.as_ref(), acl, false, false)
            .ok()
            .unwrap();

        // Write an empty default ACL to a file. Still works?
        // FIXME: Fails on ubuntu-18.04.
        //xacl_set_file(file.as_ref(), acl, false, true).ok().unwrap();

        // Write an empty access ACL to a directory. Still works?
        xacl_set_file(dir.as_ref(), acl, false, false).ok().unwrap();

        // Write an empty default ACL to a directory. Okay on Linux, FreeBSD.
        xacl_set_file(dir.as_ref(), acl, false, true).ok().unwrap();

        xacl_free(acl);
    }

    #[test]
    fn test_uninitialized_entry() {
        let mut acl = xacl_init(1).unwrap();
        let entry_p = xacl_create_entry(&mut acl).unwrap();

        let (allow, qualifier) = xacl_get_tag_qualifier(acl, entry_p).unwrap();
        assert_eq!(qualifier.name(false).unwrap(), "@tag 0");
        assert!(allow);

        xacl_free(acl);
    }
}
