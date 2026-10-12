use std::time::{Duration, Instant};

use auto_update::{AutoUpdateStatus, AutoUpdater};
use gpui::{DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Global, Task, WeakEntity};
use semver::Version;
use ui::prelude::*;
use util::ResultExt;
use workspace::notifications::{
    Notification, NotificationId, SuppressEvent, dismiss_app_notification, show_app_notification,
};

const REMINDER_DELAY: Duration = Duration::from_secs(60 * 60);
const TYPING_DELAY: Duration = Duration::from_secs(3);
const MAX_TYPING_DELAY: Duration = Duration::from_secs(60);

struct GlobalUpdateReminder {
    _reminder: Entity<UpdateReminder>,
}

impl Global for GlobalUpdateReminder {}

pub(super) fn init(cx: &mut App) {
    let Some(updater) = AutoUpdater::get(cx) else {
        return;
    };
    let reminder = cx.new(|cx| {
        cx.observe(&updater, |this: &mut UpdateReminder, updater, cx| {
            this.update_status(updater.read(cx).status(), cx);
        })
        .detach();
        cx.observe_keystrokes(|this: &mut UpdateReminder, _, _, cx| {
            this.last_keystroke = Some(cx.background_executor().now());
        })
        .detach();
        UpdateReminder::default()
    });
    reminder.update(cx, |reminder, cx| {
        reminder.update_status(updater.read(cx).status(), cx);
    });
    cx.set_global(GlobalUpdateReminder {
        _reminder: reminder,
    });
}

#[derive(Default)]
struct UpdateReminder {
    version: Option<Version>,
    visible: bool,
    reminder_task: Option<Task<()>>,
    last_keystroke: Option<Instant>,
    deferred_since: Option<Instant>,
}

impl UpdateReminder {
    fn update_status(&mut self, status: AutoUpdateStatus, cx: &mut Context<Self>) {
        let AutoUpdateStatus::Updated { version } = status else {
            return;
        };
        // Polls temporarily switch to Checking and then restore Updated. Only a
        // different installed version should interrupt the user's snooze.
        if self.version.as_ref() == Some(&version) {
            return;
        }
        self.version = Some(version);
        self.reminder_task = None;
        self.show(cx);
    }

    fn show(&mut self, cx: &mut Context<Self>) {
        let Some(version) = self.version.clone() else {
            return;
        };
        let now = cx.background_executor().now();
        let recently_typing = self
            .last_keystroke
            .is_some_and(|last| now.saturating_duration_since(last) < TYPING_DELAY);
        let defer_limit_reached = self
            .deferred_since
            .is_some_and(|start| now.saturating_duration_since(start) >= MAX_TYPING_DELAY);
        if recently_typing && !defer_limit_reached {
            self.deferred_since.get_or_insert(now);
            self.reminder_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(TYPING_DELAY).await;
                this.update(cx, |this, cx| {
                    this.reminder_task = None;
                    this.show(cx);
                })
                .log_err();
            }));
            return;
        }
        self.deferred_since = None;
        self.visible = true;
        let reminder = cx.entity().downgrade();
        show_app_notification(
            NotificationId::unique::<UpdateReadyNotification>(),
            cx,
            move |cx| {
                cx.new(|cx| UpdateReadyNotification {
                    version: version.clone(),
                    reminder: reminder.clone(),
                    focus_handle: cx.focus_handle(),
                })
            },
        );
    }

    fn snooze(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        dismiss_app_notification(&NotificationId::unique::<UpdateReadyNotification>(), cx);
        self.reminder_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REMINDER_DELAY).await;
            this.update(cx, |this, cx| {
                this.reminder_task = None;
                this.show(cx);
            })
            .log_err();
        }));
    }
}

struct UpdateReadyNotification {
    version: Version,
    reminder: WeakEntity<UpdateReminder>,
    focus_handle: FocusHandle,
}

impl EventEmitter<DismissEvent> for UpdateReadyNotification {}
impl EventEmitter<SuppressEvent> for UpdateReadyNotification {}
impl Notification for UpdateReadyNotification {}

impl Focusable for UpdateReadyNotification {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for UpdateReadyNotification {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let notes: Vec<String> = AutoUpdater::get(cx)
            .and_then(|updater| {
                updater.read(cx).pending_release_notes().map(|notes| {
                    notes
                        .lines()
                        .filter_map(|line| line.trim().strip_prefix("- ").map(str::to_owned))
                        .take(2)
                        .collect()
                })
            })
            .unwrap_or_default();
        v_flex()
            .id("update-ready-notification")
            .occlude()
            .w_full()
            .p_3()
            .gap_2()
            .elevation_3(cx)
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::ArrowCircle).color(Color::Accent))
                    .child(Label::new(format!("Brainz {} is ready", self.version))),
            )
            .child(Label::new("Restart to use the new version.").color(Color::Muted))
            .children(
                notes
                    .into_iter()
                    .map(|note| Label::new(note).size(LabelSize::Small).color(Color::Muted)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("update-later", "Later")
                            .tooltip(ui::Tooltip::text("Remind me in one hour"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reminder
                                    .update(cx, |this, cx| this.snooze(cx))
                                    .log_err();
                            })),
                    )
                    .child(
                        Button::new("update-and-restart", "Update & Restart")
                            .style(ButtonStyle::Filled)
                            .on_click(|_, _, cx| {
                                // Leave the prompt available if an unsaved-work or
                                // restart confirmation is cancelled.
                                workspace::reload(cx);
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn update_prompt_waits_until_typing_pauses(cx: &mut TestAppContext) {
        let reminder = cx.new(|_| UpdateReminder::default());
        reminder.update(cx, |this, cx| {
            this.last_keystroke = Some(cx.background_executor().now());
            this.update_status(
                AutoUpdateStatus::Updated {
                    version: Version::new(0, 2, 0),
                },
                cx,
            );
        });
        cx.run_until_parked();
        assert!(!reminder.read_with(cx, |this, _| this.visible));
        cx.executor().advance_clock(TYPING_DELAY);
        cx.run_until_parked();
        assert!(reminder.read_with(cx, |this, _| this.visible));
        cx.update(|cx| {
            dismiss_app_notification(&NotificationId::unique::<UpdateReadyNotification>(), cx)
        });
    }

    #[gpui::test]
    fn update_reminder_waits_for_install_and_survives_regular_checks(cx: &mut TestAppContext) {
        let reminder = cx.new(|_| UpdateReminder::default());
        let version = Version::new(0, 1, 3);
        for status in [
            AutoUpdateStatus::Idle,
            AutoUpdateStatus::Checking,
            AutoUpdateStatus::Downloading {
                version: version.clone(),
                progress: Some(0.5),
            },
            AutoUpdateStatus::Installing {
                version: version.clone(),
            },
        ] {
            reminder.update(cx, |this, cx| this.update_status(status, cx));
            assert!(!reminder.read_with(cx, |this, _| this.visible));
        }
        reminder.update(cx, |this, cx| {
            this.update_status(AutoUpdateStatus::Updated { version }, cx)
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(30));
        cx.run_until_parked();
        assert!(reminder.read_with(cx, |this, _| this.visible));
        reminder.update(cx, |this, cx| {
            this.update_status(AutoUpdateStatus::Checking, cx)
        });
        assert!(reminder.read_with(cx, |this, _| this.visible));
        cx.update(|cx| {
            dismiss_app_notification(&NotificationId::unique::<UpdateReadyNotification>(), cx)
        });
    }

    #[gpui::test]
    fn later_snoozes_for_one_hour_without_poll_reset(cx: &mut TestAppContext) {
        let reminder = cx.new(|_| UpdateReminder::default());
        let status = AutoUpdateStatus::Updated {
            version: Version::new(0, 1, 3),
        };
        reminder.update(cx, |this, cx| {
            this.update_status(status.clone(), cx);
            this.snooze(cx);
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(REMINDER_DELAY - Duration::from_secs(1));
        cx.run_until_parked();
        reminder.update(cx, |this, cx| {
            this.update_status(AutoUpdateStatus::Checking, cx);
            this.update_status(status, cx);
        });
        assert!(!reminder.read_with(cx, |this, _| this.visible));
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert!(reminder.read_with(cx, |this, _| this.visible));
        reminder.update(cx, |this, cx| this.snooze(cx));
        cx.run_until_parked();
        assert!(!reminder.read_with(cx, |this, _| this.visible));
        reminder.update(cx, |this, cx| {
            this.update_status(
                AutoUpdateStatus::Updated {
                    version: Version::new(0, 1, 4),
                },
                cx,
            )
        });
        assert!(reminder.read_with(cx, |this, _| this.visible));
        assert!(reminder.read_with(cx, |this, _| this.reminder_task.is_none()));
        cx.update(|cx| {
            dismiss_app_notification(&NotificationId::unique::<UpdateReadyNotification>(), cx)
        });
    }
}
