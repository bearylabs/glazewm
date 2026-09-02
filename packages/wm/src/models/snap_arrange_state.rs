use std::time::{Duration, Instant};

/// How many consecutive failed priming attempts are made before giving up.
///
/// Priming injects a synthetic keypress and briefly moves the window, so
/// it must not be retried indefinitely when the OS refuses to arrange the
/// window (e.g. because the keypress is swallowed by an input hook) or
/// keeps cancelling the arrangement. Attempts are replenished by
/// [`SnapArrangeState::invalidate`] and [`SnapArrangeState::retry`].
const MAX_FAILED_ATTEMPTS: u32 = 3;

/// Phase of snap-arrange priming for a single window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SnapArrangePhase {
  /// The window has to be primed before resizes reach its contents.
  ///
  /// Initial phase of a newly managed window, and re-entered after the
  /// window is minimized.
  #[default]
  NeedsPrime,

  /// The snap chord has been sent and the OS has not yet arranged the
  /// window.
  InFlight {
    /// When the chord was sent.
    started: Instant,
  },

  /// The window has been arranged by the OS, so resizes reach its
  /// contents until it is minimized.
  Primed {
    /// When the window was observed as arranged.
    at: Instant,
  },

  /// Priming failed [`MAX_FAILED_ATTEMPTS`] times in a row.
  GaveUp,
}

/// Tracks snap-arrange priming for a single window.
///
/// A window is "primed" by snapping it into a snap layout, which is
/// required for some windows to forward resizes to the content they host.
/// See [`SnapArrangeConfig`](wm_common::SnapArrangeConfig) for details.
///
/// An attempt is assumed to have failed until the window is observed as
/// arranged, since the OS processes the injected keypress asynchronously.
#[derive(Clone, Debug, Default)]
pub struct SnapArrangeState {
  /// Current priming phase.
  phase: SnapArrangePhase,

  /// Number of priming attempts that failed or were cancelled since the
  /// attempts were last replenished.
  failed_attempts: u32,
}

// LINT: Snap-arrange priming is only performed on Windows.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl SnapArrangeState {
  /// Gets the current priming phase.
  #[cfg(test)]
  #[must_use]
  pub fn phase(&self) -> SnapArrangePhase {
    self.phase
  }

  /// Whether a priming attempt should be started.
  #[must_use]
  pub fn needs_prime(&self) -> bool {
    self.phase == SnapArrangePhase::NeedsPrime
  }

  /// Whether a priming attempt is waiting for the OS to arrange the
  /// window.
  #[must_use]
  pub fn is_in_flight(&self) -> bool {
    matches!(self.phase, SnapArrangePhase::InFlight { .. })
  }

  /// Whether an in-flight priming attempt has been waiting for longer
  /// than `timeout`.
  ///
  /// Returns `false` if no attempt is in flight.
  #[must_use]
  pub fn is_expired(&self, timeout: Duration) -> bool {
    match self.phase {
      SnapArrangePhase::InFlight { started } => {
        started.elapsed() > timeout
      }
      _ => false,
    }
  }

  /// Whether the window has been arranged by the OS.
  #[must_use]
  pub fn is_primed(&self) -> bool {
    matches!(self.phase, SnapArrangePhase::Primed { .. })
  }

  /// Whether the window was primed less than `settle_duration` ago.
  ///
  /// The OS keeps interacting with the window (e.g. by showing its snap
  /// assist flyout) for a short while after arranging it.
  #[must_use]
  pub fn is_settling(&self, settle_duration: Duration) -> bool {
    match self.phase {
      SnapArrangePhase::Primed { at } => at.elapsed() <= settle_duration,
      _ => false,
    }
  }

  /// Records that the snap chord has been sent.
  pub fn mark_in_flight(&mut self) {
    self.phase = SnapArrangePhase::InFlight {
      started: Instant::now(),
    };
  }

  /// Records that the window has been arranged by the OS.
  ///
  /// Failed attempts are kept, since the OS can cancel the arrangement
  /// again afterwards.
  pub fn mark_primed(&mut self) {
    self.phase = SnapArrangePhase::Primed { at: Instant::now() };
  }

  /// Records that the OS did not arrange the window in time, or cancelled
  /// the arrangement afterwards.
  ///
  /// Returns to [`SnapArrangePhase::NeedsPrime`] so that the next redraw
  /// retries, or to [`SnapArrangePhase::GaveUp`] once
  /// [`MAX_FAILED_ATTEMPTS`] is reached.
  pub fn mark_failed(&mut self) {
    self.failed_attempts += 1;

    self.phase = if self.failed_attempts >= MAX_FAILED_ATTEMPTS {
      SnapArrangePhase::GaveUp
    } else {
      SnapArrangePhase::NeedsPrime
    };
  }

  /// Marks the window as needing to be primed again.
  ///
  /// Called when its host is known to have dropped the snap state, which
  /// happens when the window is minimized.
  pub fn invalidate(&mut self) {
    self.phase = SnapArrangePhase::NeedsPrime;
    self.failed_attempts = 0;
  }

  /// Gives a window that could not be primed another chance.
  ///
  /// Called when the window is shown again, so that priming is retried
  /// after the user has had a chance to change the conditions that caused
  /// it to fail. Has no effect unless the window is in
  /// [`SnapArrangePhase::GaveUp`].
  pub fn retry(&mut self) {
    if self.phase == SnapArrangePhase::GaveUp {
      self.invalidate();
    }
  }
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::{SnapArrangePhase, SnapArrangeState, MAX_FAILED_ATTEMPTS};

  #[test]
  fn needs_prime_initially() {
    let state = SnapArrangeState::default();

    assert!(state.needs_prime());
    assert!(!state.is_in_flight());
    assert!(!state.is_settling(Duration::from_secs(60)));
  }

  #[test]
  fn in_flight_until_primed() {
    let mut state = SnapArrangeState::default();
    state.mark_in_flight();

    assert!(state.is_in_flight());
    assert!(!state.needs_prime());
    assert!(!state.is_expired(Duration::from_secs(60)));

    state.mark_primed();
    assert!(matches!(state.phase(), SnapArrangePhase::Primed { .. }));
    assert!(!state.is_in_flight());
  }

  #[test]
  fn expires_after_timeout() {
    let mut state = SnapArrangeState::default();
    assert!(!state.is_expired(Duration::ZERO));

    state.mark_in_flight();
    std::thread::sleep(Duration::from_millis(1));
    assert!(state.is_expired(Duration::ZERO));
  }

  #[test]
  fn settles_after_being_primed() {
    let mut state = SnapArrangeState::default();
    state.mark_in_flight();
    assert!(!state.is_settling(Duration::from_secs(60)));

    state.mark_primed();
    assert!(state.is_settling(Duration::from_secs(60)));

    std::thread::sleep(Duration::from_millis(1));
    assert!(!state.is_settling(Duration::ZERO));
  }

  #[test]
  fn retries_until_max_failed_attempts() {
    let mut state = SnapArrangeState::default();

    for _ in 1..MAX_FAILED_ATTEMPTS {
      state.mark_in_flight();
      state.mark_failed();
      assert!(state.needs_prime());
    }

    state.mark_in_flight();
    state.mark_failed();
    assert_eq!(state.phase(), SnapArrangePhase::GaveUp);
    assert!(!state.needs_prime());
  }

  #[test]
  fn success_keeps_failed_attempts() {
    let mut state = SnapArrangeState::default();

    for _ in 1..MAX_FAILED_ATTEMPTS {
      state.mark_in_flight();
      state.mark_failed();
    }

    state.mark_in_flight();
    state.mark_primed();

    // A cancelled arrangement counts as the final failed attempt.
    state.mark_failed();
    assert_eq!(state.phase(), SnapArrangePhase::GaveUp);
  }

  #[test]
  fn invalidation_replenishes_attempts() {
    let mut state = SnapArrangeState::default();

    for _ in 0..MAX_FAILED_ATTEMPTS {
      state.mark_in_flight();
      state.mark_failed();
    }

    state.invalidate();
    state.mark_in_flight();
    state.mark_failed();
    assert!(state.needs_prime());
  }

  #[test]
  fn retry_only_applies_after_giving_up() {
    let mut state = SnapArrangeState::default();
    state.mark_in_flight();
    state.mark_primed();

    state.retry();
    assert!(matches!(state.phase(), SnapArrangePhase::Primed { .. }));

    for _ in 0..MAX_FAILED_ATTEMPTS {
      state.mark_in_flight();
      state.mark_failed();
    }

    assert_eq!(state.phase(), SnapArrangePhase::GaveUp);

    state.retry();
    assert!(state.needs_prime());
  }

  #[test]
  fn invalidation_forces_a_prime() {
    let mut state = SnapArrangeState::default();
    state.mark_in_flight();
    state.mark_primed();

    state.invalidate();
    assert!(state.needs_prime());
  }
}
