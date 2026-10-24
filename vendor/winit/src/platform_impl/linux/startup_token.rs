//! The launcher's activation token for the first window (PdfCraft patch).
//!
//! A launcher with `StartupNotify=true` passes an activation token in the environment
//! (`XDG_ACTIVATION_TOKEN` on Wayland, `DESKTOP_STARTUP_ID` on X11) and shows a busy cursor
//! until a window hands it back. winit hands back only a token given through
//! `WindowAttributesExtStartupNotify::with_activation_token`, and eframe never gives one, so
//! GNOME kept the busy cursor until its 15-second timeout and the window opened without focus.
//!
//! The first window the process creates without a token now takes the one in the environment.
//! Later windows never do: the token is spent once a window has handed it back, or once the
//! first window exists. A window that fails before handing it back releases it, so a retry
//! still clears the busy cursor. The variable is left in the environment, unlike
//! `reset_activation_token_env`, because changing the environment while other threads may read
//! it is unsound, and an application can start threads before its first window.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::window::ActivationToken;

/// Longer tokens are ignored. Launchers make tokens of a few dozen bytes; on X11 the token is
/// written to a window property and sent to the window manager, and an oversized one would
/// fail window creation.
const MAX_TOKEN_LEN: usize = 1024;

/// No window has claimed the launcher's token.
const FREE: u8 = 0;
/// A window being created holds it.
const RESERVED: u8 = 1;
/// A window has handed it back, or the first window exists.
const SPENT: u8 = 2;

static STATE: AtomicU8 = AtomicU8::new(FREE);

/// The launcher's token held by a window being created. Dropping it before it is committed or
/// handed back releases the token for the next window.
pub(crate) struct Reservation<'a> {
    state: &'a AtomicU8,
    held: bool,
}

impl Reservation<'_> {
    /// The window was created: later windows don't take the launcher's token.
    pub(crate) fn commit(self) {
        if self.held {
            self.state.store(SPENT, Ordering::Relaxed);
        }
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if self.held {
            // Fails, leaving the token spent, once it was committed or handed back.
            let _ = self.state.compare_exchange(RESERVED, FREE, Ordering::Relaxed, Ordering::Relaxed);
        }
    }
}

/// The token for a window being created: `given` if there is one, else the environment's
/// value of `var` for the process's first window, with the reservation the caller commits
/// once the window exists.
pub(crate) fn for_new_window(given: Option<ActivationToken>, var: &str) -> (Option<ActivationToken>, Reservation<'static>) {
    choose(given, &STATE, || std::env::var(var).ok())
}

/// A backend is handing a window's token back to the launcher or compositor: from here on the
/// launcher's token is spent even if creating the window fails.
pub(crate) fn handing_back() {
    hand_back(&STATE);
}

fn hand_back(state: &AtomicU8) {
    // Windows are created one at a time on the event loop's thread, so a reservation, if any,
    // is this window's.
    let _ = state.compare_exchange(RESERVED, SPENT, Ordering::Relaxed, Ordering::Relaxed);
}

fn choose(given: Option<ActivationToken>, state: &AtomicU8, read: impl FnOnce() -> Option<String>) -> (Option<ActivationToken>, Reservation<'_>) {
    // The first window holds the launcher's token, whether or not it was given one.
    let held = state.compare_exchange(FREE, RESERVED, Ordering::Relaxed, Ordering::Relaxed).is_ok();
    let reservation = Reservation { state, held };
    if given.is_some() || !held {
        return (given, reservation);
    }
    let token = read().filter(|token| !token.is_empty() && token.len() <= MAX_TOKEN_LEN).map(ActivationToken::from_raw);
    (token, reservation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(raw: &str) -> Option<ActivationToken> {
        Some(ActivationToken::from_raw(raw.to_owned()))
    }

    fn created(state: &AtomicU8, given: Option<ActivationToken>, env: &str) -> Option<ActivationToken> {
        let (token, reservation) = choose(given, state, || Some(env.to_owned()));
        reservation.commit();
        token
    }

    #[test]
    fn the_first_window_takes_the_launchers_token() {
        let state = AtomicU8::new(FREE);
        assert_eq!(created(&state, None, "gnome-shell/PdfCraft/1234"), token("gnome-shell/PdfCraft/1234"));
    }

    #[test]
    fn a_later_window_does_not_reuse_the_spent_token() {
        let state = AtomicU8::new(FREE);
        assert!(created(&state, None, "t").is_some());
        assert_eq!(created(&state, None, "t"), None);
    }

    #[test]
    fn a_given_token_wins_and_still_spends_the_launchers() {
        let state = AtomicU8::new(FREE);
        assert_eq!(created(&state, token("mine"), "env"), token("mine"));
        assert_eq!(created(&state, None, "env"), None);
        assert_eq!(created(&state, token("again"), "env"), token("again"));
    }

    #[test]
    fn a_window_that_fails_before_handing_the_token_back_releases_it() {
        let state = AtomicU8::new(FREE);
        let (first, failed) = choose(None, &state, || Some("t".into()));
        assert_eq!(first, token("t"));
        drop(failed);
        assert_eq!(created(&state, None, "t"), token("t"));
        assert_eq!(created(&state, None, "t"), None);
    }

    #[test]
    fn a_window_given_a_token_that_fails_early_releases_the_launchers() {
        let state = AtomicU8::new(FREE);
        drop(choose(token("mine"), &state, || Some("env".into())));
        assert_eq!(created(&state, None, "env"), token("env"));
    }

    #[test]
    fn a_window_that_fails_after_handing_the_token_back_spends_it() {
        let state = AtomicU8::new(FREE);
        let (first, failed) = choose(None, &state, || Some("t".into()));
        assert_eq!(first, token("t"));
        hand_back(&state);
        drop(failed);
        assert_eq!(created(&state, None, "t"), None);
    }

    #[test]
    fn a_window_created_while_the_token_is_held_does_not_take_or_release_it() {
        let state = AtomicU8::new(FREE);
        let (first, holder) = choose(None, &state, || Some("t".into()));
        assert_eq!(first, token("t"));
        let (other, reservation) = choose(None, &state, || Some("t".into()));
        assert_eq!(other, None);
        drop(reservation);
        assert_eq!(state.load(Ordering::Relaxed), RESERVED);
        drop(holder);
        assert_eq!(created(&state, None, "t"), token("t"));
    }

    #[test]
    fn a_missing_empty_or_oversized_token_is_ignored() {
        assert_eq!(created(&AtomicU8::new(FREE), None, ""), None);
        assert_eq!(choose(None, &AtomicU8::new(FREE), || None).0, None);
        let long = "x".repeat(MAX_TOKEN_LEN + 1);
        assert_eq!(created(&AtomicU8::new(FREE), None, &long), None);
        let limit = "x".repeat(MAX_TOKEN_LEN);
        assert!(created(&AtomicU8::new(FREE), None, &limit).is_some());
    }
}
