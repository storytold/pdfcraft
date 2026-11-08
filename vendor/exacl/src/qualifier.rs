//! Implements the `Qualifier` type for internal use

use crate::failx::*;
use crate::unix;
use std::fmt;
use std::io;
#[cfg(target_os = "macos")]
use uuid::Uuid;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
const OWNER_NAME: &str = "";
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
const OTHER_NAME: &str = "";
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
const MASK_NAME: &str = "";
#[cfg(target_os = "freebsd")]
const EVERYONE_NAME: &str = "";

/// A Qualifier specifies the principal that is allowed/denied access to a
/// resource.
#[derive(Debug, PartialEq, Eq)]
pub enum Qualifier {
    User(unix::uid_t),
    Group(unix::gid_t),

    #[cfg(target_os = "macos")]
    Guid(Uuid),

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    UserObj,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    GroupObj,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    Other,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    Mask,
    #[cfg(target_os = "freebsd")]
    Everyone,

    Unknown(String),
}

impl Qualifier {
    /// Create qualifier object from a user name.
    #[cfg(target_os = "macos")]
    pub fn user_named(name: &str) -> io::Result<Qualifier> {
        match unix::name_to_uid(name) {
            Ok(uid) => Ok(Qualifier::User(uid)),
            Err(err) => unix::name_to_guid(name).map_or(Err(err), Qualifier::from_guid),
        }
    }

    /// Create qualifier object from a user name.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn user_named(name: &str) -> io::Result<Qualifier> {
        match name {
            OWNER_NAME => Ok(Qualifier::UserObj),
            s => unix::name_to_uid(s).map(Qualifier::User),
        }
    }

    /// Create qualifier object from a group name.
    #[cfg(target_os = "macos")]
    pub fn group_named(name: &str) -> io::Result<Qualifier> {
        match unix::name_to_gid(name) {
            Ok(gid) => Ok(Qualifier::Group(gid)),
            Err(err) => unix::name_to_guid(name).map_or(Err(err), Qualifier::from_guid),
        }
    }

    /// Create qualifier object from a group name.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn group_named(name: &str) -> io::Result<Qualifier> {
        match name {
            OWNER_NAME => Ok(Qualifier::GroupObj),
            s => unix::name_to_gid(s).map(Qualifier::Group),
        }
    }

    /// Create qualifier from mask.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn mask_named(name: &str) -> io::Result<Qualifier> {
        match name {
            MASK_NAME => Ok(Qualifier::Mask),
            s => fail_custom(&format!("unknown mask name: {s:?}")),
        }
    }

    /// Create qualifier from other.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn other_named(name: &str) -> io::Result<Qualifier> {
        match name {
            OTHER_NAME => Ok(Qualifier::Other),
            s => fail_custom(&format!("unknown other name: {s:?}")),
        }
    }

    /// Create qualifier from everyone.
    #[cfg(target_os = "freebsd")]
    pub fn everyone_named(name: &str) -> io::Result<Qualifier> {
        match name {
            EVERYONE_NAME => Ok(Qualifier::Everyone),
            s => fail_custom(&format!("unknown everyone name: {s:?}")),
        }
    }

    /// Create qualifier from GUID and translate it. Used internally.
    #[cfg(target_os = "macos")]
    fn from_guid(guid: Uuid) -> io::Result<Qualifier> {
        Qualifier::Guid(guid).translate_guid()
    }

    /// Return the GUID for the user/group.
    #[cfg(target_os = "macos")]
    pub fn guid(&self) -> io::Result<Uuid> {
        match self {
            Qualifier::User(uid) => unix::uid_to_guid(*uid),
            Qualifier::Group(gid) => unix::gid_to_guid(*gid),
            Qualifier::Guid(guid) => Ok(*guid),
            Qualifier::Unknown(tag) => fail_custom(&format!("unknown tag: {tag:?}")),
        }
    }

    /// Return the name of the user/group.
    ///
    /// If `native` is true, return the numeric uid/gid or the underlying GUID
    /// on macOS.
    pub fn name(&self, native: bool) -> io::Result<String> {
        let result = match self {
            Qualifier::User(uid) if native => uid.to_string(),
            Qualifier::User(uid) => unix::uid_to_name(*uid)?,
            Qualifier::Group(gid) if native => gid.to_string(),
            Qualifier::Group(gid) => unix::gid_to_name(*gid)?,
            #[cfg(target_os = "macos")]
            Qualifier::Guid(guid) => guid.as_braced().to_string(),
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            Qualifier::UserObj | Qualifier::GroupObj => OWNER_NAME.to_string(),
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            Qualifier::Other => OTHER_NAME.to_string(),
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            Qualifier::Mask => MASK_NAME.to_string(),
            #[cfg(target_os = "freebsd")]
            Qualifier::Everyone => EVERYONE_NAME.to_string(),

            Qualifier::Unknown(s) => s.clone(),
        };

        Ok(result)
    }

    /// Convert GUID to a user or group ID.
    #[cfg(target_os = "macos")]
    pub fn translate_guid(&self) -> io::Result<Qualifier> {
        if let Qualifier::Guid(guid) = self {
            let qualifier = match unix::guid_to_id(*guid)? {
                (Some(uid), None) => Qualifier::User(uid),
                (None, Some(gid)) => Qualifier::Group(gid),
                (None, None) => Qualifier::Guid(*guid),
                _ => unreachable!("guid_to_id bug"),
            };
            Ok(qualifier)
        } else {
            fail_custom(&format!("not a guid: {self:?}"))
        }
    }
}

impl fmt::Display for Qualifier {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Qualifier::User(uid) => write!(f, "user:{uid}"),
            Qualifier::Group(gid) => write!(f, "group:{gid}"),
            #[cfg(target_os = "macos")]
            Qualifier::Guid(guid) => write!(f, "guid:{guid}"),
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            Qualifier::UserObj => write!(f, "user"),
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            Qualifier::GroupObj => write!(f, "group"),
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            Qualifier::Other => write!(f, "other"),
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            Qualifier::Mask => write!(f, "mask"),
            #[cfg(target_os = "freebsd")]
            Qualifier::Everyone => write!(f, "everyone"),
            Qualifier::Unknown(s) => write!(f, "unknown:{s}"),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unix::helper;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Test the `translate_guid` method.
    #[test]
    #[cfg(target_os = "macos")]
    fn test_translate_guid() -> TestResult {
        let uuids = [
            ("ffffeeee-dddd-cccc-bbbb-aaaa00000059", Qualifier::User(89)),
            ("abcdefab-cdef-abcd-efab-cdef00000059", Qualifier::Group(89)),
            (
                "00000000-0000-0000-0000-000000000000",
                Qualifier::Guid(Uuid::nil()),
            ),
        ];

        for (uuid, result) in uuids {
            let user = Qualifier::Guid(Uuid::parse_str(uuid)?);
            assert_eq!(user.translate_guid()?, result);
        }

        Ok(())
    }

    /// Test the `user_named` factory method.
    #[test]
    fn test_user_named() -> TestResult {
        // Test numeric input.
        let result = Qualifier::user_named("89")?;
        assert_eq!(result, Qualifier::User(89));

        // Test a valid user name.
        let (name, uid) = helper::valid_user();
        let result = Qualifier::user_named(&name)?;
        assert_eq!(result, Qualifier::User(uid));

        // Test a user GUID on macOS.
        #[cfg(target_os = "macos")]
        {
            let result = Qualifier::user_named("{ffffeeee-dddd-cccc-bbbb-aaaa00000059}")?;
            assert_eq!(result, Qualifier::User(89));
        }

        Ok(())
    }

    /// Test the `group_named` factory method.
    #[test]
    fn test_group_named() -> TestResult {
        // Test numeric input.
        let result = Qualifier::group_named("89")?;
        assert_eq!(result, Qualifier::Group(89));

        // Test a valid group name.
        let (name, gid) = helper::valid_group();
        let result = Qualifier::group_named(&name)?;
        assert_eq!(result, Qualifier::Group(gid));

        // Test a group GUID on macOS.
        #[cfg(target_os = "macos")]
        {
            let result = Qualifier::group_named("{abcdefab-cdef-abcd-efab-cdef00000059}")?;
            assert_eq!(result, Qualifier::Group(89));
        }

        Ok(())
    }

    /// Test the `name` method.
    #[test]
    fn test_name() -> TestResult {
        // Test `name` method on a User.
        let (name, uid) = helper::valid_user();
        let user = Qualifier::User(uid);
        assert_eq!(user.name(false)?, name);
        assert_eq!(user.name(true)?, uid.to_string());

        // Test `name` method on a Group.
        let (name, gid) = helper::valid_group();
        let group = Qualifier::Group(gid);
        assert_eq!(group.name(false)?, name);
        assert_eq!(group.name(true)?, gid.to_string());

        #[cfg(target_os = "macos")]
        {
            // Test `name` method on a Guid on macOS (native has no effect).
            let uuid = Uuid::parse_str("abcdefab-cdef-abcd-efab-cdef00000059")?;
            let guid = Qualifier::Guid(uuid);
            assert_eq!(guid.name(false)?, "{abcdefab-cdef-abcd-efab-cdef00000059}");
            assert_eq!(guid.name(true)?, "{abcdefab-cdef-abcd-efab-cdef00000059}");
        }

        Ok(())
    }
}
