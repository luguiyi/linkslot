//! Idle timer for the optional "turn Wi-Fi off after N minutes" feature.
//!
//! The frontend owns the radio and the script jobs; this module only tracks
//! whether the radio has been idle long enough. Keeping it pure makes the
//! timeout/busy behaviour testable without spawning scripts.

use slot_ui::Millis;

/// How often the frontend re-reads the radio state with `radio-status`.
pub const RADIO_POLL_MS: Millis = 60_000;

/// How long to wait before retrying a failed `radio off`.
pub const OFF_RETRY_MS: Millis = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleAction {
    None,
    /// The configured idle time elapsed; the caller should ask the script to
    /// turn the radio off. Repeated ticks may return this again until the
    /// caller acknowledges by turning the radio off or resetting the timer.
    TurnOff,
}

#[derive(Clone, Debug, Default)]
pub struct WifiIdle {
    idle_since: Option<Millis>,
}

impl WifiIdle {
    /// Advance one tick. `busy` covers any user-visible Wi-Fi use (settings
    /// page, Web service, cloud sync); `radio_on` is `Some(true)` only after a
    /// successful `radio-status` probe, so an unknown/off radio never expires.
    pub fn tick(
        &mut self,
        now: Millis,
        busy: bool,
        radio_on: Option<bool>,
        minutes: u16,
    ) -> IdleAction {
        if busy || minutes == 0 || radio_on != Some(true) {
            self.idle_since = None;
            return IdleAction::None;
        }
        let Some(since) = self.idle_since else {
            self.idle_since = Some(now);
            return IdleAction::None;
        };
        let timeout = u64::from(minutes).saturating_mul(60_000);
        if now.saturating_sub(since) >= timeout {
            IdleAction::TurnOff
        } else {
            IdleAction::None
        }
    }

    /// Forget the current idle period; the next non-busy tick starts a fresh
    /// one. Used whenever the user did something Wi-Fi related.
    pub fn reset(&mut self) {
        self.idle_since = None;
    }

    /// Restart the idle period at a known time (after a failed `radio off`).
    pub fn restart_at(&mut self, now: Millis) {
        self.idle_since = Some(now);
    }

    #[cfg(test)]
    fn since(&self) -> Option<Millis> {
        self.idle_since
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_timing_only_when_radio_is_on_and_not_busy() {
        let mut idle = WifiIdle::default();
        assert_eq!(idle.tick(0, false, None, 10), IdleAction::None);
        assert_eq!(idle.tick(1_000, false, Some(false), 10), IdleAction::None);
        assert_eq!(idle.tick(2_000, true, Some(true), 10), IdleAction::None);
        assert_eq!(idle.since(), None);

        assert_eq!(idle.tick(3_000, false, Some(true), 10), IdleAction::None);
        assert_eq!(idle.since(), Some(3_000));
        assert_eq!(idle.tick(3_000 + 599_999, false, Some(true), 10), IdleAction::None);
        assert_eq!(
            idle.tick(3_000 + 600_000, false, Some(true), 10),
            IdleAction::TurnOff
        );
    }

    #[test]
    fn busy_activity_resets_the_timer() {
        let mut idle = WifiIdle::default();
        idle.tick(0, false, Some(true), 5);
        idle.tick(100, true, Some(true), 5);
        assert_eq!(idle.since(), None);
        idle.tick(200, false, Some(true), 5);
        assert_eq!(idle.since(), Some(200));
    }

    #[test]
    fn never_and_radio_off_disable_the_timer() {
        let mut idle = WifiIdle::default();
        assert_eq!(idle.tick(0, false, Some(true), 0), IdleAction::None);
        assert_eq!(idle.since(), None);
        assert_eq!(idle.tick(0, false, Some(false), 10), IdleAction::None);
        assert_eq!(idle.since(), None);
    }

    #[test]
    fn reset_then_tick_starts_a_new_period() {
        let mut idle = WifiIdle::default();
        idle.tick(0, false, Some(true), 5);
        idle.reset();
        assert_eq!(idle.since(), None);
        idle.restart_at(9_000);
        assert_eq!(idle.since(), Some(9_000));
        assert_eq!(idle.tick(9_000 + 300_000, false, Some(true), 5), IdleAction::TurnOff);
    }
}
