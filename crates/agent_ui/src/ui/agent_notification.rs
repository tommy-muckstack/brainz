use gpui::{
    App, Context, EventEmitter, IntoElement, PlatformDisplay, Size, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowKind, WindowOptions,
    linear_color_stop, linear_gradient, point,
};
use release_channel::ReleaseChannel;
use std::{rc::Rc, time::Duration};
use ui::{Render, prelude::*};

pub struct AgentNotification {
    title: SharedString,
    caption: Option<SharedString>,
    icon: IconName,
    project_name: Option<SharedString>,
    /// Brainz: offers a "Yes" button that approves the waiting tool call
    /// without switching to the conversation.
    can_allow: bool,
}

impl AgentNotification {
    pub fn new(
        title: impl Into<SharedString>,
        caption: Option<SharedString>,
        icon: IconName,
        project_name: Option<impl Into<SharedString>>,
    ) -> Self {
        Self {
            title: title.into(),
            caption: caption,
            icon,
            project_name: project_name.map(|name| name.into()),
            can_allow: false,
        }
    }

    pub fn with_allow(mut self, can_allow: bool) -> Self {
        self.can_allow = can_allow;
        self
    }

    pub fn auto_dismiss(self, requires_response: bool, cx: &mut Context<Self>) -> Self {
        if !requires_response && !self.can_allow {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if let Some(this) = this.upgrade() {
                    this.update(cx, |this, cx| this.dismiss(cx));
                }
            })
            .detach();
        }
        self
    }

    pub fn window_options(screen: Rc<dyn PlatformDisplay>, cx: &App) -> WindowOptions {
        let size = Size {
            width: px(450.),
            height: px(72.),
        };

        let notification_margin_width = px(16.);
        let notification_margin_height = px(-48.);

        let bounds = gpui::Bounds::<Pixels> {
            origin: screen.bounds().top_right()
                - point(
                    size.width + notification_margin_width,
                    notification_margin_height,
                ),
            size,
        };

        let app_id = ReleaseChannel::global(cx).app_id();

        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            display_id: Some(screen.id()),
            window_background: WindowBackgroundAppearance::Transparent,
            app_id: Some(app_id.to_owned()),
            window_min_size: None,
            window_decorations: Some(WindowDecorations::Client),
            tabbing_identifier: None,
            ..Default::default()
        }
    }
}

pub enum AgentNotificationEvent {
    Accepted,
    Dismissed,
    /// Brainz: the user approved the pending tool call from the popup.
    Allowed,
}

impl EventEmitter<AgentNotificationEvent> for AgentNotification {}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    use std::cell::Cell;

    #[gpui::test]
    fn routine_notification_expires_after_five_seconds(cx: &mut TestAppContext) {
        let dismissed = Rc::new(Cell::new(false));
        let notification = cx.new(|cx| {
            AgentNotification::new("Done", None, IconName::Check, None::<SharedString>)
                .auto_dismiss(false, cx)
        });
        let _subscription = cx.update(|cx| {
            cx.subscribe(&notification, {
                let dismissed = dismissed.clone();
                move |_, event, _| {
                    if matches!(event, AgentNotificationEvent::Dismissed) {
                        dismissed.set(true);
                    }
                }
            })
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(4));
        cx.run_until_parked();
        assert!(!dismissed.get());
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert!(dismissed.get());
    }

    #[gpui::test]
    fn permission_and_input_notifications_remain_visible(cx: &mut TestAppContext) {
        for (can_allow, requires_response) in [(true, false), (false, true)] {
            let dismissed = Rc::new(Cell::new(false));
            let notification = cx.new(|cx| {
                AgentNotification::new("Waiting", None, IconName::Info, None::<SharedString>)
                    .with_allow(can_allow)
                    .auto_dismiss(requires_response, cx)
            });
            let _subscription = cx.update(|cx| {
                cx.subscribe(&notification, {
                    let dismissed = dismissed.clone();
                    move |_, event, _| {
                        if matches!(event, AgentNotificationEvent::Dismissed) {
                            dismissed.set(true);
                        }
                    }
                })
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_secs(30));
            cx.run_until_parked();
            assert!(!dismissed.get());
        }
    }
}

impl AgentNotification {
    pub fn accept(&mut self, cx: &mut Context<Self>) {
        cx.emit(AgentNotificationEvent::Accepted);
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        cx.emit(AgentNotificationEvent::Dismissed);
    }

    pub fn allow(&mut self, cx: &mut Context<Self>) {
        cx.emit(AgentNotificationEvent::Allowed);
    }
}

impl Render for AgentNotification {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_font = theme_settings::setup_ui_font(window, cx);
        let line_height = window.line_height();

        let bg = cx.theme().colors().elevated_surface_background;
        let gradient_overflow = || {
            div()
                .h_full()
                .absolute()
                .w_8()
                .bottom_0()
                .right_0()
                .bg(linear_gradient(
                    90.,
                    linear_color_stop(bg, 1.),
                    linear_color_stop(bg.opacity(0.2), 0.),
                ))
        };

        h_flex()
            .id("agent-notification")
            .size_full()
            .p_3()
            .gap_4()
            .justify_between()
            .elevation_3(cx)
            .text_ui(cx)
            .font(ui_font)
            .border_color(cx.theme().colors().border)
            .rounded_xl()
            .child(
                h_flex()
                    .items_start()
                    .gap_2()
                    .flex_1()
                    .child(
                        h_flex().h(line_height).justify_center().child(
                            Icon::new(self.icon)
                                .color(Color::Muted)
                                .size(IconSize::Small),
                        ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .max_w(px(300.))
                            .child(
                                div()
                                    .relative()
                                    .text_size(px(14.))
                                    .text_color(cx.theme().colors().text)
                                    .truncate()
                                    .child(self.title.clone())
                                    .child(gradient_overflow()),
                            )
                            .child(
                                h_flex()
                                    .relative()
                                    .gap_1p5()
                                    .text_size(px(12.))
                                    .text_color(cx.theme().colors().text_muted)
                                    .truncate()
                                    .when_some(
                                        self.project_name.clone(),
                                        |description, project_name| {
                                            let has_caption = self.caption.is_some();
                                            let project = div()
                                                .truncate()
                                                .when(has_caption, |this| this.max_w_16())
                                                .child(project_name);
                                            let mut row = h_flex().gap_1p5().child(project);
                                            if has_caption {
                                                row = row.child(
                                                    div().size(px(3.)).rounded_full().bg(cx
                                                        .theme()
                                                        .colors()
                                                        .text
                                                        .opacity(0.5)),
                                                );
                                            }
                                            description.child(row)
                                        },
                                    )
                                    .when_some(self.caption.clone(), |description, caption| {
                                        description.child(caption)
                                    })
                                    .child(gradient_overflow()),
                            ),
                    ),
            )
            .map(|this| {
                if self.can_allow {
                    this.child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                Button::new("allow", "Yes")
                                    .style(ButtonStyle::Tinted(ui::TintColor::Success))
                                    .start_icon(
                                        Icon::new(IconName::Check)
                                            .size(IconSize::XSmall)
                                            .color(Color::Success),
                                    )
                                    .on_click(cx.listener(move |this, _event, _, cx| {
                                        this.allow(cx);
                                    })),
                            )
                            .child(Button::new("open", "View").on_click(cx.listener(
                                move |this, _event, _, cx| {
                                    this.accept(cx);
                                },
                            )))
                            .child(Button::new("dismiss", "Dismiss").on_click(cx.listener(
                                move |this, _event, _, cx| {
                                    this.dismiss(cx);
                                },
                            ))),
                    )
                } else {
                    this.child(
                        v_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                Button::new("open", "View")
                                    .style(ButtonStyle::Tinted(ui::TintColor::Accent))
                                    .full_width()
                                    .on_click({
                                        cx.listener(move |this, _event, _, cx| {
                                            this.accept(cx);
                                        })
                                    }),
                            )
                            .child(Button::new("dismiss", "Dismiss").full_width().on_click({
                                cx.listener(move |this, _event, _, cx| {
                                    this.dismiss(cx);
                                })
                            })),
                    )
                }
            })
    }
}
