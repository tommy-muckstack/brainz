//! Brainz: the sidebar navigation. One short list at the top of the file
//! tree (Brief, Calendar, To-Do, Themes, Connectors) replaces the row of
//! status-bar icons, so every place in the app is reachable from one spot
//! and carries a name.

use gpui::{Animation, AnimationExt, App, WeakEntity, ease_out_quint};
use ui::{Tooltip, prelude::*};
use workspace::Workspace;

struct NavItem {
    key: &'static str,
    label: &'static str,
    icon: IconName,
    action: fn() -> Box<dyn gpui::Action>,
    is_active: fn(&dyn workspace::ItemHandle, &App) -> bool,
}

const ITEMS: &[NavItem] = &[
    NavItem {
        key: "brief",
        label: "Brief",
        icon: IconName::BrainzBrief,
        action: || Box::new(crate::brief::OpenBrief),
        is_active: |item, _| item.downcast::<crate::brief::BriefView>().is_some(),
    },
    NavItem {
        key: "calendar",
        label: "Calendar",
        icon: IconName::BrainzCalendar,
        action: || Box::new(crate::OpenCalendar),
        is_active: |item, _| item.downcast::<crate::CalendarView>().is_some(),
    },
    NavItem {
        key: "todo",
        label: "To-Do",
        icon: IconName::BrainzCheckboxChecked,
        action: || Box::new(crate::todo::OpenTodo),
        is_active: |item, _| item.downcast::<crate::todo::TodoView>().is_some(),
    },
    NavItem {
        key: "themes",
        label: "Themes",
        icon: IconName::BrainzTheme,
        action: || Box::new(crate::themes::OpenThemes),
        is_active: |item, _| item.downcast::<crate::themes::ThemesView>().is_some(),
    },
    NavItem {
        key: "connectors",
        label: "Connectors",
        icon: IconName::BrainzMcp,
        action: || Box::new(crate::mcp::OpenMcp),
        is_active: |item, _| item.downcast::<crate::mcp::McpView>().is_some(),
    },
];

/// The list, rendered by the project panel above the tree.
pub fn render(workspace: &WeakEntity<Workspace>, cx: &mut App) -> gpui::AnyElement {
    let active_item = workspace
        .upgrade()
        .and_then(|workspace| workspace.read(cx).active_item(cx));
    let failing_connectors = crate::mcp::health(cx)
        .map(|health| health.read(cx).failing().len())
        .unwrap_or(0);
    let colors = cx.theme().colors();
    let accent = colors.text_accent;

    // Row padding puts the icons on the same left edge as the file tree's
    // folder icons below.
    v_flex()
        .w_full()
        .px_1()
        .pt_2()
        .pb_1()
        .gap_0p5()
        .children(ITEMS.iter().enumerate().map(|(ix, item)| {
            let active = active_item
                .as_ref()
                .is_some_and(|active| (item.is_active)(active.as_ref(), cx));
            let trouble = item.key == "connectors" && failing_connectors > 0;
            h_flex()
                .id(("brainz-nav", ix))
                .relative()
                .w_full()
                .h(px(30.))
                .px(px(5.))
                .gap_2()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .when(active, |this| this.bg(colors.element_selected))
                .hover(|this| this.bg(colors.element_hover))
                .on_click(move |_, window, cx| {
                    window.dispatch_action((item.action)(), cx);
                })
                .when(active, |this| {
                    // The accent bar slides in from the left as an item
                    // becomes current, the one place the accent lives in
                    // the sidebar.
                    this.child(
                        div()
                            .absolute()
                            .left(px(-2.))
                            .top(px(7.))
                            .w(px(3.))
                            .h(px(16.))
                            .rounded_full()
                            .bg(accent)
                            .with_animation(
                                ("brainz-nav-bar", ix),
                                Animation::new(std::time::Duration::from_millis(260))
                                    .with_easing(ease_out_quint()),
                                |element, delta| {
                                    element.left(px(-2. - 6. * (1. - delta))).opacity(delta)
                                },
                            ),
                    )
                })
                .child(Icon::new(item.icon).size(IconSize::Small).color(if active {
                    Color::Default
                } else {
                    Color::Muted
                }))
                .child(Label::new(item.label).color(if active {
                    Color::Default
                } else {
                    Color::Muted
                }))
                .when(trouble, |this| {
                    this.child(div().flex_1()).child(
                        div()
                            .id(("brainz-nav-trouble", ix))
                            .size(px(7.))
                            .rounded_full()
                            .bg(colors.text_accent)
                            .tooltip(Tooltip::text(format!(
                                "{failing_connectors} connector(s) need attention"
                            ))),
                    )
                })
        }))
        .into_any_element()
}

/// The small heading between the navigation and the file tree.
pub fn render_tree_heading(cx: &mut App) -> gpui::AnyElement {
    let _ = cx;
    div()
        .w_full()
        .px(px(9.))
        .pt_3()
        .pb_1()
        .child(
            Label::new("Notes")
                .size(LabelSize::Small)
                .color(Color::Placeholder),
        )
        .into_any_element()
}
