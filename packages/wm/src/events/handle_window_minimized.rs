use tracing::info;
use wm_common::{try_warn, WindowState};
use wm_platform::NativeWindow;

#[cfg(target_os = "windows")]
use crate::models::SnapArrangeState;
use crate::{
  commands::{
    container::set_focused_descendant, window::update_window_state,
  },
  traits::WindowGetters,
  user_config::UserConfig,
  wm_state::WmState,
};

pub fn handle_window_minimized(
  native_window: &NativeWindow,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let found_window = state.window_from_native(native_window);

  // Update the window's state to be minimized.
  if let Some(window) = found_window {
    let is_minimized = try_warn!(window.native().is_minimized());

    window.update_native_properties(|properties| {
      properties.is_minimized = is_minimized;
    });

    if is_minimized && window.state() != WindowState::Minimized {
      info!("Window minimized: {window}");

      let window = update_window_state(
        window.clone(),
        WindowState::Minimized,
        state,
        config,
      )?;

      // Minimizing makes the host that owns a snap-arrange window drop
      // its snap state, so the window has to be primed again once it's
      // restored. The OS' arranged flag can still report the window as
      // arranged at that point, hence the explicit invalidation.
      #[cfg(target_os = "windows")]
      window.update_snap_arrange_state(SnapArrangeState::invalidate);

      // Clear the drag state, as a window can be minimized while
      // being dragged (e.g. via `toggle-minimized`).
      // TODO: Investigate other code paths where the drag state should be
      // cleared (e.g. most commands that call `update_window_state`).
      window.set_active_drag(None);

      // Focus should be reassigned after a window has been minimized.
      if let Some(focus_target) = state.focus_target_after_removal(&window)
      {
        set_focused_descendant(&focus_target, None);
        state.pending_sync.queue_focus_change().queue_cursor_jump();
        state.unmanaged_or_minimized_timestamp =
          Some(std::time::Instant::now());
      }
    }
  }

  Ok(())
}
