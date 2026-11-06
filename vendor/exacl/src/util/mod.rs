//! Provides a cross-platform, minimal ACL API.
//!
//! Types:
//!    `acl_t`
//!    `acl_entry_t`
//!
//! Functions:
//!    `xacl_init`      - create a new empty ACL
//!    `xacl_free`      - destroy ACL
//!    `xacl_foreach`   - apply a function to each entry in an ACL
//!    `xacl_is_empty`  - return true if an ACL is empty
//!    `xacl_is_posix`  - return true if ACL has Posix.1e semantics.
//!    `xacl_add_entry` - append new entry to an ACL
//!    `xacl_get_entry` - retrieve contents from an ACL entry
//!    `xacl_get_file`  - get ACL from file path
//!    `xacl_set_file`  - set ACL for file path
//!    `xacl_get_fd`    - get ACL for file descriptor
//!    `xacl_set_fd`    - set ACL for file descriptor
//!    `xacl_is_nfs4`   - return true if file path uses `NFSv4` ACL on `FreeBSD`

mod util_common;

#[cfg(target_os = "freebsd")]
mod util_freebsd;

#[cfg(target_os = "linux")]
mod util_linux;

#[cfg(target_os = "macos")]
mod util_macos;

// Re-export acl_entry_t and acl_t from crate::sys.
pub use crate::sys::{acl_entry_t, acl_t};

#[cfg(target_os = "freebsd")]
pub use util_freebsd::{
    xacl_add_entry, xacl_foreach, xacl_free, xacl_get_entry, xacl_get_fd, xacl_get_file, xacl_init,
    xacl_is_empty, xacl_is_nfs4, xacl_is_posix, xacl_set_fd, xacl_set_file,
};

#[cfg(target_os = "linux")]
pub use util_linux::{
    xacl_add_entry, xacl_foreach, xacl_free, xacl_get_entry, xacl_get_fd, xacl_get_file, xacl_init,
    xacl_is_empty, xacl_is_posix, xacl_set_fd, xacl_set_file,
};

#[cfg(target_os = "macos")]
pub use util_macos::{
    xacl_add_entry, xacl_foreach, xacl_free, xacl_get_entry, xacl_get_fd, xacl_get_file, xacl_init,
    xacl_is_empty, xacl_is_posix, xacl_set_fd, xacl_set_file,
};
