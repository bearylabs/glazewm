use std::time::{Duration, Instant};

use wm_common::{DisplayState, SnapArrangeConfig};
use wm_platform::{
  Direction, Dispatcher, DispatcherExtWindows, NativeWindow,
  NativeWindowWindowsExt, Rect, WindowZOrder, SET_WINDOW_POS_FLAGS,
};

use crate::{
  models::{NativeWindowProperties, SnapArrangeState, WindowContainer},
  traits::WindowGetters,
  user_config::UserConfig,
};

/// Class name of the windows that `WSLg` uses for remote applications.
pub const RAIL_WINDOW_CLASS: &str = "RAIL_WINDOW";

/// Direction of the snap chord used for priming.
const PRIME_DIRECTION: Direction = Direction::Left;

/// How long to wait for the OS to arrange a window after the snap chord
/// has been injected.
const PRIME_TIMEOUT: Duration = Duration::from_millis(500);

/// How often the OS' arranged state is polled while priming.
const PRIME_POLL_INTERVAL: Duration = Duration::from_millis(25);

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

/// Ensures a snap-arrange window is arranged by the OS, so that the tiling
/// rect applied to it reaches the content it hosts.
///
/// A window is primed by focusing it and injecting a synthetic snap chord.
/// Since the OS processes the chord asynchronously, `rect` is re-applied
/// from a background task once the window becomes arranged, and focus is
/// restored to whichever window had it beforehand.
///
/// `rect`, `z_order`, and `swp_flags` must match those of the redraw this
/// is called from, so that the window ends up at its intended position.
///
/// Failures are logged rather than propagated, since a window that cannot
/// be primed still tiles correctly on the host side.
pub fn sync_snap_arrange(
  window: &WindowContainer,
  rect: &Rect,
  z_order: &WindowZOrder,
  swp_flags: SET_WINDOW_POS_FLAGS,
  dispatcher: &Dispatcher,
  config: &UserConfig,
) {
  let snap_arrange = &config.value.general.snap_arrange;

  if !snap_arrange.enabled
    || !is_snap_arrange_window(&window.native_properties())
  {
    return;
  }

  // Give a window that couldn't be primed another chance whenever it's
  // shown again, since the conditions that caused priming to fail might
  // no longer apply.
  if matches!(window.display_state(), DisplayState::Showing) {
    window.update_snap_arrange_state(SnapArrangeState::reset);
  }

  // Resizes keep propagating for as long as the window stays arranged, so
  // priming is only needed after the state has been lost (e.g. by the
  // window being minimized and restored).
  if window.native().is_arranged() {
    window.update_snap_arrange_state(SnapArrangeState::reset);
    return;
  }

  if !window.snap_arrange_state().should_prime() {
    return;
  }

  window.update_snap_arrange_state(SnapArrangeState::mark_attempt);

  if let Err(err) = prime_window(
    window,
    rect,
    z_order,
    swp_flags,
    dispatcher,
    snap_arrange,
  ) {
    tracing::warn!("Failed to prime window for snap resizing: {}", err);
  }
}

/// Focuses the window, injects the snap chord, and schedules the window's
/// rect to be applied once the OS has arranged it.
fn prime_window(
  window: &WindowContainer,
  rect: &Rect,
  z_order: &WindowZOrder,
  swp_flags: SET_WINDOW_POS_FLAGS,
  dispatcher: &Dispatcher,
  snap_arrange: &SnapArrangeConfig,
) -> anyhow::Result<()> {
  let native = window.native().clone();

  tracing::info!("Priming window for snap resizing: {window}");

  // The OS only snaps the foreground window. Focus is left alone if the
  // window already has it, which is the common case since windows are
  // typically primed right after being managed or focused.
  let prev_focused = dispatcher
    .focused_window()
    .ok()
    .filter(|focused| focused.id() != native.id());

  if prev_focused.is_some() {
    native.focus()?;
  }

  dispatcher
    .send_snap_keypress(snap_arrange.modifier_key, &PRIME_DIRECTION)?;

  let rect = rect.clone();
  let z_order = z_order.clone();

  tokio::task::spawn(async move {
    if wait_until_arranged(&native).await {
      if let Err(err) = native.set_window_pos(&z_order, &rect, swp_flags) {
        tracing::warn!(
          "Failed to set window position after priming: {}",
          err
        );
      }
    } else {
      tracing::warn!(
        "Window was not arranged by the OS within {}ms. Its contents \
         will not follow the tiling rect.",
        PRIME_TIMEOUT.as_millis()
      );
    }

    // Restore focus regardless of the outcome. This also dismisses the
    // OS' snap assist flyout, which is shown by the snap chord.
    if let Some(prev_focused) = prev_focused {
      if let Err(err) = prev_focused.focus() {
        tracing::warn!("Failed to restore focus after priming: {}", err);
      }
    }
  });

  Ok(())
}

/// Polls until the window is arranged by the OS or [`PRIME_TIMEOUT`]
/// elapses.
///
/// Returns whether the window became arranged.
async fn wait_until_arranged(native: &NativeWindow) -> bool {
  let deadline = Instant::now() + PRIME_TIMEOUT;

  loop {
    if native.is_arranged() {
      return true;
    }

    if Instant::now() >= deadline {
      return false;
    }

    tokio::time::sleep(PRIME_POLL_INTERVAL).await;
  }
}
