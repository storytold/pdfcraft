pub const ENOENT: u32 = 2;
pub const ENOMEM: u32 = 12;
pub const EINVAL: u32 = 22;
pub const ERANGE: u32 = 34;
pub const ENOTSUP: u32 = 45;
pub const ACL_MAX_ENTRIES: u32 = 254;
pub const ACL_BRAND_UNKNOWN: u32 = 0;
pub const ACL_BRAND_POSIX: u32 = 1;
pub const ACL_BRAND_NFS4: u32 = 2;
pub const ACL_UNDEFINED_TAG: u32 = 0;
pub const ACL_USER_OBJ: u32 = 1;
pub const ACL_USER: u32 = 2;
pub const ACL_GROUP_OBJ: u32 = 4;
pub const ACL_GROUP: u32 = 8;
pub const ACL_MASK: u32 = 16;
pub const ACL_OTHER: u32 = 32;
pub const ACL_OTHER_OBJ: u32 = 32;
pub const ACL_EVERYONE: u32 = 64;
pub const ACL_ENTRY_TYPE_ALLOW: u32 = 256;
pub const ACL_ENTRY_TYPE_DENY: u32 = 512;
pub const ACL_ENTRY_TYPE_AUDIT: u32 = 1024;
pub const ACL_ENTRY_TYPE_ALARM: u32 = 2048;
pub const ACL_TYPE_ACCESS_OLD: u32 = 0;
pub const ACL_TYPE_DEFAULT_OLD: u32 = 1;
pub const ACL_TYPE_ACCESS: u32 = 2;
pub const ACL_TYPE_DEFAULT: u32 = 3;
pub const ACL_TYPE_NFS4: u32 = 4;
pub const ACL_EXECUTE: u32 = 1;
pub const ACL_WRITE: u32 = 2;
pub const ACL_READ: u32 = 4;
pub const ACL_PERM_NONE: u32 = 0;
pub const ACL_PERM_BITS: u32 = 7;
pub const ACL_POSIX1E_BITS: u32 = 7;
pub const ACL_READ_DATA: u32 = 8;
pub const ACL_LIST_DIRECTORY: u32 = 8;
pub const ACL_WRITE_DATA: u32 = 16;
pub const ACL_ADD_FILE: u32 = 16;
pub const ACL_APPEND_DATA: u32 = 32;
pub const ACL_ADD_SUBDIRECTORY: u32 = 32;
pub const ACL_READ_NAMED_ATTRS: u32 = 64;
pub const ACL_WRITE_NAMED_ATTRS: u32 = 128;
pub const ACL_DELETE_CHILD: u32 = 256;
pub const ACL_READ_ATTRIBUTES: u32 = 512;
pub const ACL_WRITE_ATTRIBUTES: u32 = 1024;
pub const ACL_DELETE: u32 = 2048;
pub const ACL_READ_ACL: u32 = 4096;
pub const ACL_WRITE_ACL: u32 = 8192;
pub const ACL_WRITE_OWNER: u32 = 16384;
pub const ACL_SYNCHRONIZE: u32 = 32768;
pub const ACL_FULL_SET: u32 = 65529;
pub const ACL_MODIFY_SET: u32 = 40953;
pub const ACL_READ_SET: u32 = 4680;
pub const ACL_WRITE_SET: u32 = 1200;
pub const ACL_NFS4_PERM_BITS: u32 = 65529;
pub const ACL_FIRST_ENTRY: u32 = 0;
pub const ACL_NEXT_ENTRY: u32 = 1;
pub const ACL_ENTRY_FILE_INHERIT: u32 = 1;
pub const ACL_ENTRY_DIRECTORY_INHERIT: u32 = 2;
pub const ACL_ENTRY_NO_PROPAGATE_INHERIT: u32 = 4;
pub const ACL_ENTRY_INHERIT_ONLY: u32 = 8;
pub const ACL_ENTRY_SUCCESSFUL_ACCESS: u32 = 16;
pub const ACL_ENTRY_FAILED_ACCESS: u32 = 32;
pub const ACL_ENTRY_INHERITED: u32 = 128;
pub const ACL_FLAGS_BITS: u32 = 191;
pub const ACL_TEXT_VERBOSE: u32 = 1;
pub const ACL_TEXT_NUMERIC_IDS: u32 = 2;
pub const ACL_TEXT_APPEND_ID: u32 = 4;
pub const _PC_ACL_NFS4: u32 = 64;
pub type __uint32_t = ::std::os::raw::c_uint;
pub type __int64_t = ::std::os::raw::c_long;
pub type __time_t = __int64_t;
pub type __gid_t = __uint32_t;
pub type __uid_t = __uint32_t;
pub type gid_t = __gid_t;
pub type time_t = __time_t;
pub type uid_t = __uid_t;
pub type acl_tag_t = u32;
pub type acl_perm_t = u32;
pub type acl_entry_type_t = u16;
pub type acl_flag_t = u16;
pub type acl_type_t = ::std::os::raw::c_int;
pub type acl_permset_t = *mut ::std::os::raw::c_int;
pub type acl_flagset_t = *mut u16;
pub type acl_entry_t = *mut ::std::os::raw::c_void;
pub type acl_t = *mut ::std::os::raw::c_void;
unsafe extern "C" {
    pub fn acl_add_flag_np(_flagset_d: acl_flagset_t, _flag: acl_flag_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_add_perm(_permset_d: acl_permset_t, _perm: acl_perm_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_calc_mask(_acl_p: *mut acl_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_clear_flags_np(_flagset_d: acl_flagset_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_clear_perms(_permset_d: acl_permset_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_copy_entry(_dest_d: acl_entry_t, _src_d: acl_entry_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_copy_ext(_buf_p: *mut ::std::os::raw::c_void, _acl: acl_t, _size: isize) -> isize;
}
unsafe extern "C" {
    pub fn acl_copy_int(_buf_p: *const ::std::os::raw::c_void) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_create_entry(
        _acl_p: *mut acl_t,
        _entry_p: *mut acl_entry_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_create_entry_np(
        _acl_p: *mut acl_t,
        _entry_p: *mut acl_entry_t,
        _index: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_delete_entry(_acl: acl_t, _entry_d: acl_entry_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_delete_entry_np(_acl: acl_t, _index: ::std::os::raw::c_int)
    -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_delete_fd_np(
        _filedes: ::std::os::raw::c_int,
        _type: acl_type_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_delete_file_np(
        _path_p: *const ::std::os::raw::c_char,
        _type: acl_type_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_delete_link_np(
        _path_p: *const ::std::os::raw::c_char,
        _type: acl_type_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_delete_def_file(_path_p: *const ::std::os::raw::c_char) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_delete_def_link_np(_path_p: *const ::std::os::raw::c_char) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_delete_flag_np(
        _flagset_d: acl_flagset_t,
        _flag: acl_flag_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_delete_perm(_permset_d: acl_permset_t, _perm: acl_perm_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_dup(_acl: acl_t) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_free(_obj_p: *mut ::std::os::raw::c_void) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_from_text(_buf_p: *const ::std::os::raw::c_char) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_get_brand_np(
        _acl: acl_t,
        _brand_p: *mut ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_entry(
        _acl: acl_t,
        _entry_id: ::std::os::raw::c_int,
        _entry_p: *mut acl_entry_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_fd(_fd: ::std::os::raw::c_int) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_get_fd_np(fd: ::std::os::raw::c_int, _type: acl_type_t) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_get_file(_path_p: *const ::std::os::raw::c_char, _type: acl_type_t) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_get_entry_type_np(
        _entry_d: acl_entry_t,
        _entry_type_p: *mut acl_entry_type_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_link_np(_path_p: *const ::std::os::raw::c_char, _type: acl_type_t) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_get_qualifier(_entry_d: acl_entry_t) -> *mut ::std::os::raw::c_void;
}
unsafe extern "C" {
    pub fn acl_get_flag_np(_flagset_d: acl_flagset_t, _flag: acl_flag_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_perm_np(_permset_d: acl_permset_t, _perm: acl_perm_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_flagset_np(
        _entry_d: acl_entry_t,
        _flagset_p: *mut acl_flagset_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_permset(
        _entry_d: acl_entry_t,
        _permset_p: *mut acl_permset_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_get_tag_type(
        _entry_d: acl_entry_t,
        _tag_type_p: *mut acl_tag_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_init(_count: ::std::os::raw::c_int) -> acl_t;
}
unsafe extern "C" {
    pub fn acl_set_fd(_fd: ::std::os::raw::c_int, _acl: acl_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_fd_np(
        _fd: ::std::os::raw::c_int,
        _acl: acl_t,
        _type: acl_type_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_file(
        _path_p: *const ::std::os::raw::c_char,
        _type: acl_type_t,
        _acl: acl_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_entry_type_np(
        _entry_d: acl_entry_t,
        _entry_type: acl_entry_type_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_link_np(
        _path_p: *const ::std::os::raw::c_char,
        _type: acl_type_t,
        _acl: acl_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_flagset_np(
        _entry_d: acl_entry_t,
        _flagset_d: acl_flagset_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_permset(
        _entry_d: acl_entry_t,
        _permset_d: acl_permset_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_qualifier(
        _entry_d: acl_entry_t,
        _tag_qualifier_p: *const ::std::os::raw::c_void,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_set_tag_type(_entry_d: acl_entry_t, _tag_type: acl_tag_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_size(_acl: acl_t) -> isize;
}
unsafe extern "C" {
    pub fn acl_to_text(_acl: acl_t, _len_p: *mut isize) -> *mut ::std::os::raw::c_char;
}
unsafe extern "C" {
    pub fn acl_to_text_np(
        _acl: acl_t,
        _len_p: *mut isize,
        _flags: ::std::os::raw::c_int,
    ) -> *mut ::std::os::raw::c_char;
}
unsafe extern "C" {
    pub fn acl_valid(_acl: acl_t) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_valid_fd_np(
        _fd: ::std::os::raw::c_int,
        _type: acl_type_t,
        _acl: acl_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_valid_file_np(
        _path_p: *const ::std::os::raw::c_char,
        _type: acl_type_t,
        _acl: acl_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_valid_link_np(
        _path_p: *const ::std::os::raw::c_char,
        _type: acl_type_t,
        _acl: acl_t,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_is_trivial_np(
        _acl: acl_t,
        _trivialp: *mut ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn acl_strip_np(_acl: acl_t, recalculate_mask: ::std::os::raw::c_int) -> acl_t;
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
    pub pw_change: time_t,
    pub pw_class: *mut ::std::os::raw::c_char,
    pub pw_gecos: *mut ::std::os::raw::c_char,
    pub pw_dir: *mut ::std::os::raw::c_char,
    pub pw_shell: *mut ::std::os::raw::c_char,
    pub pw_expire: time_t,
    pub pw_fields: ::std::os::raw::c_int,
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
    pub fn getpwuid_r(
        arg1: uid_t,
        arg2: *mut passwd,
        arg3: *mut ::std::os::raw::c_char,
        arg4: usize,
        arg5: *mut *mut passwd,
    ) -> ::std::os::raw::c_int;
}
unsafe extern "C" {
    pub fn fpathconf(
        arg1: ::std::os::raw::c_int,
        arg2: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_long;
}
unsafe extern "C" {
    pub fn pathconf(
        arg1: *const ::std::os::raw::c_char,
        arg2: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_long;
}
unsafe extern "C" {
    pub fn lpathconf(
        arg1: *const ::std::os::raw::c_char,
        arg2: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_long;
}
