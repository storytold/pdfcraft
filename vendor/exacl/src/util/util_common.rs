use crate::bititer::BitIter;
use crate::failx::*;
use crate::perm::Perm;
use crate::sys::*;

use std::ffi::c_void;
use std::io;
use std::ptr;

/// Free memory allocated by native acl_* routines.
pub fn xacl_free<T>(ptr: *mut T) {
    assert!(!ptr.is_null());
    let ret = unsafe { acl_free(ptr.cast::<c_void>()) };
    assert_eq!(ret, 0);
}

/// Return true if acl is empty.
pub fn xacl_is_empty(acl: acl_t) -> bool {
    let mut entry: acl_entry_t = ptr::null_mut();

    !xacl_get_entry(acl, sg::ACL_FIRST_ENTRY, &raw mut entry)
}

/// Return next entry in ACL.
fn xacl_get_entry(acl: acl_t, entry_id: i32, entry_p: *mut acl_entry_t) -> bool {
    let ret = unsafe { acl_get_entry(acl, entry_id, entry_p) };

    // MacOS: Zero indicates success.
    #[cfg(target_os = "macos")]
    return ret == 0;

    // Linux, FreeBSD: One indicates success.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    return ret == 1;
}

/// Iterate over entries in a native ACL.
pub fn xacl_foreach<F: FnMut(acl_entry_t) -> io::Result<()>>(
    acl: acl_t,
    mut func: F,
) -> io::Result<()> {
    let mut entry: acl_entry_t = ptr::null_mut();
    let mut entry_id = sg::ACL_FIRST_ENTRY;

    assert!(!acl.is_null());
    loop {
        if !xacl_get_entry(acl, entry_id, &raw mut entry) {
            break;
        }
        assert!(!entry.is_null());
        func(entry)?;
        entry_id = sg::ACL_NEXT_ENTRY;
    }

    Ok(())
}

/// Create a new empty ACL with the given capacity.
///
/// Client must call `xacl_free` when done with result.
pub fn xacl_init(capacity: usize) -> io::Result<acl_t> {
    let size = match i32::try_from(capacity) {
        Ok(size) if size <= sg::ACL_MAX_ENTRIES => size,
        _ => return fail_custom("Too many ACL entries"),
    };

    let acl = unsafe { acl_init(size) };
    if acl.is_null() {
        return fail_err("null", "acl_init", capacity);
    }

    Ok(acl)
}

/// Create a new entry in the specified ACL.
///
/// N.B. Memory reallocation may cause `acl` ptr to change.
pub fn xacl_create_entry(acl: &mut acl_t) -> io::Result<acl_entry_t> {
    let mut entry: acl_entry_t = ptr::null_mut();

    let ret = unsafe { acl_create_entry(&raw mut *acl, &raw mut entry) };
    if ret != 0 {
        return fail_err(ret, "acl_create_entry", ());
    }

    Ok(entry)
}

/// Get tag type from entry.
pub fn xacl_get_tag_type(entry: acl_entry_t) -> io::Result<acl_tag_t> {
    let mut tag: acl_tag_t = 0;

    let ret = unsafe { acl_get_tag_type(entry, &raw mut tag) };
    if ret != 0 {
        return fail_err(ret, "acl_get_tag_type", ());
    }

    Ok(tag)
}

/// Get permissions from the entry.
pub fn xacl_get_perm(entry: acl_entry_t) -> io::Result<Perm> {
    let mut permset: acl_permset_t = std::ptr::null_mut();

    let ret = unsafe { acl_get_permset(entry, &raw mut permset) };
    if ret != 0 {
        return fail_err(ret, "acl_get_permset", ());
    }

    assert!(!permset.is_null());

    let mut perms = Perm::empty();
    for perm in BitIter(Perm::all()) {
        let res = unsafe { acl_get_perm(permset, perm.bits()) };
        debug_assert!((0..=1).contains(&res));
        if res == 1 {
            perms |= perm;
        }
    }

    Ok(perms)
}

/// Set tag type for ACL entry.
pub fn xacl_set_tag_type(entry: acl_entry_t, tag: acl_tag_t) -> io::Result<()> {
    let ret = unsafe { acl_set_tag_type(entry, tag) };
    if ret != 0 {
        return fail_err(ret, "acl_set_tag_type", ());
    }

    Ok(())
}

/// Set permissions for the entry.
pub fn xacl_set_perm(entry: acl_entry_t, perms: Perm) -> io::Result<()> {
    let mut permset: acl_permset_t = std::ptr::null_mut();

    let ret_get = unsafe { acl_get_permset(entry, &raw mut permset) };
    if ret_get != 0 {
        return fail_err(ret_get, "acl_get_permset", ());
    }

    assert!(!permset.is_null());

    let ret_clear = unsafe { acl_clear_perms(permset) };
    if ret_clear != 0 {
        return fail_err(ret_clear, "acl_clear_perms", ());
    }

    for perm in BitIter(perms) {
        let ret = unsafe { acl_add_perm(permset, perm.bits()) };
        debug_assert_eq!(ret, 0);
    }

    Ok(())
}
