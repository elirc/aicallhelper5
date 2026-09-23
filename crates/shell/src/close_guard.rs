//! Close guard (spec §13 Close): when the page reports unsaved work, the first
//! close is cancelled once and the page is told (`window:close-requested`); a
//! second close within 10 s always closes; an unresponsive page never makes
//! the window uncloseable.
//!
//! "Responsive" = the page talked to the core (any command, e.g.
//! `set_close_guard`) within [`ACK_WINDOW`] after the close request. The glue
//! calls [`CloseGuard::ping`] on every command.

use std::time::{Duration, Instant};

use callcore_contract::config::CLOSE_GUARD_WINDOW;

/// If the page has not pinged within this long after a cancelled close, the
/// next close is allowed even after the 10 s window.
pub const ACK_WINDOW: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseDecision {
    /// Let the window close.
    Allow,
    /// Cancel this close and emit `window:close-requested`.
    CancelAndNotify,
}

#[derive(Debug, Clone, Default)]
pub struct CloseGuard {
    active: bool,
    requested_at: Option<Instant>,
    last_ping: Option<Instant>,
}

impl CloseGuard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// `set_close_guard(active)`. Also counts as a ping.
    pub fn set_active(&mut self, active: bool, now: Instant) {
        self.active = active;
        self.last_ping = Some(now);
        if !active {
            self.requested_at = None;
        }
    }

    /// The page showed a sign of life (any command).
    pub fn ping(&mut self, now: Instant) {
        self.last_ping = Some(now);
    }

    /// Decide a close request. `page_attached` = an event channel is attached
    /// (if nobody can receive `window:close-requested`, cancelling would only
    /// make the window uncloseable).
    pub fn on_close_request(&mut self, now: Instant, page_attached: bool) -> CloseDecision {
        if !self.active || !page_attached {
            return CloseDecision::Allow;
        }
        if let Some(t) = self.requested_at {
            let since = now.saturating_duration_since(t);
            if since < CLOSE_GUARD_WINDOW {
                return CloseDecision::Allow;
            }
            let acked = self
                .last_ping
                .is_some_and(|p| p >= t && p.saturating_duration_since(t) <= ACK_WINDOW);
            if !acked {
                // The page never reacted to the last request: unresponsive.
                return CloseDecision::Allow;
            }
        }
        self.requested_at = Some(now);
        CloseDecision::CancelAndNotify
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn inactive_guard_always_allows() {
        let t0 = Instant::now();
        let mut g = CloseGuard::new();
        assert_eq!(g.on_close_request(t0, true), CloseDecision::Allow);
    }

    #[test]
    fn first_close_cancelled_second_within_window_allowed() {
        let t0 = Instant::now();
        let mut g = CloseGuard::new();
        g.set_active(true, t0);
        assert_eq!(
            g.on_close_request(t0 + secs(1), true),
            CloseDecision::CancelAndNotify
        );
        assert_eq!(g.on_close_request(t0 + secs(9), true), CloseDecision::Allow);
    }

    #[test]
    fn cycle_restarts_after_window_when_page_responded() {
        let t0 = Instant::now();
        let mut g = CloseGuard::new();
        g.set_active(true, t0);
        assert_eq!(g.on_close_request(t0, true), CloseDecision::CancelAndNotify);
        g.ping(t0 + secs(1)); // page acknowledged
        assert_eq!(
            g.on_close_request(t0 + secs(20), true),
            CloseDecision::CancelAndNotify
        );
        assert_eq!(
            g.on_close_request(t0 + secs(21), true),
            CloseDecision::Allow
        );
    }

    #[test]
    fn unresponsive_page_allows_next_close_even_after_window() {
        let t0 = Instant::now();
        let mut g = CloseGuard::new();
        g.set_active(true, t0);
        assert_eq!(
            g.on_close_request(t0 + secs(1), true),
            CloseDecision::CancelAndNotify
        );
        // No ping at all.
        assert_eq!(
            g.on_close_request(t0 + secs(60), true),
            CloseDecision::Allow
        );
    }

    #[test]
    fn a_ping_after_the_ack_window_is_not_an_ack() {
        let t0 = Instant::now();
        let mut g = CloseGuard::new();
        g.set_active(true, t0);
        g.on_close_request(t0, true);
        g.ping(t0 + secs(5)); // too late to count as a reaction
        assert_eq!(
            g.on_close_request(t0 + secs(30), true),
            CloseDecision::Allow
        );
    }

    #[test]
    fn no_attached_page_allows() {
        let t0 = Instant::now();
        let mut g = CloseGuard::new();
        g.set_active(true, t0);
        assert_eq!(g.on_close_request(t0, false), CloseDecision::Allow);
    }

    #[test]
    fn clearing_the_guard_allows_and_resets() {
        let t0 = Instant::now();
        let mut g = CloseGuard::new();
        g.set_active(true, t0);
        g.on_close_request(t0, true);
        g.set_active(false, t0 + secs(1));
        assert_eq!(g.on_close_request(t0 + secs(2), true), CloseDecision::Allow);
        g.set_active(true, t0 + secs(3));
        assert_eq!(
            g.on_close_request(t0 + secs(4), true),
            CloseDecision::CancelAndNotify
        );
    }

    #[test]
    fn never_uncloseable_under_repeated_clicks() {
        let t0 = Instant::now();
        let mut g = CloseGuard::new();
        g.set_active(true, t0);
        let mut allowed = false;
        for i in 0..3 {
            if g.on_close_request(t0 + Duration::from_millis(i * 100), true) == CloseDecision::Allow
            {
                allowed = true;
            }
        }
        assert!(allowed, "the second click must close");
    }
}
