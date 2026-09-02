use tracing::info;
use wm_common::{DisplayState, HideMethod};
use wm_platform::NativeWindow;

#[cfg(target_os = "windows")]
use crate::commands::window::{
  dismiss_snap_assist, queue_redraw_if_unprimed,
};
use crate::{
  commands::window::manage_window, traits::WindowGetters,
  user_config::UserConfig, wm_state::WmState,
};

pub fn handle_window_shown(
  native_window: NativeWindow,
  state: &mut WmState,
  config: &mut UserConfig,
) -> anyhow::Result<()> {
  // The OS' snap assist flyout takes focus before it is shown, so it can
  // only be dismissed once it has been shown.
  #[cfg(target_os = "windows")]
  if dismiss_snap_assist(&native_window, state, config) {
    return Ok(());
  }

  let found_window = state.window_from_native(&native_window);

  if let Some(window) = found_window {
    info!("Window shown: {window}");

    // Update display state if window is already managed.
    if config.value.general.hide_method != HideMethod::PlaceInCorner
      && window.display_state() == DisplayState::Showing
    {
      window.set_display_state(DisplayState::Shown);

      // Priming for snap resizing is skipped while the window is
      // hidden, so redraw now that it's shown.
      #[cfg(target_os = "windows")]
      queue_redraw_if_unprimed(&window, state, config);
    } else {
      state.pending_sync.queue_container_to_redraw(window);
    }
  } else if !state.ignored_windows.contains(&native_window) {
    // If the window is not managed and not explicitly ignored, manage it.
    manage_window(native_window, None, state, config)?;
  }

  Ok(())
}
