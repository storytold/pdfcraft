pub const ENOENT: u32 = 2;
pub const ENOMEM: u32 = 12;
pub const EINVAL: u32 = 22;
pub const ERANGE: u32 = 34;
pub const ENOTSUP: u32 = 45;
pub const ACL_MAX_ENTRIES: u32 = 128;
pub const O_SYMLINK: u32 = 2097152;
pub const ID_TYPE_UID: u32 = 0;
pub const ID_TYPE_GID: u32 = 1;
pub type __uint32_t = ::std::os::raw::c_uint;
pub type __darwin_time_t = ::std::os::raw::c_long;
pub type u_int64_t = ::std::os::raw::c_ulonglong;
pub type __darwin_gid_t = __uint32_t;
pub type __darwin_id_t = __uint32_t;
pub type __darwin_uid_t = __uint32_t;
pub type __darwin_uuid_t = [::std::os::raw::c_uchar; 16usize];
pub type gid_t = __darwin_gid_t;
pub type id_t = __darwin_id_t;
pub type uid_t = __darwin_uid_t;
pub const acl_perm_t_ACL_READ_DATA: acl_perm_t = 2;
pub const acl_perm_t_ACL_LIST_DIRECTORY: acl_perm_t = 2;
pub const acl_perm_t_ACL_WRITE_DATA: acl_perm_t = 4;
pub const acl_perm_t_ACL_ADD_FILE: acl_perm_t = 4;
pub const acl_perm_t_ACL_EXECUTE: acl_perm_t = 8;
pub const acl_perm_t_ACL_SEARCH: acl_perm_t = 8;
pub const acl_perm_t_ACL_DELETE: acl_perm_t = 16;
pub const acl_perm_t_ACL_APPEND_DATA: acl_perm_t = 32;
pub const acl_perm_t_ACL_ADD_SUBDIRECTORY: acl_perm_t = 32;
pub const acl_perm_t_ACL_DELETE_CHILD: acl_perm_t = 64;
pub const acl_perm_t_ACL_READ_ATTRIBUTES: acl_perm_t = 128;
pub const acl_perm_t_ACL_WRITE_ATTRIBUTES: acl_perm_t = 256;
pub const acl_perm_t_ACL_READ_EXTATTRIBUTES: acl_perm_t = 512;
pub const acl_perm_t_ACL_WRITE_EXTATTRIBUTES: acl_perm_t = 1024;
pub const acl_perm_t_ACL_READ_SECURITY: acl_perm_t = 2048;
pub const acl_perm_t_ACL_WRITE_SECURITY: acl_perm_t = 4096;
pub const acl_perm_t_ACL_CHANGE_OWNER: acl_perm_t = 8192;
pub const acl_perm_t_ACL_SYNCHRONIZE: acl_perm_t = 1048576;
pub type acl_perm_t = ::std::os::raw::c_uint;
pub const acl_tag_t_ACL_UNDEFINED_TAG: acl_tag_t = 0;
pub const acl_tag_t_ACL_EXTENDED_ALLOW: acl_tag_t = 1;
pub const acl_tag_t_ACL_EXTENDED_DENY: acl_tag_t = 2;
pub type acl_tag_t = ::std::os::raw::c_uint;
pub const acl_type_t_ACL_TYPE_EXTENDED: acl_type_t = 256;
pub const acl_type_t_ACL_TYPE_ACCESS: acl_type_t = 0;
pub const acl_type_t_ACL_TYPE_DEFAULT: acl_type_t = 1;
pub const acl_type_t_ACL_TYPE_AFS: acl_type_t = 2;
pub const acl_type_t_ACL_TYPE_CODA: acl_type_t = 3;
pub const acl_type_t_ACL_TYPE_NTFS: acl_type_t = 4;
pub const acl_type_t_ACL_TYPE_NWFS: acl_type_t = 5;
pub type acl_type_t = ::std::os::raw::c_uint;
pub const acl_entry_id_t_ACL_FIRST_ENTRY: acl_entry_id_t = 0;
pub const acl_entry_id_t_ACL_NEXT_ENTRY: acl_entry_id_t = -1;
pub const acl_entry_id_t_ACL_LAST_ENTRY: acl_entry_id_t = -2;
pub type acl_entry_id_t = ::std::os::raw::c_int;
pub const acl_flag_t_ACL_FLAG_DEFER_INHERIT: acl_flag_t = 1;
pub const acl_flag_t_ACL_FLAG_NO_INHERIT: acl_flag_t = 131072;
pub const acl_flag_t_ACL_ENTRY_INHERITED: acl_flag_t = 16;
pub const acl_flag_t_ACL_ENTRY_FILE_INHERIT: acl_flag_t = 32;
pub const acl_flag_t_ACL_ENTRY_DIRECTORY_INHERIT: acl_flag_t = 64;
pub const acl_flag_t_ACL_ENTRY_LIMIT_INHERIT: acl_flag_t = 128;
pub const acl_flag_t_ACL_ENTRY_ONLY_INHERIT: acl_flag_t = 256;
pub type acl_flag_t = ::std::os::raw::c_uint;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _acl {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _acl_entry {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _acl_permset {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _acl_flagset {
    _unused: [u8; 0],
}
pub type acl_t = *mut _acl;
pub type acl_entry_t = *mut _acl_entry;
pub type acl_permset_t = *mut _acl_permset;
pub type acl_flagset_t = *mut _acl_flagset;
pub type acl_permset_mask_t = u_int64_t;
unsafe extern "C" {
    pub fn acl_dup(acl: acl_t) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_free(obj_p: *mut ::std::os::raw::c_void) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_init(count: ::std::os::raw::c_int) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_copy_entry(dest_d: acl_entry_t, src_d: acl_entry_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_create_entry(acl_p: *mut acl_t, entry_p: *mut acl_entry_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_create_entry_np(
        acl_p: *mut acl_t,
        entry_p: *mut acl_entry_t,
        entry_index: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
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
    pub fn acl_valid(acl: acl_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_valid_fd_np(
        fd: ::std::os::raw::c_int,
        type_: acl_type_t,
        acl: acl_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_valid_file_np(
        path: *const ::std::os::raw::c_char,
        type_: acl_type_t,
        acl: acl_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_valid_link_np(
        path: *const ::std::os::raw::c_char,
        type_: acl_type_t,
        acl: acl_t,
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
    pub fn acl_get_perm_np(permset_d: acl_permset_t, perm: acl_perm_t) -> ::std::os::raw::c_int;
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
    pub fn acl_maximal_permset_mask_np(mask_p: *mut acl_permset_mask_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_permset_mask_np(
        entry_d: acl_entry_t,
        mask_p: *mut acl_permset_mask_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_permset_mask_np(
        entry_d: acl_entry_t,
        mask: acl_permset_mask_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_add_flag_np(flagset_d: acl_flagset_t, flag: acl_flag_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_clear_flags_np(flagset_d: acl_flagset_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_delete_flag_np(flagset_d: acl_flagset_t, flag: acl_flag_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_flag_np(flagset_d: acl_flagset_t, flag: acl_flag_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_flagset_np(
        obj_p: *mut ::std::os::raw::c_void,
        flagset_p: *mut acl_flagset_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_flagset_np(
        obj_p: *mut ::std::os::raw::c_void,
        flagset_d: acl_flagset_t,
    ) -> ::std::os::raw::c_int;
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
    pub fn acl_delete_def_file(path_p: *const ::std::os::raw::c_char) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_fd(fd: ::std::os::raw::c_int) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_get_fd_np(fd: ::std::os::raw::c_int, type_: acl_type_t) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_get_file(path_p: *const ::std::os::raw::c_char, type_: acl_type_t) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_get_link_np(path_p: *const ::std::os::raw::c_char, type_: acl_type_t) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_set_fd(fd: ::std::os::raw::c_int, acl: acl_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_fd_np(
        fd: ::std::os::raw::c_int,
        acl: acl_t,
        acl_type: acl_type_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_file(
        path_p: *const ::std::os::raw::c_char,
        type_: acl_type_t,
        acl: acl_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_link_np(
        path_p: *const ::std::os::raw::c_char,
        type_: acl_type_t,
        acl: acl_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_copy_ext(buf_p: *mut ::std::os::raw::c_void, acl: acl_t, size: isize) -> isize;
}
unsafe extern "C" {
    pub fn acl_copy_ext_native(
        buf_p: *mut ::std::os::raw::c_void,
        acl: acl_t,
        size: isize,
    ) -> isize;
}
unsafe extern "C" {
    pub fn acl_copy_int(buf_p: *const ::std::os::raw::c_void) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_copy_int_native(buf_p: *const ::std::os::raw::c_void) -> acl_t;
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
    pub fn open(
        arg1: *const ::std::os::raw::c_char,
        arg2: ::std::os::raw::c_int,
        ...
    ) -> ::std::os::raw::c_int;
}
pub type uuid_t = __darwin_uuid_t;
unsafe extern "C" {
    pub fn mbr_uid_to_uuid(uid: uid_t, uu: *mut ::std::os::raw::c_uchar) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn mbr_gid_to_uuid(gid: gid_t, uu: *mut ::std::os::raw::c_uchar) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn mbr_uuid_to_id(
        uu: *const ::std::os::raw::c_uchar,
        uid_or_gid: *mut id_t,
        id_type: *mut ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct group {
    pub gr_name: *mut ::std::os::raw::c_char,
    pub gr_passwd: *mut ::std::os::raw::c_char,
    pub gr_gid: gid_t,
    pub gr_mem: *mut *mut ::std::os::raw::c_char,
}
unsafe extern "C" {
    pub fn getgrgid_r(
        arg1: gid_t,
        arg2: *mut group,
        arg3: *mut ::std::os::raw::c_char,
        arg4: usize,
        arg5: *mut *mut group,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn getgrnam_r(
        arg1: *const ::std::os::raw::c_char,
        arg2: *mut group,
        arg3: *mut ::std::os::raw::c_char,
        arg4: usize,
        arg5: *mut *mut group,
    ) -> ::std::os::raw::c_int;
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct passwd {
    pub pw_name: *mut ::std::os::raw::c_char,
    pub pw_passwd: *mut ::std::os::raw::c_char,
    pub pw_uid: uid_t,
    pub pw_gid: gid_t,
    pub pw_change: __darwin_time_t,
    pub pw_class: *mut ::std::os::raw::c_char,
    pub pw_gecos: *mut ::std::os::raw::c_char,
    pub pw_dir: *mut ::std::os::raw::c_char,
    pub pw_shell: *mut ::std::os::raw::c_char,
    pub pw_expire: __darwin_time_t,
}
unsafe extern "C" {
    pub fn getpwuid_r(
        arg1: uid_t,
        arg2: *mut passwd,
        arg3: *mut ::std::os::raw::c_char,
        arg4: usize,
        arg5: *mut *mut passwd,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn getpwnam_r(
        arg1: *const ::std::os::raw::c_char,
        arg2: *mut passwd,
        arg3: *mut ::std::os::raw::c_char,
        arg4: usize,
        arg5: *mut *mut passwd,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn close(arg1: ::std::os::raw::c_int) -> ::std::os::raw::c_int;
}
