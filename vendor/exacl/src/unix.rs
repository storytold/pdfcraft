//! Implements utilities for converting user/group names to uid/gid.
//
// This module supports a `_LABTEST` cfg flag that adds some synthetic
// name/uid mappings. These are designed to catch some edge cases in testing.
// To use the _LABTEST cfg flag, the code has to be explicitly compiled in
// debug mode with `RUSTFLAGS="--cfg _LABTEST"`. You can detect code is
// compiled with _LABTEST by using the 🧪 emoji as a user name. The group name
// mapping is tested on FreeBSD by adding the group name manually to /etc/group.
//
//   USER NAME                                UID
//   ---------                                ------
//   "\u{1F9EA}"  (test tube emoji)           777771
//   "777772"                                 777773
//   "fbbc14b7-95f9-47a7-8ee8-1cccb9220943"   777774
//
//   GROUP NAME                               GID
//   ----------                               ------
//   é (non-UTF8 bytes; latin1 encoding)      777775

use crate::failx::*;
use crate::sys::{getgrgid_r, getgrnam_r, getpwnam_r, getpwuid_r, group, passwd, sg};
#[cfg(target_os = "macos")]
use crate::sys::{id_t, mbr_gid_to_uuid, mbr_uid_to_uuid, mbr_uuid_to_id};

use std::ffi::{CStr, CString};
use std::io;
use std::mem;
use std::os::raw::c_char;
use std::ptr;
#[cfg(target_os = "macos")]
use uuid::Uuid;

// Export uid_t and gid_t.
pub use crate::sys::{gid_t, uid_t};

// Max buffer sizes for getpwnam_r, getgrnam_r, et al. are usually determined
// by calling sysconf with SC_GETPW_R_SIZE_MAX or SC_GETGR_R_SIZE_MAX. Rather
// than calling sysconf, this code hard-wires the default value and quadruples
// the buffer size as needed, up to a maximum of 1MB.
//
// SC_GETPW_R_SIZE_MAX/SC_GETGR_R_SIZE_MAX default to 1024 on vanilla Ubuntu
// and 4096 on macOS/FreeBSD. We start the initial buffer size at 4096 bytes.

const INITIAL_BUFSIZE: usize = 4096; // 4KB
const MAX_BUFSIZE: usize = 1_048_576; // 1MB

// A name that starts with a left curly brace '{' is treated differently
// by `name_to_uid` and `name_to_gid`. It represents an underlying GUID or
// SID.

const CURLY_BRACE: char = '{';

/// Convert user name to uid.
///
/// 1. If `name` begins with a curly brace, stop and return failure.
/// 2. If `name` is a valid decimal number [0..2^32-1], return it as uid.
/// 3. Look up `name` using `getpwnam` and return uid if found.
/// 4. Otherwise, stop and return failure.
pub fn name_to_uid(name: &str) -> io::Result<uid_t> {
    if !name.starts_with(CURLY_BRACE) {
        // Try to parse name as a decimal user ID.
        if let Ok(uid) = name.parse::<uid_t>() {
            return Ok(uid);
        }
        // Lookup user name using `getpwnam_r`.
        if let Some(uid) = getpwnam(name)? {
            return Ok(uid);
        }
    }

    fail_custom(&format!("unknown user name: {name:?}"))
}

/// Convert user name to uid using `getpwnam_r` library call.
fn getpwnam(name: &str) -> io::Result<Option<uid_t>> {
    let mut pwd = mem::MaybeUninit::<passwd>::uninit();
    let mut buf = Vec::<c_char>::with_capacity(INITIAL_BUFSIZE);
    let mut result = ptr::null_mut();
    let cstr = CString::new(name)?;

    let mut ret;
    loop {
        ret = unsafe {
            getpwnam_r(
                cstr.as_ptr(),
                pwd.as_mut_ptr(),
                buf.as_mut_ptr(),
                buf.capacity(),
                &raw mut result,
            )
        };

        if ret == 0 || ret != sg::ERANGE || buf.capacity() >= MAX_BUFSIZE {
            break;
        }

        // Quadruple buffer size and try again.
        buf.reserve(4 * buf.capacity());
    }

    if ret != 0 {
        return fail_err(ret, "getpwnam_r", name);
    }

    if result.is_null() {
        #[cfg(_LABTEST)]
        return Ok(labtest_mock_getpwnam(name));

        #[cfg(not(_LABTEST))]
        return Ok(None);
    }

    let uid = unsafe { pwd.assume_init().pw_uid };
    Ok(Some(uid))
}

/// Mock additional name -> uid mappings for _LABTEST.
#[cfg(all(debug_assertions, _LABTEST))]
fn labtest_mock_getpwnam(name: &str) -> Option<uid_t> {
    match name {
        "\u{1F9EA}" => Some(777_771),
        "777772" => Some(777_773),
        "fbbc14b7-95f9-47a7-8ee8-1cccb9220943" => Some(777_774),
        _ => None,
    }
}

/// Convert group name to gid.
///
/// 1. If `name` begins with a curly brace, stop and return failure.
/// 2. If `name` is a valid decimal number [0..2^32-1], return it as gid.
/// 3. Look up `name` using `getgrnam` and return gid if found.
/// 4. Otherwise, stop and return failure.
pub fn name_to_gid(name: &str) -> io::Result<gid_t> {
    if !name.starts_with(CURLY_BRACE) {
        // Try to parse name as a decimal group ID.
        if let Ok(gid) = name.parse::<gid_t>() {
            return Ok(gid);
        }
        // Lookup group name using `getgrnam_r`.
        if let Some(gid) = getgrnam(name)? {
            return Ok(gid);
        }
    }

    fail_custom(&format!("unknown group name: {name:?}"))
}

/// Convert group name to gid using `getgrnam_r` library call.
fn getgrnam(name: &str) -> io::Result<Option<gid_t>> {
    let mut grp = mem::MaybeUninit::<group>::uninit();
    let mut buf = Vec::<c_char>::with_capacity(INITIAL_BUFSIZE);
    let mut result = ptr::null_mut();
    let cstr = CString::new(name)?;

    let mut ret;
    loop {
        ret = unsafe {
            getgrnam_r(
                cstr.as_ptr(),
                grp.as_mut_ptr(),
                buf.as_mut_ptr(),
                buf.capacity(),
                &raw mut result,
            )
        };

        if ret == 0 || ret != sg::ERANGE || buf.capacity() >= MAX_BUFSIZE {
            break;
        }

        // Quadruple buffer size and try again.
        buf.reserve(4 * buf.capacity());
    }

    if ret != 0 {
        return fail_err(ret, "getgrnam_r", name);
    }

    if result.is_null() {
        Ok(None)
    } else {
        let gid = unsafe { grp.assume_init().gr_gid };
        Ok(Some(gid))
    }
}

/// Convert uid to user name.
///
/// If we look up the user name and it doesn't exist, return the original `uid`
/// as a decimal string.
///
/// If we look up the user name but it's is a decimal number that could be
/// confused for a `uid` or it starts with '{', return the original `uid` as a
/// decimal string.
pub fn uid_to_name(uid: uid_t) -> io::Result<String> {
    if let Some(name) = getpwuid(uid)?
        && name.parse::<uid_t>().is_err()
        && !name.starts_with(CURLY_BRACE)
    {
        return Ok(name);
    }

    Ok(uid.to_string())
}

/// Convert uid to name using `getpwuid_r` library call.
fn getpwuid(uid: uid_t) -> io::Result<Option<String>> {
    let mut pwd = mem::MaybeUninit::<passwd>::uninit();
    let mut buf = Vec::<c_char>::with_capacity(INITIAL_BUFSIZE);
    let mut result = ptr::null_mut();

    let mut ret;
    loop {
        ret = unsafe {
            getpwuid_r(
                uid,
                pwd.as_mut_ptr(),
                buf.as_mut_ptr(),
                buf.capacity(),
                &raw mut result,
            )
        };

        if ret == 0 || ret != sg::ERANGE || buf.capacity() >= MAX_BUFSIZE {
            break;
        }

        // Quadruple buffer size and try again.
        buf.reserve(4 * buf.capacity());
    }

    if ret != 0 {
        return fail_err(ret, "getpwuid_r", uid);
    }

    if result.is_null() {
        #[cfg(_LABTEST)]
        return Ok(labtest_mock_getpwuid(uid));

        #[cfg(not(_LABTEST))]
        return Ok(None);
    }

    // A Rust `String` can only store valid UTF-8. It is possible for the
    // system to return a C String that contains non-UTF-8 bytes. When this
    // happens we return None, as if the name does not exist.
    let cstr = unsafe { CStr::from_ptr(pwd.assume_init().pw_name) };
    let name = cstr.to_str().ok();
    Ok(name.map(std::convert::Into::into))
}

/// Mock additional uid -> name mappings for _LABTEST.
#[cfg(all(debug_assertions, _LABTEST))]
fn labtest_mock_getpwuid(uid: uid_t) -> Option<String> {
    match uid {
        777_771 => Some("\u{1F9EA}".into()),
        777_773 => Some("777772".into()),
        777_774 => Some("fbbc14b7-95f9-47a7-8ee8-1cccb9220943".into()),
        _ => None,
    }
}

/// Convert gid to group name.
///
/// If we look up the group name and it doesn't exist, return the original `gid`
/// as a decimal string.
///
/// If we look up the group name but it's is a decimal number that could be
/// confused for a `gid` or it starts with '{', return the original `gid` as a
/// decimal string.
pub fn gid_to_name(gid: gid_t) -> io::Result<String> {
    if let Some(name) = getgrgid(gid)?
        && name.parse::<gid_t>().is_err()
        && !name.starts_with(CURLY_BRACE)
    {
        return Ok(name);
    }

    Ok(gid.to_string())
}

/// Convert gid to name using `getgruid_r` library call.
fn getgrgid(gid: gid_t) -> io::Result<Option<String>> {
    let mut grp = mem::MaybeUninit::<group>::uninit();
    let mut buf = Vec::<c_char>::with_capacity(INITIAL_BUFSIZE);
    let mut result = ptr::null_mut();

    let mut ret;
    loop {
        ret = unsafe {
            getgrgid_r(
                gid,
                grp.as_mut_ptr(),
                buf.as_mut_ptr(),
                buf.capacity(),
                &raw mut result,
            )
        };

        if ret == 0 || ret != sg::ERANGE || buf.capacity() >= MAX_BUFSIZE {
            break;
        }

        // Quadruple buffer size and try again.
        buf.reserve(4 * buf.capacity());
    }

    if ret != 0 {
        return fail_err(ret, "getgrgid_r", gid);
    }

    if result.is_null() {
        return Ok(None);
    }

    // A Rust `String` can only store valid UTF-8. It is possible for the
    // system to return a C String that contains non-UTF-8 bytes. When this
    // happens we return None, as if the name does not exist.
    let cstr = unsafe { CStr::from_ptr(grp.assume_init().gr_name) };
    let name = cstr.to_str().ok();
    Ok(name.map(std::convert::Into::into))
}

/// Parse a user/group name as a UUID. The UUID must be in curly braces.
/// It must be in hyphenated format.
#[cfg(target_os = "macos")]
pub fn name_to_guid(name: &str) -> Option<Uuid> {
    if name.starts_with(CURLY_BRACE) {
        // `Uuid::try_parse` checks for the closing brace.
        Uuid::try_parse(name).ok()
    } else {
        None
    }
}

/// Convert uid to GUID.
#[cfg(target_os = "macos")]
pub fn uid_to_guid(uid: uid_t) -> io::Result<Uuid> {
    let mut bytes = [0u8; 16];

    // On error, returns one of {EIO, ENOENT, EAUTH, EINVAL, ENOMEM}.
    let ret = unsafe { mbr_uid_to_uuid(uid, bytes.as_mut_ptr()) };
    if ret != 0 {
        return fail_from_err(ret, "mbr_uid_to_uuid", uid);
    }

    Ok(Uuid::from_bytes(bytes))
}

/// Convert gid to GUID.
#[cfg(target_os = "macos")]
pub fn gid_to_guid(gid: gid_t) -> io::Result<Uuid> {
    let mut bytes = [0u8; 16];

    // On error, returns one of {EIO, ENOENT, EAUTH, EINVAL, ENOMEM}.
    let ret = unsafe { mbr_gid_to_uuid(gid, bytes.as_mut_ptr()) };
    if ret != 0 {
        return fail_from_err(ret, "mbr_gid_to_uuid", gid);
    }

    Ok(Uuid::from_bytes(bytes))
}

/// Convert GUID to uid/gid.
///
/// Returns a pair of options (Option[uid], Option[gid]). Either one option must
/// be set or neither is set. If neither is set, the GUID was not found.
#[cfg(target_os = "macos")]
pub fn guid_to_id(guid: Uuid) -> io::Result<(Option<uid_t>, Option<gid_t>)> {
    let mut id_c: id_t = 0;
    let mut idtype: i32 = 0;
    let mut bytes = guid.into_bytes();

    // On error, returns one of {EIO, ENOENT, EAUTH, EINVAL, ENOMEM}.
    let ret = unsafe { mbr_uuid_to_id(bytes.as_mut_ptr(), &raw mut id_c, &raw mut idtype) };
    if ret == sg::ENOENT {
        // GUID was not found.
        return Ok((None, None));
    }

    if ret != 0 {
        return fail_from_err(ret, "mbr_uuid_to_id", guid);
    }

    let result = match idtype {
        sg::ID_TYPE_UID => (Some(id_c), None),
        sg::ID_TYPE_GID => (None, Some(id_c)),
        _ => {
            return fail_custom(&format!(
                "mbr_uuid_to_id: Unknown idtype {idtype:?} for guid {guid:?}"
            ));
        }
    };

    Ok(result)
}

////////////////////////////////////////////////////////////////////////////////

/// The `unix::helper` module contains helper functions for test code only.
#[cfg(test)]
pub mod helper {
    /// Init logging for tests.
    pub fn init_logging() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    /// Retrieve `user_id` and `group_id` of unix user with specified name.
    /// Equivalent to running `getent passwd NAME`.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn getent(name: &str) -> (u32, u32) {
        use std::str::FromStr;

        let cmd = std::process::Command::new("getent")
            .args(["passwd", name])
            .output()
            .expect("failed to execute getent");

        if !cmd.status.success() {
            let error = String::from_utf8(cmd.stderr).expect("invalid utf8");
            panic!("getent failed ({name}): {error:?}")
        }

        // Expected output:
        // ```
        // NAME:x:UID:GID:...
        // ```

        let out = String::from_utf8(cmd.stdout).expect("invalid utf8");
        let tokens = out.split(':').collect::<Vec<_>>();
        assert_eq!(tokens[0], name);
        let user_id = u32::from_str(tokens[2]).expect("invalid uid");
        let group_id = u32::from_str(tokens[3]).expect("invalid gid");

        (user_id, group_id)
    }

    /// Retrieve `user_id` and `group_id` of unix entity with specified name.
    ///  `dscl . -read /Users/NAME UniqueID PrimaryGroupID`
    #[cfg(target_os = "macos")]
    pub fn getent(name: &str) -> (u32, u32) {
        use std::str::FromStr;

        let cmd = std::process::Command::new("dscl")
            .args([
                ".",
                "-read",
                &format!("/Users/{name}"),
                "UniqueID",
                "PrimaryGroupID",
            ])
            .output()
            .expect("failed to execute dscl");

        if !cmd.status.success() {
            let error = String::from_utf8(cmd.stderr).expect("invalid utf8");
            panic!("dscl failed ({name}): {error:?}")
        }

        // Expected output:
        // ```
        // PrimaryGroupID: GID
        // UniqueID: UID
        // ```

        let out = String::from_utf8(cmd.stdout).expect("invalid utf8");
        let mut group_id = None;
        let mut user_id = None;

        for line in out.lines() {
            // Split token pair on space character.
            let tokens = line.split_once(' ').unwrap();
            match tokens.0 {
                "PrimaryGroupID:" => group_id = Some(u32::from_str(tokens.1).expect("invalid gid")),
                "UniqueID:" => user_id = Some(u32::from_str(tokens.1).expect("invalid uid")),
                _ => (),
            }
        }

        (user_id.expect("no uid"), group_id.expect("no gid"))
    }

    /// Return a valid username/uid pair.
    pub fn valid_user() -> (String, u32) {
        let name = if cfg!(target_os = "macos") {
            "_spotlight"
        } else {
            "daemon"
        };

        let (uid, _) = getent(name);
        (name.to_string(), uid)
    }

    /// Return a valid groupname/gid pair.
    pub fn valid_group() -> (String, u32) {
        let name = if cfg!(target_os = "macos") {
            "_spotlight"
        } else {
            "daemon"
        };

        let (_, gid) = getent(name);
        (name.to_string(), gid)
    }

    /// Return an anonymous (no name) uid.
    pub fn anonymous_uid() -> u32 {
        14987
    }

    /// Return an anonymous (no name) gid.
    pub fn anonymous_gid() -> u32 {
        14988
    }

    /// Test the `getent` test function.
    #[test]
    fn test_getent() {
        let user = "daemon";
        let result = getent(user);
        eprintln!("getent: {user} = {result:?}");

        #[cfg(target_os = "macos")]
        {
            let user = "_spotlight";
            let result = getent(user);
            eprintln!("getent: {user} = {result:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Test invalid name in `name_to_uid`, `name_to_gid`, `getpwnam`, `getgrnam`.
    #[test]
    fn test_invalid_name() -> TestResult {
        helper::init_logging();

        let invalid_names = [
            "",
            "non_existant",
            "-1",         // negative
            " root",      // begins with space
            " 500",       // begins with space
            "4294967296", // 2**32 is too big
            "\u{1F600}",  // grinning face emoji
            ":",
            ",",
            "{",
        ];

        for name in invalid_names {
            let err = name_to_uid(name).unwrap_err();
            assert_eq!(err.to_string(), format!("unknown user name: \"{name}\""));
        }

        for name in invalid_names {
            let err = name_to_gid(name).unwrap_err();
            assert_eq!(err.to_string(), format!("unknown group name: \"{name}\""));
        }

        for name in invalid_names {
            let result = getpwnam(name)?;
            assert_eq!(result, None);
        }

        for name in invalid_names {
            let result = getgrnam(name)?;
            assert_eq!(result, None);
        }

        // Test invalid user name with quote.
        let err = name_to_uid("\"").unwrap_err();
        assert_eq!(err.to_string(), "unknown user name: \"\\\"\"");

        // Test invalid group name with quote.
        let err = name_to_gid("\"").unwrap_err();
        let msg = err.to_string();
        if cfg!(target_os = "linux") {
            // May trigger NSS backend error on Linux (EIO).
            assert!(
                (msg == "Input/output error (os error 5)")
                    || (msg == "unknown group name: \"\\\"\"")
            );
        } else {
            assert_eq!(msg, "unknown group name: \"\\\"\"");
        }

        // Test invalid user name with newline.
        let err = name_to_uid("\n").unwrap_err();
        assert_eq!(err.to_string(), "unknown user name: \"\\n\"");

        // Test invalid group name with newline.
        let err = name_to_gid("\n").unwrap_err();
        assert_eq!(err.to_string(), "unknown group name: \"\\n\"");

        Ok(())
    }

    /// Test valid numeric user name in `name_to_uid`, `name_to_gid`.
    #[test]
    fn test_numeric_name() -> TestResult {
        helper::init_logging();

        let numeric_names = ["500", "0", "1", "4294967295", "000004294967295"];

        for name in numeric_names {
            let result = name_to_uid(name)?;
            assert_eq!(result, name.parse()?);
        }

        for name in numeric_names {
            let result = name_to_gid(name)?;
            assert_eq!(result, name.parse()?);
        }

        for name in numeric_names {
            let result = getpwnam(name)?;
            assert_eq!(result, None);
        }

        for name in numeric_names {
            let result = getgrnam(name)?;
            assert_eq!(result, None);
        }

        Ok(())
    }

    /// Test valid user name in `name_to_uid`, `uid_to_name`.
    #[test]
    fn test_valid_user() -> TestResult {
        helper::init_logging();

        let (name, uid) = helper::valid_user();

        let result = name_to_uid(&name)?;
        assert_eq!(result, uid);

        let result = uid_to_name(uid)?;
        assert_eq!(result, name);

        Ok(())
    }

    /// Test anonymous uid in `uid_to_name`.
    #[test]
    fn test_anonymous_uid() -> TestResult {
        helper::init_logging();

        let uid = helper::anonymous_uid();

        let result = uid_to_name(uid)?;
        assert_eq!(result, uid.to_string());

        Ok(())
    }

    /// Test valid group name in `name_to_gid`, `gid_to_name`.
    #[test]
    fn test_valid_group() -> TestResult {
        helper::init_logging();

        let (name, gid) = helper::valid_group();

        let result = name_to_gid(&name)?;
        assert_eq!(result, gid);

        let result = gid_to_name(gid)?;
        assert_eq!(result, name);

        Ok(())
    }

    /// Test anonymous gid in `gid_to_name`.
    #[test]
    fn test_anonymous_gid() -> TestResult {
        helper::init_logging();

        let gid = helper::anonymous_gid();

        let result = gid_to_name(gid)?;
        assert_eq!(result, gid.to_string());

        Ok(())
    }

    /// Test `name_to_guid` converts name to a guid.
    #[test]
    #[cfg(target_os = "macos")]
    fn test_name_to_guid() -> TestResult {
        helper::init_logging();

        let test_cases = [
            ("{00000000-0000-0000-0000-000000000000}", Some(Uuid::nil())),
            ("{00000000000000000000000000000000}", None), // must be hypenated
            ("00000000000000000000000000000000", None),   // braces required
            ("00000000-0000-0000-0000-000000000000", None), // braces required
            ("{00000000-0000-0000-0000-000000000000", None), // braces required
            (
                "{abcdefab-cdef-abcd-efab-cdef00000059}",
                Some(Uuid::parse_str("abcdefab-cdef-abcd-efab-cdef00000059")?),
            ),
        ];

        for (name, value) in test_cases {
            let guid = name_to_guid(name);
            assert_eq!(guid, value, "name=`{name}`");
        }

        Ok(())
    }

    /// Test `uid_to_guid` converts uid to guid.
    #[test]
    #[cfg(target_os = "macos")]
    fn test_uid_to_guid() -> TestResult {
        helper::init_logging();

        let uids = [
            (89u32, "ffffeeee-dddd-cccc-bbbb-aaaa00000059"),
            (1500, "ffffeeee-dddd-cccc-bbbb-aaaa000005dc"),
            (20, "ffffeeee-dddd-cccc-bbbb-aaaa00000014"),
            (4_294_967_295, "ffffeeee-dddd-cccc-bbbb-aaaaffffffff"),
            (0, "ffffeeee-dddd-cccc-bbbb-aaaa00000000"),
        ];

        for (uid, uuid) in uids {
            let guid = uid_to_guid(uid)?;
            let expected = Uuid::parse_str(uuid)?;
            assert_eq!(guid, expected);
        }

        Ok(())
    }

    /// Test `gid_to_guid` converts gid to guid.
    #[test]
    #[cfg(target_os = "macos")]
    fn test_gid_to_guid() -> TestResult {
        helper::init_logging();

        let gids = [
            (89u32, "abcdefab-cdef-abcd-efab-cdef00000059"),
            (1500, "aaaabbbb-cccc-dddd-eeee-ffff000005dc"),
            (20, "abcdefab-cdef-abcd-efab-cdef00000014"),
            (4_294_967_295, "aaaabbbb-cccc-dddd-eeee-ffffffffffff"),
            (0, "abcdefab-cdef-abcd-efab-cdef00000000"),
        ];

        for (gid, uuid) in gids {
            let guid = gid_to_guid(gid)?;
            let expected = Uuid::parse_str(uuid)?;
            assert_eq!(guid, expected);
        }

        Ok(())
    }

    /// Test `guid_to_id` converts guid to a uid or gid, if possible.
    #[test]
    #[cfg(target_os = "macos")]
    fn test_guid_to_id() -> TestResult {
        helper::init_logging();

        let uuids = [
            ("ffffeeee-dddd-cccc-bbbb-aaaa00000059", (Some(89u32), None)),
            ("ffffeeee-dddd-cccc-bbbb-aaaa000005dc", (Some(1500), None)),
            ("abcdefab-cdef-abcd-efab-cdef00000059", (None, Some(89u32))),
            ("aaaabbbb-cccc-dddd-eeee-ffff000005dc", (None, Some(1500))),
            ("abcdefab-cdef-abcd-efab-cdef00000014", (None, Some(20))),
            ("00000000-0000-0000-0000-000000000000", (None, None)),
            ("ffffffff-ffff-ffff-ffff-ffffffffffff", (None, None)),
            (
                "ffffeeee-dddd-cccc-bbbb-aaaaffffffff",
                (Some(4_294_967_295), None),
            ),
            (
                "aaaabbbb-cccc-dddd-eeee-ffffffffffff",
                (None, Some(4_294_967_295)),
            ),
            ("ffffeeee-dddd-cccc-bbbb-aaaa00000000", (Some(0), None)),
            ("abcdefab-cdef-abcd-efab-cdef00000000", (None, Some(0))),
            ("ffffeeeeddddccccbbbbaaaa000005dc", (Some(1500), None)),
            ("abcdefabcdefabcdefabcdef00000000", (None, Some(0))),
        ];

        for (uuid, expected) in uuids {
            let guid = Uuid::parse_str(uuid)?;
            let id = guid_to_id(guid)?;
            assert_eq!(id, expected);
        }

        Ok(())
    }

    /// Test the `_LABTEST` cfg flag.
    #[test]
    #[cfg(_LABTEST)]
    fn test_labtest() -> TestResult {
        helper::init_logging();

        let users = [
            ("\u{1F9EA}", 777_771),
            ("777772", 777_773),
            ("fbbc14b7-95f9-47a7-8ee8-1cccb9220943", 777_774),
        ];

        for (name, uid) in users {
            let result = getpwnam(name)?;
            assert_eq!(result, Some(uid));

            let result = getpwuid(uid)?;
            assert_eq!(result, Some(name.into()));
        }

        // Test some edge cases.
        assert_eq!(name_to_uid("777772")?, 777_772);
        assert_eq!(uid_to_name(777_773)?, "777773");
        assert_eq!(name_to_gid("777772")?, 777_772);

        // Test non-UTF-8 group name on systems where it was added.
        // Even when a group name is available, the result should be the GID
        // in decimal.
        assert_eq!(gid_to_name(777_775)?, "777775");

        Ok(())
    }
}
