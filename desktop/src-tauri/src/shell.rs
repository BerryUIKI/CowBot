//! Background residency: the tray icon, closing to it, and quitting from it.
//!
//! Agent turns and crew runs execute in this process (see `commands::post_message`
//! and `commands::run_task`), so closing the window used to end them silently.
//! CowBot now stays resident: closing the window hides it, the tray icon is the
//! way back, and quitting is an explicit action that is refused while work is
//! running (ADR-0064).
//!
//! The tray menu's labels are localised by the frontend because the shell has no
//! access to the interface locale; English is the fallback until the window has
//! reported its language.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

use crate::{config, startup, AppState};

/// The tray icon's id, so the menu is reachable without a second handle.
pub const TRAY_ID: &str = "main";

/// Menu ids, matched in the tray's menu handler.
const OPEN_ID: &str = "tray-open";
const QUIT_ID: &str = "tray-quit";

/// Emitted when Quit was asked for while a task or channel turn is running.
///
/// The window is revealed at the same time, so the notice the frontend draws
/// lands somewhere the user can see.
pub const QUIT_BLOCKED_EVENT: &str = "cowbot-tray-quit-blocked";

/// The tray's two menu items, kept so their labels can follow the locale.
///
/// Only the text changes afterwards: on Linux a tray menu cannot be replaced
/// once it is set, so the items are built once and updated in place.
pub struct TrayHandles {
    pub open: MenuItem<tauri::Wry>,
    pub quit: MenuItem<tauri::Wry>,
}

/// What closing the main window should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// Keep the process alive and take the window away.
    Hide,
    /// End the process, as CowBot did before it had a tray.
    Exit,
}

/// Decide what a window close means.
///
/// `ready` is whether the workspace finished opening. A window that never got
/// past the startup screen has nothing to stay resident for, and hiding it would
/// bury a failure the user still has to answer.
pub fn close_action(ready: bool, close_to_tray: bool) -> CloseAction {
    if ready && close_to_tray {
        CloseAction::Hide
    } else {
        CloseAction::Exit
    }
}

/// Build the tray icon and its menu.
///
/// Called from the app's setup hook, after the window exists. A tray that cannot
/// be created is logged rather than fatal: the app still runs, and closing the
/// window then quits as it always did.
pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let open_item = MenuItem::with_id(app, OPEN_ID, "Open CowBot", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, QUIT_ID, "Quit CowBot", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open_item, &separator, &quit_item])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("CowBot")
        .menu(&menu)
        // Left click brings the window back; the menu is the right-click path.
        // Linux ignores this and shows the menu for either button.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            OPEN_ID => reveal(app),
            QUIT_ID => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                reveal(tray.app_handle());
            }
        });

    // The window icon is embedded by `generate_context!`, so the tray uses the
    // same artwork as the title bar without another bundled asset.
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    app.manage(TrayHandles {
        open: open_item,
        quit: quit_item,
    });
    Ok(())
}

/// Bring the main window back after it was hidden or minimised.
pub fn reveal(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Quit from the tray, unless something is still running.
///
/// The running sets live in the Rust state, so this answer is exact even while
/// the window is hidden and its timers are throttled. A refused quit reveals the
/// window and tells the frontend, which explains why.
fn quit(app: &AppHandle) {
    if work_in_flight(app) {
        reveal(app);
        let _ = app.emit(QUIT_BLOCKED_EVENT, ());
        return;
    }
    app.exit(0);
}

/// Whether a crew run or a channel turn is still executing.
fn work_in_flight(app: &AppHandle) -> bool {
    let Some(state) = app.try_state::<AppState>() else {
        return false;
    };
    !state.running.lock().is_empty() || !state.replies.lock().is_empty()
}

/// What the window's close button should do right now.
pub fn close_action_for(app: &AppHandle) -> CloseAction {
    close_action(initialized(app), close_to_tray(app))
}

/// Whether the shell finished opening its workspace.
fn initialized(app: &AppHandle) -> bool {
    app.try_state::<AppState>().is_some()
}

/// The stored preference.
///
/// Read from `config.json` directly: the startup screen's window is closed
/// before `AppState` exists, and it still has to obey the switch.
fn close_to_tray(app: &AppHandle) -> bool {
    let Some(path) = config_path(app) else {
        return true;
    };
    config::load(&path)
        .unwrap_or_else(|error| {
            eprintln!(
                "warning: failed to read {}: {error}; keeping closing-to-tray on",
                path.display()
            );
            config::AppConfig::default()
        })
        .close_to_tray
}

fn config_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    if let Some(state) = app.try_state::<AppState>() {
        return Some(state.config_path.clone());
    }
    app.try_state::<startup::StartupState>()
        .map(|state| state.config_path.clone())
        .filter(|path| !path.as_os_str().is_empty())
}

/// Persist whether closing the window hides it.
pub fn set_close_to_tray(state: &AppState, enabled: bool) -> Result<(), String> {
    let mut config = config::load(&state.config_path)
        .map_err(|error| format!("failed to read {}: {error}", state.config_path.display()))?;
    config.close_to_tray = enabled;
    config::save(&state.config_path, &config)
        .map_err(|error| format!("failed to write {}: {error}", state.config_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ready_window_hides_when_the_switch_is_on() {
        assert_eq!(close_action(true, true), CloseAction::Hide);
    }

    #[test]
    fn the_switch_turns_closing_back_into_quitting() {
        assert_eq!(close_action(true, false), CloseAction::Exit);
    }

    #[test]
    fn a_window_that_never_opened_quits() {
        assert_eq!(close_action(false, true), CloseAction::Exit);
    }
}
