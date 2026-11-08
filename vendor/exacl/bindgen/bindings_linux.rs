pub const ENOENT: u32 = 2;
pub const ENOMEM: u32 = 12;
pub const EINVAL: u32 = 22;
pub const ERANGE: u32 = 34;
pub const ENOTSUP: u32 = 95;
pub const ACL_READ: u32 = 4;
pub const ACL_WRITE: u32 = 2;
pub const ACL_EXECUTE: u32 = 1;
pub const ACL_UNDEFINED_TAG: u32 = 0;
pub const ACL_USER_OBJ: u32 = 1;
pub const ACL_USER: u32 = 2;
pub const ACL_GROUP_OBJ: u32 = 4;
pub const ACL_GROUP: u32 = 8;
pub const ACL_MASK: u32 = 16;
pub const ACL_OTHER: u32 = 32;
pub const ACL_TYPE_ACCESS: u32 = 32768;
pub const ACL_TYPE_DEFAULT: u32 = 16384;
pub const ACL_FIRST_ENTRY: u32 = 0;
pub const ACL_NEXT_ENTRY: u32 = 1;
pub const ACL_MULTI_ERROR: u32 = 4096;
pub const ACL_DUPLICATE_ERROR: u32 = 8192;
pub const ACL_MISS_ERROR: u32 = 12288;
pub const ACL_ENTRY_ERROR: u32 = 16384;
pub type __uid_t = ::std::os::raw::c_uint;
pub type __gid_t = ::std::os::raw::c_uint;
pub type __mode_t = ::std::os::raw::c_uint;
pub type gid_t = __gid_t;
pub type mode_t = __mode_t;
pub type uid_t = __uid_t;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct __acl_ext {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct __acl_entry_ext {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct __acl_permset_ext {
    _unused: [u8; 0],
}
pub type acl_type_t = ::std::os::raw::c_uint;
pub type acl_tag_t = ::std::os::raw::c_int;
pub type acl_perm_t = ::std::os::raw::c_uint;
pub type acl_t = *mut __acl_ext;
pub type acl_entry_t = *mut __acl_entry_ext;
pub type acl_permset_t = *mut __acl_permset_ext;
unsafe extern "C" {
    pub fn acl_init(count: ::std::os::raw::c_int) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_dup(acl: acl_t) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_free(obj_p: *mut ::std::os::raw::c_void) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_valid(acl: acl_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_copy_entry(dest_d: acl_entry_t, src_d: acl_entry_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_create_entry(acl_p: *mut acl_t, entry_p: *mut acl_entry_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_delete_entry(acl: acl_t, entry_d: acl_entry_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_entry(
        acl: acl_t,
        entry_id: ::std::os::raw::c_int,
        entry_p: *mut acl_entry_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_add_perm(permset_d: acl_permset_t, perm: acl_perm_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_calc_mask(acl_p: *mut acl_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_clear_perms(permset_d: acl_permset_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_delete_perm(permset_d: acl_permset_t, perm: acl_perm_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_permset(
        entry_d: acl_entry_t,
        permset_p: *mut acl_permset_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_permset(entry_d: acl_entry_t, permset_d: acl_permset_t)
        -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_qualifier(entry_d: acl_entry_t) -> *mut ::std::os::raw::c_void;
}
unsafe extern "C" {
    pub fn acl_get_tag_type(
        entry_d: acl_entry_t,
        tag_type_p: *mut acl_tag_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_qualifier(
        entry_d: acl_entry_t,
        tag_qualifier_p: *const ::std::os::raw::c_void,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_tag_type(entry_d: acl_entry_t, tag_type: acl_tag_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_copy_ext(buf_p: *mut ::std::os::raw::c_void, acl: acl_t, size: isize) -> isize;
}
unsafe extern "C" {
    pub fn acl_copy_int(buf_p: *const ::std::os::raw::c_void) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_from_text(buf_p: *const ::std::os::raw::c_char) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_size(acl: acl_t) -> isize;
}
unsafe extern "C" {
    pub fn acl_to_text(acl: acl_t, len_p: *mut isize) -> *mut ::std::os::raw::c_char;
}
unsafe extern "C" {
    pub fn acl_delete_def_file(path_p: *const ::std::os::raw::c_char) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_fd(fd: ::std::os::raw::c_int) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_get_file(path_p: *const ::std::os::raw::c_char, type_: acl_type_t) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_set_fd(fd: ::std::os::raw::c_int, acl: acl_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_file(
        path_p: *const ::std::os::raw::c_char,
        type_: acl_type_t,
        acl: acl_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_to_any_text(
        acl: acl_t,
        prefix: *const ::std::os::raw::c_char,
        separator: ::std::os::raw::c_char,
        options: ::std::os::raw::c_int,
    ) -> *mut ::std::os::raw::c_char;
}
unsafe extern "C" {
    pub fn acl_cmp(acl1: acl_t, acl2: acl_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_check(acl: acl_t, last: *mut ::std::os::raw::c_int) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_from_mode(mode: mode_t) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_equiv_mode(acl: acl_t, mode_p: *mut mode_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_extended_file(path_p: *const ::std::os::raw::c_char) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_extended_file_nofollow(
        path_p: *const ::std::os::raw::c_char,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_extended_fd(fd: ::std::os::raw::c_int) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_entries(acl: acl_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_error(code: ::std::os::raw::c_int) -> *const ::std::os::raw::c_char;
}
unsafe extern "C" {
    pub fn acl_get_perm(permset_d: acl_permset_t, perm: acl_perm_t) -> ::std::os::raw::c_int;
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct group {
    pub gr_name: *mut ::std::os::raw::c_char,
    pub gr_passwd: *mut ::std::os::raw::c_char,
    pub gr_gid: __gid_t,
    pub gr_mem: *mut *mut ::std::os::raw::c_char,
}
unsafe extern "C" {
    pub fn getgrgid_r(
        __gid: __gid_t,
        __resultbuf: *mut group,
        __buffer: *mut ::std::os::raw::c_char,
        __buflen: usize,
        __result: *mut *mut group,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn getgrnam_r(
        __name: *const ::std::os::raw::c_char,
        __resultbuf: *mut group,
        __buffer: *mut ::std::os::raw::c_char,
        __buflen: usize,
        __result: *mut *mut group,
    ) -> ::std::os::raw::c_int;
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct passwd {
    pub pw_name: *mut ::std::os::raw::c_char,
    pub pw_passwd: *mut ::std::os::raw::c_char,
    pub pw_uid: __uid_t,
    pub pw_gid: __gid_t,
    pub pw_gecos: *mut ::std::os::raw::c_char,
    pub pw_dir: *mut ::std::os::raw::c_char,
    pub pw_shell: *mut ::std::os::raw::c_char,
}
unsafe extern "C" {
    pub fn getpwuid_r(
        __uid: __uid_t,
        __resultbuf: *mut passwd,
        __buffer: *mut ::std::os::raw::c_char,
        __buflen: usize,
        __result: *mut *mut passwd,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn getpwnam_r(
        __name: *const ::std::os::raw::c_char,
        __resultbuf: *mut passwd,
        __buffer: *mut ::std::os::raw::c_char,
        __buflen: usize,
        __result: *mut *mut passwd,
    ) -> ::std::os::raw::c_int;
}
