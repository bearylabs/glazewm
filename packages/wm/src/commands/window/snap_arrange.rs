use std::time::{Duration, Instant};

use wm_common::DisplayState;
use wm_platform::{
  DispatcherExtWindows, Key, NativeWindow, NativeWindowWindowsExt, Rect,
};

use crate::{
  models::{NativeWindowProperties, SnapArrangeState, WindowContainer},
  traits::{CommonGetters, PositionGetters, WindowGetters},
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

/// How much smaller a window is made while it waits to be primed.
///
/// The OS ignores the snap chord for a window that already occupies the
/// zone it would snap it to, and the window's host only forwards an
/// actual resize to the content it hosts. Both are avoided by keeping the
/// window off its snap zone until it has been primed, after which it is
/// resized to its real rect.
const PRIME_NUDGE_PX: i32 = 20;

/// How long a scheduled priming attempt waits for the conditions needed
/// to send the snap chord.
///
/// The chord can be held off by another attempt that is in progress, or
/// by the window not becoming the OS' foreground window.
const PRIME_SCHEDULE_TIMEOUT: Duration = Duration::from_millis(2000);

/// How long the OS is expected to keep interacting with a window after
/// it has handed the window back.
///
/// The OS adjusts the window's size a second time up to roughly 170ms
/// after the snap chord, and its snap assist flyout takes keyboard focus
/// roughly 150ms after it. Moving the window or changing the foreground
/// window during that time cancels the arrangement, so the window must be
/// left alone until it has settled.
///
/// The OS hands the window back by making it the foreground window again,
/// which is when this duration starts to elapse. How long that takes
/// varies: the flyout is only shown when there is another window to
/// suggest for the other half of the screen, and it takes several hundred
/// milliseconds to close after being dismissed. Measured on a `WSLg`
/// window, moving it 200ms after it became the foreground window again
/// kept the arrangement, whereas moving it immediately cancelled it.
const PRIME_SETTLE_DURATION: Duration = Duration::from_millis(300);

/// How long a priming attempt is allowed to wait for the OS to hand the
/// window back before it is settled regardless.
///
/// The OS never hands the window back if the user focuses another window
/// while it is being primed, in which case the attempt would otherwise
/// never settle.
const MAX_PRIME_SETTLE_DURATION: Duration = Duration::from_millis(2000);

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
/// Called from a redraw, before the window's rect is applied. A window is
/// primed by injecting a synthetic snap chord, which the OS applies to
/// the foreground window, so the window is focused for the duration of
/// the attempt. The WM's focus state is left untouched and re-asserted
/// afterwards.
///
/// The chord itself is sent by [`send_scheduled_prime`] once the window's
/// position has been applied, it is the foreground window, and focus has
/// settled, since all of those interfere with the OS' snap. Only one
/// window is primed at a time, so the remaining ones are scheduled by the
/// redraws that `settle_prime` queues for them.
///
/// Priming is needed once per window, and again after the window has been
/// minimized. Moving, resizing, and hiding the window while switching
/// workspaces leave the arrangement intact. The OS does cancel it when
/// the window is sized to cover the whole workspace, which is detected
/// here and results in the window being primed again the next time its
/// rect changes.
///
/// `rect` is the rect that the redraw is about to apply to the window.
///
/// Returns the rect that the redraw should apply instead, which is
/// [`PRIME_NUDGE_PX`] smaller while the window waits to be primed, or
/// `None` if `rect` should be applied unchanged.
#[must_use]
pub fn sync_snap_arrange(
  window: &WindowContainer,
  rect: &Rect,
  state: &mut WmState,
  config: &UserConfig,
) -> Option<Rect> {
  if !is_prime_candidate(window, config) {
    return None;
  }

  // Give a window that couldn't be primed another chance whenever it's
  // shown again, since the conditions that caused priming to fail might
  // no longer apply. The window is still hidden at this point, so priming
  // itself waits for the redraw that follows its shown event.
  if window.display_state() == DisplayState::Showing {
    window.update_snap_arrange_state(SnapArrangeState::retry);
    return None;
  }

  if window.display_state() != DisplayState::Shown {
    return None;
  }

  let snap_arrange_state = window.snap_arrange_state();

  // The OS' arranged flag is unreliable until the attempt has settled,
  // since the OS keeps acting on the snap until then.
  if snap_arrange_state.is_in_flight()
    || snap_arrange_state.is_settling(PRIME_SETTLE_DURATION)
  {
    return None;
  }

  // The rect reaches the content the window hosts, so no priming is
  // needed and the rect is worth remembering as the one that content has.
  //
  // The arranged flag is only taken as proof of that for a window that
  // this priming put in that state. The OS also reports a window as
  // arranged right after it has been restored from being minimized, at
  // which point its host has dropped the state that makes it forward
  // resizes.
  if snap_arrange_state.is_primed() && window.native().is_arranged() {
    window.update_snap_arrange_state(|snap_arrange_state| {
      snap_arrange_state.mark_applied(rect.clone());
    });

    return None;
  }

  // The window is no longer arranged, but was given this rect while it
  // was. This is the resting state of a window that covers its whole
  // workspace, since the OS cancels the arrangement of a window that is
  // sized that way. Priming it again would only be undone by the redraw
  // that follows, so it is deferred until the window's rect changes.
  if snap_arrange_state.applied_rect().as_ref() == Some(rect) {
    return None;
  }

  // The OS cancels the arrangement of a primed window every so often
  // (e.g. when a sibling window is closed while the window is being
  // resized), in which case it has to be primed again.
  if snap_arrange_state.is_primed() {
    tracing::warn!("Arrangement was lost: {window}");
    record_failed_attempt(window);
  }

  if !window.snap_arrange_state().needs_prime() {
    return None;
  }

  // Keep the earlier schedule if the same window is redrawn again, so
  // that repeated redraws don't postpone the chord indefinitely.
  let is_scheduled = state
    .scheduled_snap_prime
    .is_some_and(|(window_id, _)| window_id == window.id());

  if !is_scheduled {
    state.scheduled_snap_prime = Some((window.id(), Instant::now()));
  }

  Some(Rect::from_xy(
    rect.x(),
    rect.y(),
    (rect.width() - PRIME_NUDGE_PX).max(1),
    (rect.height() - PRIME_NUDGE_PX).max(1),
  ))
}

/// Sends the snap chord for the scheduled window once it is safe to do
/// so.
///
/// Called periodically while a prime is scheduled. The chord is held off
/// while the window's position is still being applied, while another
/// window is being primed, while the window is not the OS' foreground
/// window, and while focus is still changing.
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

  // The OS ignores the chord while the window is hidden or minimized. The
  // window is redrawn again once it's shown or restored.
  let is_ready = window.display_state() == DisplayState::Shown
    && window.native().is_visible().unwrap_or(false)
    && !window.native().is_minimized().unwrap_or(true);

  if !is_prime_candidate(&window, config)
    || !window.snap_arrange_state().needs_prime()
    || !is_ready
  {
    state.scheduled_snap_prime = None;
    return;
  }

  if scheduled_at.elapsed() > PRIME_SCHEDULE_TIMEOUT {
    tracing::warn!(
      "Window could not be prepared for priming within {}ms: {window}",
      PRIME_SCHEDULE_TIMEOUT.as_millis()
    );

    state.scheduled_snap_prime = None;
    record_failed_attempt(&window);
    restore_focus(state);
    return;
  }

  if scheduled_at.elapsed() < PRIME_DELAY
    || active_prime(state, config).is_some()
  {
    return;
  }

  // The chord acts on the OS' foreground window, so the window is focused
  // for the duration of the attempt. The WM's focus state is left
  // untouched, and its focused container is re-focused once the attempt
  // is done.
  if !is_foreground(&window, state) {
    if let Err(err) = window.native().focus() {
      tracing::warn!("Failed to focus window for priming: {}", err);

      state.scheduled_snap_prime = None;
      record_failed_attempt(&window);
      restore_focus(state);
    }

    return;
  }

  let is_focus_settling = state
    .last_focus_event_timestamp
    .is_some_and(|timestamp| timestamp.elapsed() < FOCUS_SETTLE_DURATION);

  if is_focus_settling {
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
/// [`PRIME_SETTLE_DURATION`] after the OS has arranged it and handed it
/// back as the foreground window. The WM must not move the window or
/// change the foreground window during that time, since doing so cancels
/// the arrangement.
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

  // The OS keeps the window from being the foreground window while it is
  // still acting on the snap (e.g. via its snap assist flyout). Moving
  // the window before it has been handed back cancels the arrangement, so
  // the settle period only starts once it is the foreground window again.
  if snap_arrange_state.is_primed() && !is_foreground(&window, state) {
    window.update_snap_arrange_state(|snap_arrange_state| {
      snap_arrange_state.defer_settle(MAX_PRIME_SETTLE_DURATION);
    });
  }

  let snap_arrange_state = window.snap_arrange_state();

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
/// to be primed are redrawn as well, so that the next candidate gets its
/// turn. The WM's focused container is re-focused, since priming moved
/// focus to the window it primed.
///
/// If the arrangement was cancelled in the meantime, the redraw schedules
/// another attempt instead.
fn settle_prime(
  window: &WindowContainer,
  state: &mut WmState,
  config: &UserConfig,
) {
  if window.native().is_arranged() {
    window.update_snap_arrange_state(SnapArrangeState::mark_settled);

    let waiting_windows = state
      .windows()
      .into_iter()
      .filter(|other| {
        other.id() != window.id()
          && is_prime_candidate(other, config)
          && is_unprimed(other)
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
  restore_focus(state);
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
/// another attempt if any are left. The WM's focused container is
/// re-focused, since priming moved focus to the window it primed.
pub fn fail_prime(window: &WindowContainer, state: &mut WmState) {
  record_failed_attempt(window);

  if state.active_snap_prime == Some(window.id()) {
    state.active_snap_prime = None;
  }

  state.pending_sync.queue_container_to_redraw(window.clone());
  restore_focus(state);
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

/// Queues a redraw of the window if it is waiting to be primed, or if the
/// OS has cancelled the arrangement it was primed with.
///
/// Priming is only scheduled from a redraw, so this is called from events
/// that change whether the window can be primed (e.g. it gets focused or
/// is shown). A cancelled arrangement is otherwise only noticed the next
/// time the window happens to be redrawn, which can be long after the
/// window has become unable to resize the content it hosts.
pub fn queue_redraw_if_unprimed(
  window: &WindowContainer,
  state: &mut WmState,
  config: &UserConfig,
) {
  if is_prime_candidate(window, config) && is_unprimed(window) {
    state.pending_sync.queue_container_to_redraw(window.clone());
  }
}

/// Whether the focus event for the window is a side effect of
/// snap-arrange priming.
///
/// Priming acts on the OS' foreground window, so it focuses the window it
/// primes, and the OS moves focus to windows of its own (a staging window
/// and the snap assist flyout) while acting on the snap. None of that
/// should change the WM's focus state, which is handed back the
/// foreground once the attempt is done.
pub fn is_priming_focus(
  native_window: &NativeWindow,
  state: &mut WmState,
  config: &UserConfig,
) -> bool {
  if active_prime(state, config).is_some() {
    return true;
  }

  state
    .scheduled_snap_prime
    .and_then(|(window_id, _)| state.container_by_id(window_id))
    .and_then(|container| container.as_window_container().ok())
    .is_some_and(|window| *window.native() == *native_window)
}

/// Whether the rect that the window is meant to have has not reached the
/// content it hosts.
///
/// Used by the events that can act on a window that is not being redrawn,
/// where the rect a redraw would apply is derived from the window's
/// layout instead of being given.
fn is_unprimed(window: &WindowContainer) -> bool {
  let snap_arrange_state = window.snap_arrange_state();

  if snap_arrange_state.is_in_flight()
    || snap_arrange_state.is_settling(PRIME_SETTLE_DURATION)
    || (snap_arrange_state.is_primed() && window.native().is_arranged())
  {
    return false;
  }

  match redraw_rect(window) {
    Some(rect) => snap_arrange_state.applied_rect() != Some(rect),
    None => false,
  }
}

/// Gets the rect that a redraw applies to the window.
///
/// Returns `None` if the window's layout can't be resolved.
fn redraw_rect(window: &WindowContainer) -> Option<Rect> {
  let rect = window.to_rect().ok()?;
  let border_delta = window.total_border_delta().ok()?;

  Some(rect.apply_delta(&border_delta, None))
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

/// Queues a focus change if the WM's focused container is a window that
/// is not the OS' foreground window.
///
/// Priming acts on the foreground window, so the window being primed is
/// focused for the duration of the attempt, as is the OS' snap assist
/// flyout. This hands focus back once the attempt is done.
fn restore_focus(state: &mut WmState) {
  let focused_window = state
    .focused_container()
    .and_then(|focused| focused.as_window_container().ok());

  if let Some(focused_window) = focused_window {
    if !is_foreground(&focused_window, state) {
      state.pending_sync.queue_focus_change();
    }
  }
}

/// Whether the window is the OS' foreground window.
fn is_foreground(window: &WindowContainer, state: &WmState) -> bool {
  state
    .dispatcher
    .focused_window()
    .is_ok_and(|foreground| foreground == *window.native())
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
