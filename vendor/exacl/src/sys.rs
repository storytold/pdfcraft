//! Rust bindings to system C API.

#![allow(dead_code, non_camel_case_types)] // constant is never used

pub use crate::bindings::*;

// Demangle some MacOS constants. Linux provides these as-is.

#[cfg(target_os = "macos")]
pub const ACL_READ: acl_perm_t = acl_perm_t_ACL_READ_DATA;

#[cfg(target_os = "macos")]
pub const ACL_WRITE: acl_perm_t = acl_perm_t_ACL_WRITE_DATA;

#[cfg(target_os = "macos")]
pub const ACL_EXECUTE: acl_perm_t = acl_perm_t_ACL_EXECUTE;

// Linux doesn't have ACL flags; adding acl_flag_t makes the code more orthogonal.
// On FreeBSD, acl_flag_t is a u16.
#[cfg(target_os = "linux")]
pub type acl_flag_t = u32;

// Linux doesn't have ACL_MAX_ENTRIES, so define it as 2 billion.
#[cfg(target_os = "linux")]
pub const ACL_MAX_ENTRIES: u32 = 2_000_000_000;

// MacOS and FreeBSD use acl_get_perm_np().
#[cfg(any(target_os = "macos", target_os = "freebsd"))]
pub unsafe fn acl_get_perm(permset_d: acl_permset_t, perm: acl_perm_t) -> ::std::os::raw::c_int {
    unsafe { acl_get_perm_np(permset_d, perm) }
}

/// Non-portable ACL Permissions & Flags (`macOS` only)
#[cfg(all(target_os = "macos", not(docsrs)))]
pub mod np {
    use super::*;

    pub const ACL_DELETE: acl_perm_t = acl_perm_t_ACL_DELETE;
    pub const ACL_APPEND_DATA: acl_perm_t = acl_perm_t_ACL_APPEND_DATA;
    pub const ACL_DELETE_CHILD: acl_perm_t = acl_perm_t_ACL_DELETE_CHILD;
    pub const ACL_READ_ATTRIBUTES: acl_perm_t = acl_perm_t_ACL_READ_ATTRIBUTES;
    pub const ACL_WRITE_ATTRIBUTES: acl_perm_t = acl_perm_t_ACL_WRITE_ATTRIBUTES;
    pub const ACL_READ_EXTATTRIBUTES: acl_perm_t = acl_perm_t_ACL_READ_EXTATTRIBUTES;
    pub const ACL_WRITE_EXTATTRIBUTES: acl_perm_t = acl_perm_t_ACL_WRITE_EXTATTRIBUTES;
    pub const ACL_READ_SECURITY: acl_perm_t = acl_perm_t_ACL_READ_SECURITY;
    pub const ACL_WRITE_SECURITY: acl_perm_t = acl_perm_t_ACL_WRITE_SECURITY;
    pub const ACL_CHANGE_OWNER: acl_perm_t = acl_perm_t_ACL_CHANGE_OWNER;
    pub const ACL_SYNCHRONIZE: acl_perm_t = acl_perm_t_ACL_SYNCHRONIZE;

    pub const ACL_FLAG_DEFER_INHERIT: acl_flag_t = acl_flag_t_ACL_FLAG_DEFER_INHERIT;
    pub const ACL_FLAG_NO_INHERIT: acl_flag_t = acl_flag_t_ACL_FLAG_NO_INHERIT;
    pub const ACL_ENTRY_INHERITED: acl_flag_t = acl_flag_t_ACL_ENTRY_INHERITED;
    pub const ACL_ENTRY_FILE_INHERIT: acl_flag_t = acl_flag_t_ACL_ENTRY_FILE_INHERIT;
    pub const ACL_ENTRY_DIRECTORY_INHERIT: acl_flag_t = acl_flag_t_ACL_ENTRY_DIRECTORY_INHERIT;
    pub const ACL_ENTRY_LIMIT_INHERIT: acl_flag_t = acl_flag_t_ACL_ENTRY_LIMIT_INHERIT;
    pub const ACL_ENTRY_ONLY_INHERIT: acl_flag_t = acl_flag_t_ACL_ENTRY_ONLY_INHERIT;
}

/// Non-portable ACL Permissions & Flags (`FreeBSD` only)
#[cfg(all(target_os = "freebsd", not(docsrs)))]
pub mod np {
    use super::{acl_flag_t, acl_perm_t};

    pub const ACL_READ_DATA: acl_perm_t = super::ACL_READ_DATA;
    pub const ACL_WRITE_DATA: acl_perm_t = super::ACL_WRITE_DATA;
    // `ACL_EXECUTE` is portable.
    pub const ACL_DELETE: acl_perm_t = super::ACL_DELETE;
    pub const ACL_APPEND_DATA: acl_perm_t = super::ACL_APPEND_DATA;
    pub const ACL_DELETE_CHILD: acl_perm_t = super::ACL_DELETE_CHILD;
    pub const ACL_READ_ATTRIBUTES: acl_perm_t = super::ACL_READ_ATTRIBUTES;
    pub const ACL_WRITE_ATTRIBUTES: acl_perm_t = super::ACL_WRITE_ATTRIBUTES;
    pub const ACL_READ_EXTATTRIBUTES: acl_perm_t = super::ACL_READ_NAMED_ATTRS;
    pub const ACL_WRITE_EXTATTRIBUTES: acl_perm_t = super::ACL_WRITE_NAMED_ATTRS;
    pub const ACL_READ_SECURITY: acl_perm_t = super::ACL_READ_ACL;
    pub const ACL_WRITE_SECURITY: acl_perm_t = super::ACL_WRITE_ACL;
    pub const ACL_CHANGE_OWNER: acl_perm_t = super::ACL_WRITE_OWNER;
    pub const ACL_SYNCHRONIZE: acl_perm_t = super::ACL_SYNCHRONIZE;

    pub const ACL_ENTRY_INHERITED: acl_flag_t = super::ACL_ENTRY_INHERITED as acl_flag_t;
    pub const ACL_ENTRY_FILE_INHERIT: acl_flag_t = super::ACL_ENTRY_FILE_INHERIT as acl_flag_t;
    pub const ACL_ENTRY_DIRECTORY_INHERIT: acl_flag_t =
        super::ACL_ENTRY_DIRECTORY_INHERIT as acl_flag_t;
    pub const ACL_ENTRY_LIMIT_INHERIT: acl_flag_t =
        super::ACL_ENTRY_NO_PROPAGATE_INHERIT as acl_flag_t;
    pub const ACL_ENTRY_ONLY_INHERIT: acl_flag_t = super::ACL_ENTRY_INHERIT_ONLY as acl_flag_t;
    pub const ACL_ENTRY_SUCCESSFUL_ACCESS: acl_flag_t =
        super::ACL_ENTRY_SUCCESSFUL_ACCESS as acl_flag_t;
    pub const ACL_ENTRY_FAILED_ACCESS: acl_flag_t = super::ACL_ENTRY_FAILED_ACCESS as acl_flag_t;
}

/// Non-portable ACL Permissions (Docs only). These are fabricated constants to
/// make it possible for docs to be built on macOS and Linux.
#[cfg(docsrs)]
pub mod np {
    use super::*;

    pub const ACL_READ_DATA: acl_perm_t = 1 << 8;
    pub const ACL_WRITE_DATA: acl_perm_t = 1 << 9;
    pub const ACL_DELETE: acl_perm_t = 1 << 10;
    pub const ACL_APPEND_DATA: acl_perm_t = 1 << 11;
    pub const ACL_DELETE_CHILD: acl_perm_t = 1 << 12;
    pub const ACL_READ_ATTRIBUTES: acl_perm_t = 1 << 13;
    pub const ACL_WRITE_ATTRIBUTES: acl_perm_t = 1 << 14;
    pub const ACL_READ_EXTATTRIBUTES: acl_perm_t = 1 << 15;
    pub const ACL_WRITE_EXTATTRIBUTES: acl_perm_t = 1 << 16;
    pub const ACL_READ_SECURITY: acl_perm_t = 1 << 17;
    pub const ACL_WRITE_SECURITY: acl_perm_t = 1 << 18;
    pub const ACL_CHANGE_OWNER: acl_perm_t = 1 << 19;
    pub const ACL_SYNCHRONIZE: acl_perm_t = 1 << 20;

    pub const ACL_FLAG_DEFER_INHERIT: acl_flag_t = 1 << 21;
    pub const ACL_FLAG_NO_INHERIT: acl_flag_t = 1 << 22;
    pub const ACL_ENTRY_INHERITED: acl_flag_t = 1 << 23;
    pub const ACL_ENTRY_FILE_INHERIT: acl_flag_t = 1 << 24;
    pub const ACL_ENTRY_DIRECTORY_INHERIT: acl_flag_t = 1 << 25;
    pub const ACL_ENTRY_LIMIT_INHERIT: acl_flag_t = 1 << 26;
    pub const ACL_ENTRY_ONLY_INHERIT: acl_flag_t = 1 << 27;
}

// Convenience constants where the API expects a signed i32 type, but bindgen
// provides u32. (FIXME: Replace with bindgen ParseCallbacks::int_macro?)

pub mod sg {
    #![allow(clippy::cast_possible_wrap)]

    use super::*;

    pub const ENOENT: i32 = super::ENOENT as i32;
    pub const ENOTSUP: i32 = super::ENOTSUP as i32;
    pub const EINVAL: i32 = super::EINVAL as i32;
    pub const ENOMEM: i32 = super::ENOMEM as i32;
    pub const ERANGE: i32 = super::ERANGE as i32;
    pub const ACL_MAX_ENTRIES: i32 = super::ACL_MAX_ENTRIES as i32;

    #[cfg(target_os = "macos")]
    pub const ACL_TYPE_EXTENDED: acl_type_t = super::acl_type_t_ACL_TYPE_EXTENDED;
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub const ACL_TYPE_ACCESS: acl_type_t = super::ACL_TYPE_ACCESS as acl_type_t;
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub const ACL_TYPE_DEFAULT: acl_type_t = super::ACL_TYPE_DEFAULT as acl_type_t;
    #[cfg(target_os = "freebsd")]
    pub const ACL_TYPE_NFS4: acl_type_t = super::ACL_TYPE_NFS4 as acl_type_t;
    #[cfg(target_os = "freebsd")]
    pub const ACL_BRAND_UNKNOWN: i32 = super::ACL_BRAND_UNKNOWN as i32;
    #[cfg(target_os = "freebsd")]
    pub const ACL_BRAND_POSIX: i32 = super::ACL_BRAND_POSIX as i32;
    #[cfg(target_os = "freebsd")]
    pub const ACL_BRAND_NFS4: i32 = super::ACL_BRAND_NFS4 as i32;
    #[cfg(target_os = "freebsd")]
    pub const ACL_ENTRY_TYPE_ALLOW: acl_entry_type_t =
        super::ACL_ENTRY_TYPE_ALLOW as acl_entry_type_t;
    #[cfg(target_os = "freebsd")]
    pub const ACL_ENTRY_TYPE_DENY: acl_entry_type_t =
        super::ACL_ENTRY_TYPE_DENY as acl_entry_type_t;

    #[cfg(target_os = "macos")]
    pub const ACL_FIRST_ENTRY: i32 = super::acl_entry_id_t_ACL_FIRST_ENTRY;
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub const ACL_FIRST_ENTRY: i32 = super::ACL_FIRST_ENTRY as i32;

    #[cfg(target_os = "macos")]
    pub const ACL_NEXT_ENTRY: i32 = super::acl_entry_id_t_ACL_NEXT_ENTRY;
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub const ACL_NEXT_ENTRY: i32 = super::ACL_NEXT_ENTRY as i32;

    #[cfg(target_os = "macos")]
    pub const O_SYMLINK: i32 = super::O_SYMLINK as i32;

    #[cfg(target_os = "macos")]
    pub const ACL_EXTENDED_ALLOW: acl_tag_t = super::acl_tag_t_ACL_EXTENDED_ALLOW;
    #[cfg(target_os = "macos")]
    pub const ACL_EXTENDED_DENY: acl_tag_t = super::acl_tag_t_ACL_EXTENDED_DENY;
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub const ACL_USER_OBJ: acl_tag_t = super::ACL_USER_OBJ as acl_tag_t;
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub const ACL_USER: acl_tag_t = super::ACL_USER as acl_tag_t;
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub const ACL_GROUP_OBJ: acl_tag_t = super::ACL_GROUP_OBJ as acl_tag_t;
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub const ACL_GROUP: acl_tag_t = super::ACL_GROUP as acl_tag_t;
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub const ACL_MASK: acl_tag_t = super::ACL_MASK as acl_tag_t;
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub const ACL_OTHER: acl_tag_t = super::ACL_OTHER as acl_tag_t;
    #[cfg(target_os = "freebsd")]
    pub const ACL_EVERYONE: acl_tag_t = super::ACL_EVERYONE as acl_tag_t;

    #[cfg(target_os = "macos")]
    pub const ID_TYPE_UID: i32 = super::ID_TYPE_UID as i32;
    #[cfg(target_os = "macos")]
    pub const ID_TYPE_GID: i32 = super::ID_TYPE_GID as i32;

    #[cfg(target_os = "freebsd")]
    pub const PC_ACL_NFS4: i32 = super::_PC_ACL_NFS4 as i32;

    #[test]
    fn test_signed() {
        assert!(super::ENOENT as i32 >= 0);
        assert!(super::ENOTSUP as i32 >= 0);
        assert!(super::EINVAL as i32 >= 0);
        assert!(super::ENOMEM as i32 >= 0);
        assert!(super::ACL_MAX_ENTRIES as i32 >= 0);

        #[cfg(target_os = "linux")]
        assert!(super::ACL_FIRST_ENTRY as i32 >= 0);

        #[cfg(target_os = "linux")]
        assert!(super::ACL_NEXT_ENTRY as i32 >= 0);

        #[cfg(target_os = "macos")]
        assert!(super::O_SYMLINK as i32 >= 0);

        #[cfg(target_os = "macos")]
        assert!(super::ID_TYPE_UID as i32 >= 0);
        #[cfg(target_os = "macos")]
        assert!(super::ID_TYPE_GID as i32 >= 0);
    }
}
