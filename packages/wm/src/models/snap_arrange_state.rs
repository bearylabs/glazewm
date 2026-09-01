use std::time::{Duration, Instant};

/// How long to wait before a window is primed again.
///
/// Must exceed the time the priming routine spends waiting for the OS to
/// arrange the window, so that an in-flight attempt isn't counted as a
/// failure.
const PRIME_COOLDOWN: Duration = Duration::from_millis(1500);

/// How many consecutive failed priming attempts are made before giving up.
///
/// Priming injects a synthetic keypress and briefly moves the window, so
/// it must not be retried indefinitely when the OS refuses to arrange the
/// window (e.g. because the keypress is swallowed by an input hook).
const MAX_FAILED_ATTEMPTS: u32 = 2;

/// Tracks snap-arrange priming attempts for a single window.
///
/// A window is "primed" by snapping it into a snap layout, which is
/// required for some windows to forward resizes to the content they host.
/// See [`SnapArrangeConfig`](wm_common::SnapArrangeConfig) for details.
///
/// An attempt is assumed to have failed until the window is observed as
/// arranged, since the OS processes the injected keypress asynchronously.
#[derive(Clone, Debug, Default)]
pub struct SnapArrangeState {
  /// When the most recent priming attempt was started.
  last_attempt: Option<Instant>,

  /// Number of consecutive priming attempts after which the window was
  /// never observed as arranged.
  failed_attempts: u32,
}

// LINT: Snap-arrange priming is only performed on Windows.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl SnapArrangeState {
  /// Whether the window should be primed.
  ///
  /// Returns `false` while a previous attempt might still be in flight,
  /// and after [`MAX_FAILED_ATTEMPTS`] consecutive failures. Failures are
  /// cleared by [`SnapArrangeState::reset`].
  #[must_use]
  pub fn should_prime(&self) -> bool {
    if self.failed_attempts >= MAX_FAILED_ATTEMPTS {
      return false;
    }

    self
      .last_attempt
      .is_none_or(|last_attempt| last_attempt.elapsed() >= PRIME_COOLDOWN)
  }

  /// Records that a priming attempt has been started.
  pub fn mark_attempt(&mut self) {
    self.last_attempt = Some(Instant::now());
    self.failed_attempts += 1;
  }

  /// Clears any recorded attempts.
  ///
  /// Called once the window is observed as arranged, and whenever it is
  /// shown again, so that priming is retried after the user has had a
  /// chance to change the conditions that caused it to fail.
  pub fn reset(&mut self) {
    self.last_attempt = None;
    self.failed_attempts = 0;
  }
}

#[cfg(test)]
mod tests {
  use super::{SnapArrangeState, MAX_FAILED_ATTEMPTS};

  #[test]
  fn primes_when_no_attempt_has_been_made() {
    assert!(SnapArrangeState::default().should_prime());
  }

  #[test]
  fn does_not_prime_during_cooldown() {
    let mut state = SnapArrangeState::default();
    state.mark_attempt();

    assert!(!state.should_prime());
  }

  #[test]
  fn does_not_prime_after_max_failed_attempts() {
    let mut state = SnapArrangeState::default();

    for _ in 0..MAX_FAILED_ATTEMPTS {
      state.mark_attempt();
    }

    // Ignore the cooldown to isolate the failure count.
    state.last_attempt = None;
    assert!(!state.should_prime());
  }

  #[test]
  fn primes_again_after_reset() {
    let mut state = SnapArrangeState::default();

    for _ in 0..MAX_FAILED_ATTEMPTS {
      state.mark_attempt();
    }

    state.reset();
    assert!(state.should_prime());
  }
}
