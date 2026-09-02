use std::time::{Duration, Instant};

use wm_platform::Rect;

/// How many consecutive failed priming attempts are made before giving up.
///
/// Priming injects a synthetic keypress and briefly moves the window, so
/// it must not be retried indefinitely when the OS refuses to arrange the
/// window (e.g. because the keypress is swallowed by an input hook) or
/// cancels the arrangement right away. Attempts are replenished by
/// [`SnapArrangeState::mark_settled`], [`SnapArrangeState::invalidate`]
/// and [`SnapArrangeState::retry`].
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

    /// When the OS was last observed to have stopped interacting with
    /// the window.
    ///
    /// Pushed forward by [`SnapArrangeState::defer_settle`] for as long
    /// as the OS keeps the window from being the foreground window.
    settling_since: Instant,
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

  /// Rect that was last applied to the window while the OS considered it
  /// arranged, and that therefore reached the content the window hosts.
  applied_rect: Option<Rect>,
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

  /// Gets the rect that last reached the content the window hosts.
  ///
  /// Returns `None` if no rect is known to have reached it.
  #[must_use]
  pub fn applied_rect(&self) -> Option<Rect> {
    self.applied_rect.clone()
  }

  /// Records that `rect` reached the content the window hosts.
  pub fn mark_applied(&mut self, rect: Rect) {
    self.applied_rect = Some(rect);
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

  /// Whether the OS stopped interacting with a primed window less than
  /// `settle_duration` ago.
  ///
  /// The OS keeps interacting with the window (e.g. by showing its snap
  /// assist flyout) for a short while after arranging it, which is
  /// accounted for by [`SnapArrangeState::defer_settle`].
  #[must_use]
  pub fn is_settling(&self, settle_duration: Duration) -> bool {
    match self.phase {
      SnapArrangePhase::Primed { settling_since, .. } => {
        settling_since.elapsed() <= settle_duration
      }
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
  /// Failed attempts are kept until the attempt has settled, since the OS
  /// can still cancel the arrangement right afterwards.
  pub fn mark_primed(&mut self) {
    let now = Instant::now();

    self.phase = SnapArrangePhase::Primed {
      at: now,
      settling_since: now,
    };
  }

  /// Restarts the settle period of a primed window.
  ///
  /// Called while the OS is still interacting with the window after
  /// arranging it, which it signals by keeping the window from being the
  /// foreground window. Moving the window during that time cancels the
  /// arrangement, so the settle period only starts once the OS has handed
  /// the window back.
  ///
  /// Has no effect once the window has been primed for longer than
  /// `max_settle_duration`, so that a window that never regains focus
  /// (e.g. because the user focused another window in the meantime) is
  /// not deferred indefinitely.
  pub fn defer_settle(&mut self, max_settle_duration: Duration) {
    if let SnapArrangePhase::Primed { at, settling_since } = &mut self.phase
    {
      if at.elapsed() <= max_settle_duration {
        *settling_since = Instant::now();
      }
    }
  }

  /// Records that a priming attempt has settled with the window still
  /// arranged.
  ///
  /// Replenishes the failed attempts, so that the budget only limits
  /// consecutive failures. The OS cancels the arrangement of a settled
  /// window every so often (e.g. when a sibling window is closed while
  /// the window is being resized), which the window has to be able to
  /// recover from for the rest of its lifetime.
  pub fn mark_settled(&mut self) {
    if self.is_primed() {
      self.failed_attempts = 0;
    }
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
  /// happens when the window is minimized. The rect that last reached the
  /// content the window hosts is forgotten as well, since the host resizes
  /// that content on its own while the window is minimized.
  pub fn invalidate(&mut self) {
    self.phase = SnapArrangePhase::NeedsPrime;
    self.failed_attempts = 0;
    self.applied_rect = None;
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

  use wm_platform::Rect;

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
  fn applied_rect_is_forgotten_on_invalidation() {
    let mut state = SnapArrangeState::default();
    assert_eq!(state.applied_rect(), None);

    let rect = Rect::from_xy(0, 0, 800, 600);
    state.mark_applied(rect.clone());
    assert_eq!(state.applied_rect(), Some(rect));

    state.invalidate();
    assert_eq!(state.applied_rect(), None);
  }

  #[test]
  fn settling_is_deferred_while_the_os_interacts() {
    let mut state = SnapArrangeState::default();
    state.mark_in_flight();
    state.mark_primed();

    std::thread::sleep(Duration::from_millis(2));
    assert!(!state.is_settling(Duration::from_millis(1)));

    state.defer_settle(Duration::from_secs(60));
    assert!(state.is_settling(Duration::from_millis(1)));
  }

  #[test]
  fn settling_is_not_deferred_past_the_max_duration() {
    let mut state = SnapArrangeState::default();
    state.mark_in_flight();
    state.mark_primed();

    std::thread::sleep(Duration::from_millis(2));
    state.defer_settle(Duration::ZERO);
    assert!(!state.is_settling(Duration::from_millis(1)));
  }

  #[test]
  fn settling_is_only_deferred_while_primed() {
    let mut state = SnapArrangeState::default();
    state.mark_in_flight();

    state.defer_settle(Duration::from_secs(60));
    assert!(state.is_in_flight());
    assert!(!state.is_settling(Duration::from_secs(60)));
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
  fn unsettled_success_keeps_failed_attempts() {
    let mut state = SnapArrangeState::default();

    for _ in 1..MAX_FAILED_ATTEMPTS {
      state.mark_in_flight();
      state.mark_failed();
    }

    state.mark_in_flight();
    state.mark_primed();

    // An arrangement that is cancelled before it settled counts as the
    // final failed attempt.
    state.mark_failed();
    assert_eq!(state.phase(), SnapArrangePhase::GaveUp);
  }

  #[test]
  fn settling_replenishes_attempts() {
    let mut state = SnapArrangeState::default();

    for _ in 1..MAX_FAILED_ATTEMPTS {
      state.mark_in_flight();
      state.mark_failed();
    }

    state.mark_in_flight();
    state.mark_primed();
    state.mark_settled();

    for _ in 1..MAX_FAILED_ATTEMPTS {
      state.mark_in_flight();
      state.mark_failed();
      assert!(state.needs_prime());
    }
  }

  #[test]
  fn settling_only_replenishes_attempts_while_primed() {
    let mut state = SnapArrangeState::default();

    for _ in 1..MAX_FAILED_ATTEMPTS {
      state.mark_in_flight();
      state.mark_failed();
    }

    state.mark_in_flight();
    state.mark_settled();
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
