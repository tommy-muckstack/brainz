//! Brainz: the rocket in the status bar. Click opens a new shell tab in the
//! conversation panel; right-click offers Claude and Codex too.

use gpui::{Action as _, App, FocusHandle, Window};
use terminal_view::terminal_panel::{OpenClaude, OpenCodex, OpenShell};
use ui::{ContextMenu, PopoverMenu, Tooltip, prelude::*};
use workspace::{HideStatusItem, ItemHandle, StatusItemView};

pub struct LaunchButton {
    pane_item_focus_handle: Option<FocusHandle>,
}

impl LaunchButton {
    pub fn new() -> Self {
        Self {
            pane_item_focus_handle: None,
        }
    }
}

impl Render for LaunchButton {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.pane_item_focus_handle.clone();
        div().child(
            PopoverMenu::new("brainz-launch-menu")
                .trigger_with_tooltip(
                    IconButton::new("brainz-launch-button", IconName::Launch)
                        .icon_size(IconSize::Small),
                    move |_window, cx| {
                        if let Some(focus_handle) = &focus_handle {
                            Tooltip::for_action_in(
                                "Launch Terminal, Claude, or Codex",
                                &OpenShell,
                                focus_handle,
                                cx,
                            )
                        } else {
                            Tooltip::for_action("Launch Terminal, Claude, or Codex", &OpenShell, cx)
                        }
                    },
                )
                .anchor(gpui::Anchor::BottomLeft)
                .menu(|window, cx| {
                    Some(ContextMenu::build(window, cx, |menu, _, _| {
                        menu.header("Launch")
                            .action("Terminal", OpenShell.boxed_clone())
                            .action("Claude", OpenClaude.boxed_clone())
                            .action("Codex", OpenCodex.boxed_clone())
                    }))
                }),
        )
    }
}

impl StatusItemView for LaunchButton {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_item_focus_handle = active_pane_item.map(|item| item.item_focus_handle(cx));
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}
