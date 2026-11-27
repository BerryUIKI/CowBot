//! System notifications for work that finishes while the window is out of sight.
//!
//! The shell is resident (ADR-0064), so a crew run or a waiting approval can
//! complete with nothing on screen. Two events are worth a notification: an
//! approval that is blocking a run, and a task run that settled. Channel replies
//! are deliberately excluded — a user posting in several channels would be
//! notified about work they just started and can already see.
//!
//! Notifications are dropped unless the window is actually out of sight, and
//! unless the device has them switched on. Both answers are read at the moment
//! of the event, so the setting takes effect without a restart.
//!
//! The copy is English: the interface locale lives with the frontend, and a
//! notification can fire while no window exists to report it (ADR-0065).

use std::collections::HashSet;

use cowbot_client::Client;
use cowbot_core::event::EventPayload;
use cowbot_core::task::TaskId;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::{config, shell, AppState};

/// Whether the window is in front of the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Hidden to the tray, or minimised.
    Hidden,
    /// Open and not minimised.
    Shown,
}

/// Whether an event of this kind should raise a notification.
///
/// Pure so the rule can be tested without a window: a notification the user is
/// already looking at is noise, and a switch that is off means silence.
pub fn wants_notification(enabled: bool, visibility: Visibility) -> bool {
    enabled && visibility == Visibility::Hidden
}

/// Track the client's events and notify for the ones worth interrupting for.
///
/// Spawns its own task, like [`crate::install_events`]. The deduplication set
/// lives in that task, so it lasts exactly as long as this subscription.
pub fn install(app: &AppHandle, client: std::sync::Arc<dyn Client>) {
    // A click on a notification body is how a user gets back to the work it
    // named. Registered once, before any notification can be shown.
    let handle = app.clone();
    let _ = app.notification().on_action(move |action| {
        if action.action_id() == "tap" {
            shell::reveal(&handle);
        }
    });

    let mut events = client.subscribe();
    let handle = app.clone();
    let tasks = client.clone();
    tauri::async_runtime::spawn(async move {
        // `TaskCompleted` and `TaskFailed` are published per assignment, so a
        // crew task settles more than once. Only the first settle is news.
        let mut announced: HashSet<TaskId> = HashSet::new();
        while let Some(event) = events.recv().await {
            if !wants_notification(enabled(&handle), visibility(&handle)) {
                continue;
            }
            match event.payload {
                EventPayload::ApprovalRequested { call, reason, .. } => {
                    let detail = match call {
                        Some(call) => format!("{} · {reason}", call.name),
                        None => reason,
                    };
                    show(&handle, "CowBot needs your approval", &detail);
                }
                EventPayload::TaskCompleted { task_id, .. } => {
                    if !announced.insert(task_id.clone()) {
                        continue;
                    }
                    let title = task_title(&tasks, &task_id).await;
                    show(&handle, "Task finished", &title);
                }
                EventPayload::TaskFailed { task_id, error } => {
                    if !announced.insert(task_id.clone()) {
                        continue;
                    }
                    let title = task_title(&tasks, &task_id).await;
                    show(&handle, "Task failed", &format!("{title}: {error}"));
                }
                _ => {}
            }
        }
    });
}

/// Show one notification, reporting a failure rather than propagating it.
///
/// A platform that refuses the toast must not stop the stream: the run it
/// describes has already finished, and the next event is unrelated.
fn show(app: &AppHandle, title: &str, body: &str) {
    if let Err(error) = app.notification().builder().title(title).body(body).show() {
        eprintln!("warning: a notification could not be shown: {error}");
    }
}

/// The task's own title, falling back to its id.
///
/// A lookup that fails must not swallow the notification — the id still tells
/// the user which task it was about when they open the window.
async fn task_title(client: &std::sync::Arc<dyn Client>, task_id: &TaskId) -> String {
    match client.get_task(task_id).await {
        Ok(Some(task)) if !task.title.trim().is_empty() => task.title,
        _ => task_id.0.clone(),
    }
}

/// Whether this device wants notifications, read fresh so the switch is live.
fn enabled(app: &AppHandle) -> bool {
    config_path(app)
        .and_then(|path| match config::load(&path) {
            Ok(config) => Some(config.notifications_enabled),
            Err(error) => {
                eprintln!(
                    "warning: failed to read {}: {error}; keeping notifications on",
                    path.display()
                );
                None
            }
        })
        .unwrap_or(true)
}

/// Whether the main window is out of sight right now.
///
/// An unreadable window state counts as shown: a notification the user can
/// already see is worse than one that was never sent.
fn visibility(app: &AppHandle) -> Visibility {
    let Some(window) = app.get_webview_window("main") else {
        return Visibility::Shown;
    };
    let hidden =
        matches!(window.is_visible(), Ok(false)) || matches!(window.is_minimized(), Ok(true));
    if hidden {
        Visibility::Hidden
    } else {
        Visibility::Shown
    }
}

fn config_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.try_state::<AppState>()
        .map(|state| state.config_path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hidden_window_gets_the_notification() {
        assert!(wants_notification(true, Visibility::Hidden));
    }

    #[test]
    fn a_visible_window_does_not() {
        // The user is looking at the result; a toast would be noise.
        assert!(!wants_notification(true, Visibility::Shown));
    }

    #[test]
    fn the_switch_turns_notifications_off() {
        assert!(!wants_notification(false, Visibility::Hidden));
        assert!(!wants_notification(false, Visibility::Shown));
    }
}
