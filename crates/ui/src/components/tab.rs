use std::cmp::Ordering;

use gpui::{AnyElement, IntoElement, Stateful};
use smallvec::SmallVec;

use crate::prelude::*;

const START_TAB_SLOT_SIZE: Pixels = px(12.);
const END_TAB_SLOT_SIZE: Pixels = px(14.);

/// The position of a [`Tab`] within a list of tabs.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TabPosition {
    /// The tab is first in the list.
    First,

    /// The tab is in the middle of the list (i.e., it is not the first or last tab).
    ///
    /// The [`Ordering`] is where this tab is positioned with respect to the selected tab.
    Middle(Ordering),

    /// The tab is last in the list.
    Last,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TabCloseSide {
    Start,
    End,
}

#[derive(IntoElement, RegisterComponent)]
pub struct Tab {
    div: Stateful<Div>,
    selected: bool,
    position: TabPosition,
    close_side: TabCloseSide,
    start_slot: Option<AnyElement>,
    start_slot_width: Pixels,
    end_slot: Option<AnyElement>,
    children: SmallVec<[AnyElement; 2]>,
}

impl Tab {
    pub fn new(id: impl Into<ElementId>) -> Self {
        let id = id.into();
        Self {
            div: div()
                .id(id.clone())
                .debug_selector(|| format!("TAB-{}", id)),
            selected: false,
            position: TabPosition::First,
            close_side: TabCloseSide::End,
            start_slot: None,
            start_slot_width: START_TAB_SLOT_SIZE,
            end_slot: None,
            children: SmallVec::new(),
        }
    }

    pub fn position(mut self, position: TabPosition) -> Self {
        self.position = position;
        self
    }

    pub fn close_side(mut self, close_side: TabCloseSide) -> Self {
        self.close_side = close_side;
        self
    }

    pub fn start_slot<E: IntoElement>(mut self, element: impl Into<Option<E>>) -> Self {
        self.start_slot = element.into().map(IntoElement::into_any_element);
        self
    }

    pub fn start_slot_width(mut self, width: Pixels) -> Self {
        self.start_slot_width = width;
        self
    }

    pub fn end_slot<E: IntoElement>(mut self, element: impl Into<Option<E>>) -> Self {
        self.end_slot = element.into().map(IntoElement::into_any_element);
        self
    }

    pub fn content_height(cx: &App) -> Pixels {
        DynamicSpacing::Base32.px(cx) - px(1.)
    }

    pub fn container_height(cx: &App) -> Pixels {
        DynamicSpacing::Base32.px(cx)
    }
}

impl InteractiveElement for Tab {
    fn interactivity(&mut self) -> &mut gpui::Interactivity {
        self.div.interactivity()
    }
}

impl StatefulInteractiveElement for Tab {}

impl Toggleable for Tab {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl ParentElement for Tab {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

impl RenderOnce for Tab {
    #[allow(refining_impl_trait)]
    fn render(self, _: &mut Window, cx: &mut App) -> Stateful<Div> {
        let (text_color, tab_bg, _tab_hover_bg, _tab_active_bg) = match self.selected {
            false => (
                cx.theme().colors().text_muted,
                cx.theme().colors().tab_inactive_background,
                cx.theme().colors().ghost_element_hover,
                cx.theme().colors().ghost_element_active,
            ),
            true => (
                cx.theme().colors().text,
                cx.theme().colors().tab_active_background,
                cx.theme().colors().element_hover,
                cx.theme().colors().element_active,
            ),
        };

        let (start_slot, end_slot) = {
            let start_slot = h_flex()
                .w(self.start_slot_width)
                .h(START_TAB_SLOT_SIZE)
                .flex_none()
                .justify_center()
                .children(self.start_slot);

            let end_slot = h_flex()
                .size(END_TAB_SLOT_SIZE)
                .justify_center()
                .children(self.end_slot);

            match self.close_side {
                TabCloseSide::End => (start_slot, end_slot),
                TabCloseSide::Start => (end_slot, start_slot),
            }
        };

        self.div
            .h(Tab::container_height(cx))
            .bg(tab_bg)
            .border_color(cx.theme().colors().border)
            .map(|this| match self.position {
                TabPosition::First => {
                    if self.selected {
                        this.pl_px().border_r_1().pb_px()
                    } else {
                        this.pl_px().pr_px().border_b_1()
                    }
                }
                TabPosition::Last => {
                    if self.selected {
                        this.border_l_1().border_r_1().pb_px()
                    } else {
                        this.pl_px().border_b_1().border_r_1()
                    }
                }
                TabPosition::Middle(Ordering::Equal) => this.border_l_1().border_r_1().pb_px(),
                TabPosition::Middle(Ordering::Less) => this.border_l_1().pr_px().border_b_1(),
                TabPosition::Middle(Ordering::Greater) => this.border_r_1().pl_px().border_b_1(),
            })
            .cursor_pointer()
            .child(
                h_flex()
                    .group("")
                    .relative()
                    .h(Tab::content_height(cx))
                    .px(DynamicSpacing::Base04.px(cx))
                    .gap(DynamicSpacing::Base04.rems(cx))
                    .text_color(text_color)
                    .child(start_slot)
                    .children(self.children)
                    .child(end_slot),
            )
    }
}

impl Component for Tab {
    fn scope() -> ComponentScope {
        ComponentScope::Navigation
    }

    fn description() -> &'static str {
        "A tab component that can be used in a tabbed interface, \
        supporting different positions and states."
    }

    fn preview(_window: &mut Window, _cx: &mut App) -> AnyElement {
        v_flex()
            .gap_6()
            .children(vec![example_group_with_title(
                "Variations",
                vec![
                    single_example(
                        "Default",
                        Tab::new("default").child("Default Tab").into_any_element(),
                    ),
                    single_example(
                        "Selected",
                        Tab::new("selected")
                            .toggle_state(true)
                            .child("Selected Tab")
                            .into_any_element(),
                    ),
                    single_example(
                        "First",
                        Tab::new("first")
                            .position(TabPosition::First)
                            .child("First Tab")
                            .into_any_element(),
                    ),
                    single_example(
                        "Middle",
                        Tab::new("middle")
                            .position(TabPosition::Middle(Ordering::Equal))
                            .child("Middle Tab")
                            .into_any_element(),
                    ),
                    single_example(
                        "Last",
                        Tab::new("last")
                            .position(TabPosition::Last)
                            .child("Last Tab")
                            .into_any_element(),
                    ),
                ],
            )])
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ButtonLike, ButtonSize, ContextMenu, PopoverMenu, PopoverMenuHandle, TabBar,
        right_click_menu,
    };
    use gpui::{Modifiers, MouseButton, TestAppContext, point};

    struct TabMenuTestView {
        menu: PopoverMenuHandle<ContextMenu>,
        tab_clicks: usize,
    }

    impl Render for TabMenuTestView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let trigger = PopoverMenu::new("color-menu")
                .with_handle(self.menu.clone())
                .trigger(
                    ButtonLike::new("color-button")
                        .size(ButtonSize::Default)
                        .width(px(22.))
                        .height(px(22.).into())
                        .child(div().size(px(14.)).debug_selector(|| "COLOR_ICON".into())),
                )
                .menu(|window, cx| {
                    Some(ContextMenu::build(window, cx, |menu, _, _| {
                        menu.header("Tab Color")
                    }))
                });
            let tab = Tab::new("chat")
                .start_slot(trigger)
                .start_slot_width(px(22.))
                .child("Chat")
                .on_click(cx.listener(|this, _, _, _| this.tab_clicks += 1));
            div().size_full().child(
                TabBar::new("chat-tabs").child(
                    right_click_menu("chat-menu")
                        .trigger(move |_, _, _| tab)
                        .menu(|window, cx| {
                            ContextMenu::build(window, cx, |menu, _, _| menu.header("Tab Color"))
                        }),
                ),
            )
        }
    }

    #[gpui::test]
    fn clicking_tab_icon_opens_menu_without_activating_tab(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
        let menu = PopoverMenuHandle::default();
        let (view, cx) = cx.add_window_view(|_, _| TabMenuTestView {
            menu: menu.clone(),
            tab_clicks: 0,
        });
        let center = cx.debug_bounds("COLOR_ICON").expect("icon bounds").center();
        for offset in [
            point(px(0.), px(0.)),
            point(px(9.), px(0.)),
            point(px(-9.), px(0.)),
            point(px(0.), px(9.)),
            point(px(0.), px(-9.)),
        ] {
            let position = center + offset;
            cx.simulate_mouse_move(position, None, Modifiers::default());
            cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
            cx.run_until_parked();
            assert!(
                menu.is_deployed(),
                "clicking the icon or its padding should open the menu: {offset:?}"
            );
            assert_eq!(view.read_with(cx, |view, _| view.tab_clicks), 0);
            cx.update(|_, cx| menu.hide(cx));
            cx.run_until_parked();
        }
    }
}
