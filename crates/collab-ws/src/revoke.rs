//! Session tickets the cabinet has withdrawn before their `exp`.
//!
//! A ticket's signature, expiry and file binding cannot say "closed early", and
//! the cabinet closes a session when a user is removed from a file, loses the
//! right to write, or an administrator ends it. The transport consults this set
//! before a `hello` joins a room, so a withdrawn ticket opens no new socket, and
//! [`crate::WsServer::revoke_session`] closes the sockets it already opened
//! (thread #18 on PR #6622).

use std::collections::HashSet;
use std::sync::Mutex;

/// The revoked session ids. Shared by the transport and the control plane, so a
/// revocation reaches the sockets the control plane never sees.
#[derive(Debug, Default)]
pub struct Revocations {
    revoked: Mutex<HashSet<String>>,
}

impl Revocations {
    pub fn new() -> Self {
        Self::default()
    }

    /// Withdraw `session_id`. `true` when it was not revoked already.
    ///
    /// A poisoned lock is recovered rather than propagated: the set is only ever
    /// touched for one insert or one lookup, so a panic elsewhere leaves it
    /// consistent, and a launch that cannot revoke a session would be worse than
    /// the panic it came from.
    pub fn revoke(&self, session_id: &str) -> bool {
        match self.revoked.lock() {
            Ok(mut set) => set.insert(session_id.to_owned()),
            Err(poisoned) => poisoned.into_inner().insert(session_id.to_owned()),
        }
    }

    /// Has the cabinet withdrawn this session?
    pub fn is_revoked(&self, session_id: &str) -> bool {
        match self.revoked.lock() {
            Ok(set) => set.contains(session_id),
            Err(poisoned) => poisoned.into_inner().contains(session_id),
        }
    }

    /// How many sessions are withdrawn. A gauge, not a counter: there is no
    /// un-revoke, and the set lives as long as the process.
    pub fn revoked(&self) -> usize {
        match self.revoked.lock() {
            Ok(set) => set.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Revocations;

    #[test]
    fn a_session_is_revoked_once_and_forever() {
        let revocations = Revocations::new();
        assert!(!revocations.is_revoked("session-1"));
        assert!(revocations.revoke("session-1"));
        assert!(!revocations.revoke("session-1"), "a second revocation changes nothing");
        assert!(revocations.is_revoked("session-1"));
        assert!(!revocations.is_revoked("session-2"), "only the named session is withdrawn");
        assert_eq!(revocations.revoked(), 1);
    }
}
