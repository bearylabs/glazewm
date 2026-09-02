use std::time::{Duration, Instant};

use wm_common::DisplayState;
use wm_platform::{
  DispatcherExtWindows, Key, NativeWindow, NativeWindowWindowsExt,
};

use crate::{
  models::{NativeWindowProperties, SnapArrangeState, WindowContainer},
  traits::{CommonGetters, WindowGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

/// Class name of the windows that `WSLg` uses for remote applications.
pub const RAIL_WINDOW_CLASS: &str = "RAIL_WINDOW";

/// Class name of the OS' snap assist flyout, which is shown after a
/// window has been snapped and takes keyboard focus.
const SNAP_ASSIST_WINDOW_CLASS: &str = "XamlExplorerHostIslandWindow";

/// Process name of the OS' snap assist flyout.
const SNAP_ASSIST_PROCESS_NAME: &str = "explorer";

/// Direction key of the snap chord used for priming.
const PRIME_DIRECTION_KEY: Key = Key::Left;

/// How long to wait after a window has been positioned before sending the
/// snap chord.
///
/// The window's position is applied asynchronously by the OS. A move that
/// is still pending when the OS snaps the window interleaves with the
/// snap and cancels the arrangement.
const PRIME_DELAY: Duration = Duration::from_millis(100);

/// How long focus has to be unchanged before the snap chord is sent.
///
/// The chord acts on whichever window is in the foreground when the OS
/// processes it. Focus can still be bouncing between windows right after
/// one of them is shown or restored (e.g. between the windows of a `WSLg`
/// session), in which case the chord would snap the wrong window.
const FOCUS_SETTLE_DURATION: Duration = Duration::from_millis(150);

/// How long to wait for the OS to arrange a window after the snap chord
/// has been sent.
const PRIME_TIMEOUT: Duration = Duration::from_millis(500);

/// How long the OS is expected to keep interacting with a window after
/// arranging it.
///
/// The OS adjusts the window's size a second time up to roughly 170ms
/// after the snap chord, and its snap assist flyout takes keyboard focus
/// roughly 150ms after it. Moving the window or changing the foreground
/// window during that time cancels the arrangement, so the window must be
/// left alone until it has settled.
const PRIME_SETTLE_DURATION: Duration = Duration::from_millis(300);

/// Whether a window only forwards resizes to the content it hosts while
/// the OS considers it arranged.
///
/// Only the `RAIL_WINDOW` windows used by `WSLg` are known to behave this
/// way. The check is kept separate from the priming logic so that other
/// remote display hosts can be added without touching it.
#[must_use]
pub fn is_snap_arrange_window(
  native_properties: &NativeWindowProperties,
) -> bool {
  native_properties.class_name == RAIL_WINDOW_CLASS
}

/// Whether the window is subject to priming under the current config.
fn is_prime_candidate(
  window: &WindowContainer,
  config: &UserConfig,
) -> bool {
  config.value.general.snap_arrange.enabled
    && is_snap_arrange_window(&window.native_properties())
}

/// Schedules priming of the window if it needs it, so that the tiling
/// rect applied to it reaches the content it hosts.
///
/// Called from a redraw, after the window's rect has been applied. A
/// window is primed by injecting a synthetic snap chord, which the OS
/// applies to the foreground window. Since the WM's focus sync runs
/// before redraws and focuses windows synchronously, only the WM's
/// focused container can be the foreground window, and it is the only
/// window that is ever primed. Windows that need priming but aren't
/// focused are primed once they become the focused container.
///
/// The chord itself is sent by [`send_scheduled_prime`] once the window's
/// position has been applied and focus has settled, since both interfere
/// with the OS' snap.
///
/// Priming is needed once per window, and again after the window has been
/// minimized. Everything else — moving, resizing, and hiding the window
/// while switching workspaces — leaves the host's snap-arrange state
/// intact. The OS occasionally cancels the arrangement on its own (e.g.
/// when the window's host echoes a stale size back to it), which is
/// detected here as well and results in the window being primed again.
pub fn sync_snap_arrange(
  window: &WindowContainer,
  state: &mut WmState,
  config: &UserConfig,
) {
  if !is_prime_candidate(window, config) {
    return;
  }

  // Give a window that couldn't be primed another chance whenever it's
  // shown again, since the conditions that caused priming to fail might
  // no longer apply. The window is still hidden at this point, so priming
  // itself waits for the redraw that follows its shown event.
  if window.display_state() == DisplayState::Showing {
    window.update_snap_arrange_state(SnapArrangeState::retry);
    return;
  }

  if window.display_state() != DisplayState::Shown {
    return;
  }

  let snap_arrange_state = window.snap_arrange_state();

  // The OS' arranged flag is reliable once the attempt has settled, so a
  // window that has lost it needs to be primed again.
  if snap_arrange_state.is_primed()
    && !snap_arrange_state.is_settling(PRIME_SETTLE_DURATION)
    && !window.native().is_arranged()
  {
    tracing::warn!("Arrangement was lost: {window}");
    record_failed_attempt(window);
  }

  if !window.snap_arrange_state().needs_prime() {
    return;
  }

  let is_focused = state
    .focused_container()
    .is_some_and(|focused| focused.id() == window.id());

  if !is_focused {
    return;
  }

  // Keep the earlier schedule if the same window is redrawn again, so
  // that repeated redraws don't postpone the chord indefinitely.
  let is_scheduled = state
    .scheduled_snap_prime
    .is_some_and(|(window_id, _)| window_id == window.id());

  if !is_scheduled {
    state.scheduled_snap_prime = Some((window.id(), Instant::now()));
  }
}

/// Sends the snap chord for the scheduled window once it is safe to do
/// so.
///
/// Called periodically while a prime is scheduled. The chord is held off
/// while the window's position is still being applied, while focus is
/// still changing, and while another window is being primed.
pub fn send_scheduled_prime(state: &mut WmState, config: &UserConfig) {
  let Some((window_id, scheduled_at)) = state.scheduled_snap_prime else {
    return;
  };

  let window = state
    .container_by_id(window_id)
    .and_then(|container| container.as_window_container().ok());

  let Some(window) = window else {
    state.scheduled_snap_prime = None;
    return;
  };

  let is_focused = state
    .focused_container()
    .is_some_and(|focused| focused.id() == window.id());

  if !is_prime_candidate(&window, config)
    || !window.snap_arrange_state().needs_prime()
    || window.display_state() != DisplayState::Shown
    || !is_focused
  {
    state.scheduled_snap_prime = None;
    return;
  }

  let is_focus_settling = state
    .last_focus_event_timestamp
    .is_some_and(|timestamp| timestamp.elapsed() < FOCUS_SETTLE_DURATION);

  if scheduled_at.elapsed() < PRIME_DELAY
    || is_focus_settling
    || active_prime(state, config).is_some()
  {
    return;
  }

  // The OS ignores the chord while the window is hidden or minimized. The
  // window is redrawn again once it's shown or restored.
  let is_ready = window.native().is_visible().unwrap_or(false)
    && !window.native().is_minimized().unwrap_or(true);

  if !is_ready {
    state.scheduled_snap_prime = None;
    return;
  }

  state.scheduled_snap_prime = None;
  tracing::info!("Priming window for snap resizing: {window}");

  let modifier_key = config.value.general.snap_arrange.modifier_key;

  if let Err(err) = state
    .dispatcher
    .send_keypress(&[modifier_key, PRIME_DIRECTION_KEY])
  {
    tracing::warn!("Failed to inject snap keypress: {}", err);
    record_failed_attempt(&window);
    return;
  }

  window.update_snap_arrange_state(SnapArrangeState::mark_in_flight);
  state.active_snap_prime = Some(window.id());
}

/// Gets the window that is currently being primed.
///
/// A window is being primed while its snap chord is in flight, and for
/// [`PRIME_SETTLE_DURATION`] after the OS has arranged it. The WM must
/// not move the window or change the foreground window during that time,
/// since doing so cancels the arrangement.
///
/// Attempts that are stale are cleared as a side effect: the attempt is
/// failed via [`fail_prime`] if [`PRIME_TIMEOUT`] has elapsed, finalized
/// via [`settle_prime`] if it has settled, and forgotten if the window has
/// been unmanaged or invalidated in the meantime.
///
/// Returns `None` if no window is being primed.
pub fn active_prime(
  state: &mut WmState,
  config: &UserConfig,
) -> Option<WindowContainer> {
  let window_id = state.active_snap_prime?;

  let window = state
    .container_by_id(window_id)
    .and_then(|container| container.as_window_container().ok());

  let Some(window) = window else {
    state.active_snap_prime = None;
    return None;
  };

  let snap_arrange_state = window.snap_arrange_state();

  if snap_arrange_state.is_expired(PRIME_TIMEOUT) {
    tracing::warn!(
      "Window was not arranged by the OS within {}ms: {window}",
      PRIME_TIMEOUT.as_millis()
    );

    fail_prime(&window, state);
    return None;
  }

  if snap_arrange_state.is_in_flight()
    || snap_arrange_state.is_settling(PRIME_SETTLE_DURATION)
  {
    return Some(window);
  }

  // The attempt has settled or was invalidated (e.g. by minimizing the
  // window) in the meantime.
  state.active_snap_prime = None;

  if snap_arrange_state.is_primed() {
    settle_prime(&window, state, config);
  }

  None
}

/// Finalizes a priming attempt after the OS has stopped interacting with
/// the window.
///
/// Queues a redraw so that the snap rect is replaced by the window's
/// tiling rect, which is safe to do now. Other windows that are waiting
/// to be primed are redrawn as well, so that the next focused candidate
/// gets its turn. Focus is re-asserted if the OS has moved it elsewhere
/// in the meantime.
///
/// If the arrangement was cancelled in the meantime, the redraw schedules
/// another attempt instead.
fn settle_prime(
  window: &WindowContainer,
  state: &mut WmState,
  config: &UserConfig,
) {
  if window.native().is_arranged() {
    let waiting_windows = state
      .windows()
      .into_iter()
      .filter(|other| {
        other.id() != window.id()
          && is_prime_candidate(other, config)
          && other.snap_arrange_state().needs_prime()
          && other
            .workspace()
            .is_some_and(|workspace| workspace.is_displayed())
      })
      .collect::<Vec<_>>();

    state
      .pending_sync
      .queue_containers_to_redraw(waiting_windows);
  } else {
    tracing::warn!("Arrangement was cancelled by the OS: {window}");
    record_failed_attempt(window);
  }

  state.pending_sync.queue_container_to_redraw(window.clone());
  queue_focus_change_if_lost(window, state);
}

/// Completes the in-flight priming attempt of the window, which has been
/// arranged by the OS.
///
/// The window is left at its snap rect until the attempt has settled, at
/// which point [`settle_prime`] applies its tiling rect.
pub fn complete_prime(window: &WindowContainer) {
  tracing::info!("Window primed for snap resizing: {window}");
  window.update_snap_arrange_state(SnapArrangeState::mark_primed);
}

/// Fails the in-flight priming attempt of the window.
///
/// Queues a redraw, which re-applies the window's rect in case the OS
/// did arrange the window without it being detected, and schedules
/// another attempt if any are left. Focus is re-asserted if the OS has
/// moved it elsewhere.
pub fn fail_prime(window: &WindowContainer, state: &mut WmState) {
  record_failed_attempt(window);

  if state.active_snap_prime == Some(window.id()) {
    state.active_snap_prime = None;
  }

  state.pending_sync.queue_container_to_redraw(window.clone());
  queue_focus_change_if_lost(window, state);
}

/// Dismisses the OS' snap assist flyout if it has been shown for a window
/// that is being primed.
///
/// The flyout is dismissed by injecting an `Escape` keypress, which
/// returns focus to the primed window without cancelling its arrangement.
/// Re-asserting focus instead would cancel it. The flyout activates itself
/// before it is shown, and `Escape` only takes effect (and is harmless to
/// the arrangement) once it is both shown and focused, so this is called
/// from both focus and shown events.
///
/// Returns `true` if the given window is the flyout and was dismissed.
pub fn dismiss_snap_assist(
  native_window: &NativeWindow,
  state: &mut WmState,
  config: &UserConfig,
) -> bool {
  let Some(active) = active_prime(state, config) else {
    return false;
  };

  if !is_snap_assist_window(native_window) {
    return false;
  }

  let is_shown_and_focused = native_window.is_visible().unwrap_or(false)
    && state
      .dispatcher
      .focused_window()
      .is_ok_and(|foreground| foreground == *native_window);

  if !is_shown_and_focused {
    return false;
  }

  tracing::info!("Dismissing snap assist flyout for: {active}");

  if let Err(err) = state.dispatcher.send_keypress(&[Key::Escape]) {
    tracing::warn!("Failed to dismiss snap assist flyout: {}", err);
  }

  true
}

/// Queues a redraw of the window if it is waiting to be primed.
///
/// Priming is only scheduled from a redraw, so this is called from events
/// that change whether the window can be primed (e.g. it gets focused or
/// is shown).
pub fn queue_redraw_if_needs_prime(
  window: &WindowContainer,
  state: &mut WmState,
  config: &UserConfig,
) {
  if is_prime_candidate(window, config)
    && window.snap_arrange_state().needs_prime()
  {
    state.pending_sync.queue_container_to_redraw(window.clone());
  }
}

/// Whether the window is the OS' snap assist flyout.
fn is_snap_assist_window(native_window: &NativeWindow) -> bool {
  native_window
    .class_name()
    .is_ok_and(|class_name| class_name == SNAP_ASSIST_WINDOW_CLASS)
    && native_window
      .process_name()
      .is_ok_and(|process_name| process_name == SNAP_ASSIST_PROCESS_NAME)
}

/// Queues a focus change if the window is the WM's focused container but
/// not the OS' foreground window.
fn queue_focus_change_if_lost(
  window: &WindowContainer,
  state: &mut WmState,
) {
  let is_focused = state
    .focused_container()
    .is_some_and(|focused| focused.id() == window.id());

  let is_foreground = state
    .dispatcher
    .focused_window()
    .is_ok_and(|foreground| foreground == *window.native());

  if is_focused && !is_foreground {
    state.pending_sync.queue_focus_change();
  }
}

/// Records a failed priming attempt on the window.
fn record_failed_attempt(window: &WindowContainer) {
  window.update_snap_arrange_state(SnapArrangeState::mark_failed);

  if !window.snap_arrange_state().needs_prime() {
    tracing::warn!(
      "Giving up on priming window. Its contents will not follow the \
       tiling rect: {window}"
    );
  }
}
