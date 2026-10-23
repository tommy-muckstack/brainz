//! Brainz: the yellow Create button at the top of the sidebar. It opens a
//! menu beside itself that makes a canvas, note, folder, to-do or calendar
//! invite. The to-do and invite are small forms: a to-do lands under the
//! board's "Today" section; an invite opens Google Calendar's new-event page
//! filled in, because EventKit can write events but cannot add guests.

use std::{path::PathBuf, rc::Rc, time::Duration};

use anyhow::{Context as _, Result, anyhow};
use chrono::{Datelike as _, Days, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone as _, Utc};
use editor::Editor;
use gpui::{
    Action, Anchor, Animation, AnimationExt, App, ClickEvent, CursorStyle, DismissEvent, Entity,
    EventEmitter, FocusHandle, Focusable, Transformation, WeakEntity, Window, actions,
    ease_out_quint, percentage, point,
};
use ui::{ButtonLike, PopoverMenu, Transformable as _, prelude::*};
use workspace::{ModalView, Workspace};

use crate::{brain_config::BrainConfig, open_loops};

actions!(
    brainz_create,
    [
        /// Add a to-do to the board's "Today" section.
        NewTodo,
        /// Draft a calendar invite and finish it in Google Calendar.
        NewCalendarInvite,
    ]
);

const MENU_OPEN_MS: u64 = 220;
/// Each row starts this much after the one above it.
const ROW_STAGGER_MS: u64 = 30;

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &NewTodo, window, cx| {
            open_form(workspace, FormKind::Todo, window, cx)
        });
        workspace.register_action(|workspace, _: &NewCalendarInvite, window, cx| {
            open_form(workspace, FormKind::Invite, window, cx)
        });
    })
    .detach();
}

/// Runs a sidebar-owned step, such as starting the inline "new folder" row
/// in the file tree, which only the project panel can do.
pub type SidebarCallback = Rc<dyn Fn(&mut Window, &mut App)>;

/// The Create button, rendered by the project panel above the navigation.
pub fn render(on_new_folder: SidebarCallback, _cx: &mut App) -> gpui::AnyElement {
    let trigger = CreateTrigger {
        open: false,
        on_click: None,
        cursor_style: None,
    };
    div()
        .w_full()
        .px_1()
        .pt_2()
        .child(
            PopoverMenu::new("brainz-create")
                .full_width(true)
                .trigger(trigger)
                .anchor(Anchor::TopLeft)
                .attach(Anchor::TopRight)
                .offset(point(px(6.), px(0.)))
                .menu(move |window, cx| {
                    let on_new_folder = on_new_folder.clone();
                    Some(cx.new(|cx| CreateMenu::new(on_new_folder, window, cx)))
                }),
        )
        .into_any_element()
}

/// A quarter turn and a half: the plus spins and settles as a cross.
const PLUS_TURN: f32 = 0.375;

/// The yellow face of the Create button. The popover tells it whether the
/// menu is open, and its plus spins into a cross and back.
#[derive(IntoElement)]
struct CreateTrigger {
    open: bool,
    on_click: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
    cursor_style: Option<CursorStyle>,
}

impl Clickable for CreateTrigger {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    fn cursor_style(mut self, cursor_style: CursorStyle) -> Self {
        self.cursor_style = Some(cursor_style);
        self
    }
}

impl Toggleable for CreateTrigger {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }
}

impl RenderOnce for CreateTrigger {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();
        let accent = colors.text_accent;
        let ink = colors.background;
        // Count changes so only a real open or close spins the plus; the
        // first paint after launch shows it at rest.
        let state = window.use_keyed_state("brainz-create-plus", cx, |_, _| (self.open, 0usize));
        let (was_open, mut turns) = *state.read(cx);
        if was_open != self.open {
            turns += 1;
            state.update(cx, |state, _| *state = (self.open, turns));
        }
        let (from, to) = if self.open {
            (0., PLUS_TURN)
        } else {
            (PLUS_TURN, 0.)
        };
        let plus = Icon::new(IconName::Plus)
            .size(IconSize::Small)
            .color(Color::Custom(ink));
        let plus = if turns == 0 {
            plus.into_any_element()
        } else {
            plus.with_animation(
                ("brainz-create-plus", turns),
                Animation::new(Duration::from_millis(MENU_OPEN_MS)).with_easing(ease_out_quint()),
                move |plus, delta| {
                    plus.transform(Transformation::rotate(percentage(
                        from + (to - from) * delta,
                    )))
                },
            )
            .into_any_element()
        };
        let mut button = ButtonLike::new("brainz-create-trigger")
            .size(ButtonSize::None)
            .style(ButtonStyle::Transparent)
            .full_width()
            .toggle_state(self.open)
            .child(
                h_flex()
                    .id("brainz-create-face")
                    .w_full()
                    .h(px(30.))
                    .pl(px(12.))
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .bg(accent)
                    .text_color(ink)
                    .hover(|this| this.bg(accent.opacity(0.88)))
                    .child(plus)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Create"),
                    ),
            );
        if let Some(handler) = self.on_click {
            button = button.on_click(move |event, window, cx| handler(event, window, cx));
        }
        if let Some(cursor_style) = self.cursor_style {
            button = button.cursor_style(cursor_style);
        }
        button
    }
}

#[derive(Clone)]
enum Choice {
    Action(fn() -> Box<dyn Action>),
    Folder,
}

struct Entry {
    label: &'static str,
    hint: &'static str,
    icon: IconName,
    choice: Choice,
}

const ENTRIES: &[Entry] = &[
    Entry {
        label: "Canvas",
        hint: "A blank whiteboard in any folder",
        icon: IconName::BrainzWhiteboard,
        choice: Choice::Action(|| Box::new(crate::whiteboard::NewCanvas)),
    },
    Entry {
        label: "Note",
        hint: "A blank note in this folder",
        icon: IconName::FileTextOutlined,
        choice: Choice::Action(|| Box::new(crate::new_note::NewBlankNote)),
    },
    Entry {
        label: "Folder",
        hint: "In the selected folder",
        icon: IconName::Folder,
        choice: Choice::Folder,
    },
    Entry {
        label: "To-do",
        hint: "Added to Today",
        icon: IconName::BrainzCheckboxEmpty,
        choice: Choice::Action(|| Box::new(NewTodo)),
    },
    Entry {
        label: "Calendar invite",
        hint: "Finish in Google Calendar",
        icon: IconName::BrainzCalendar,
        choice: Choice::Action(|| Box::new(NewCalendarInvite)),
    },
];

pub struct CreateMenu {
    focus_handle: FocusHandle,
    selected: Option<usize>,
    on_new_folder: SidebarCallback,
}

impl CreateMenu {
    fn new(on_new_folder: SidebarCallback, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        Self {
            focus_handle,
            selected: None,
            on_new_folder,
        }
    }

    fn choose(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = ENTRIES.get(ix) else {
            return;
        };
        // Dismissing hands focus back to where it was, so the action below
        // reaches the workspace rather than this closing menu.
        cx.emit(DismissEvent);
        match entry.choice.clone() {
            Choice::Action(action) => window.dispatch_action(action(), cx),
            Choice::Folder => {
                let on_new_folder = self.on_new_folder.clone();
                window.defer(cx, move |window, cx| on_new_folder(window, cx));
            }
        }
    }

    fn select_next(&mut self, _: &menu::SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(self.selected.map_or(0, |ix| (ix + 1) % ENTRIES.len()));
        cx.notify();
    }

    fn select_previous(
        &mut self,
        _: &menu::SelectPrevious,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = Some(self.selected.map_or(ENTRIES.len() - 1, |ix| {
            (ix + ENTRIES.len() - 1) % ENTRIES.len()
        }));
        cx.notify();
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.selected {
            self.choose(ix, window, cx);
        }
    }

    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for CreateMenu {}

impl Focusable for CreateMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CreateMenu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let accent = colors.text_accent;
        // The panel unfolds from the button: it slides right out of the
        // sidebar and fades up, then the rows follow one after another.
        v_flex()
            .id("brainz-create-menu")
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .w(px(260.))
            .p_1()
            .gap_0p5()
            .elevation_2(cx)
            .children(ENTRIES.iter().enumerate().map(|(ix, entry)| {
                let selected = self.selected == Some(ix);
                h_flex()
                    .id(("brainz-create-row", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_2p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |this| this.bg(colors.element_hover))
                    .hover(|this| this.bg(colors.element_hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.choose(ix, window, cx)))
                    .child(
                        div()
                            .flex_none()
                            .size(px(26.))
                            .rounded_md()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(accent.opacity(0.14))
                            .child(
                                Icon::new(entry.icon)
                                    .size(IconSize::Small)
                                    .color(Color::Custom(accent)),
                            ),
                    )
                    .child(
                        v_flex().min_w_0().child(Label::new(entry.label)).child(
                            Label::new(entry.hint)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        ),
                    )
                    .with_animation(
                        ("brainz-create-row-in", ix),
                        Animation::new(Duration::from_millis(
                            MENU_OPEN_MS + ROW_STAGGER_MS * ix as u64,
                        ))
                        .with_easing(ease_out_quint()),
                        move |row, delta| {
                            // Hold each row back by its stagger, then ease it in.
                            let total = (MENU_OPEN_MS + ROW_STAGGER_MS * ix as u64) as f32;
                            let start = (ROW_STAGGER_MS * ix as u64) as f32 / total;
                            let t = ((delta - start) / (1. - start)).clamp(0., 1.);
                            row.opacity(t).ml(px(-6. * (1. - t)))
                        },
                    )
            }))
            .with_animation(
                "brainz-create-menu-in",
                Animation::new(Duration::from_millis(MENU_OPEN_MS)).with_easing(ease_out_quint()),
                |menu, delta| menu.opacity(delta).ml(px(-10. * (1. - delta))),
            )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FormKind {
    Todo,
    Invite,
}

fn open_form(
    workspace: &mut Workspace,
    kind: FormKind,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let root = workspace
        .root_paths(cx)
        .first()
        .map(|path| path.to_path_buf());
    let owner = cx.weak_entity();
    workspace.toggle_modal(window, cx, move |window, cx| {
        QuickForm::new(kind, owner, root, window, cx)
    });
}

/// A small modal for the two Create entries that need a few words first.
pub struct QuickForm {
    kind: FormKind,
    workspace: WeakEntity<Workspace>,
    root: Option<PathBuf>,
    fields: Vec<(&'static str, Entity<Editor>)>,
    error: Option<SharedString>,
}

impl QuickForm {
    fn new(
        kind: FormKind,
        workspace: WeakEntity<Workspace>,
        root: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let specs: &[(&'static str, &str, &str)] = match kind {
            FormKind::Todo => &[("To-do", "What needs doing, and by when?", "")],
            FormKind::Invite => &[
                ("Title", "Coffee with Sam", ""),
                ("Date", "today, tomorrow or 2026-10-22", "tomorrow"),
                ("Time", "3pm or 15:30 (blank for all day)", ""),
                ("Minutes", "30", "30"),
                ("Guests", "sam@example.com, alex@example.com", ""),
            ],
        };
        let fields = specs
            .iter()
            .map(|(label, placeholder, text)| {
                let editor = cx.new(|cx| {
                    let mut editor = Editor::single_line(window, cx);
                    editor.set_placeholder_text(placeholder, window, cx);
                    editor.set_text(*text, window, cx);
                    editor
                });
                (*label, editor)
            })
            .collect::<Vec<_>>();
        if let Some((_, first)) = fields.first() {
            window.focus(&first.focus_handle(cx), cx);
        }
        Self {
            kind,
            workspace,
            root,
            fields,
            error: None,
        }
    }

    fn text(&self, ix: usize, cx: &App) -> String {
        self.fields
            .get(ix)
            .map(|(_, editor)| editor.read(cx).text(cx).trim().to_owned())
            .unwrap_or_default()
    }

    fn move_focus(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.fields.len() as isize;
        let current = self
            .fields
            .iter()
            .position(|(_, editor)| editor.focus_handle(cx).is_focused(window))
            .unwrap_or(0) as isize;
        let next = (current + step).rem_euclid(count) as usize;
        window.focus(&self.fields[next].1.focus_handle(cx), cx);
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let result = match self.kind {
            FormKind::Todo => self.add_todo(window, cx),
            FormKind::Invite => self.draft_invite(cx),
        };
        match result {
            Ok(()) => cx.emit(DismissEvent),
            Err(error) => {
                self.error = Some(error.to_string().into());
                cx.notify();
            }
        }
    }

    fn add_todo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<()> {
        let title = self.text(0, cx);
        anyhow::ensure!(!title.is_empty(), "Write what needs doing first.");
        let root = self
            .root
            .clone()
            .context("Open your brain folder to add to-dos.")?;
        let path = root.join(BrainConfig::load(&root).todo);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("Could not read {}", path.display()))?;
        let output = open_loops::insert_board_line(&text, &todo_line(&title), true);
        std::fs::write(&path, output)
            .with_context(|| format!("Could not write {}", path.display()))?;
        if let Some(workspace) = self.workspace.upgrade() {
            window.defer(cx, move |window, cx| {
                workspace.update(cx, |workspace, cx| {
                    crate::todo::TodoView::open(workspace, window, cx)
                });
            });
        }
        Ok(())
    }

    fn draft_invite(&mut self, cx: &mut Context<Self>) -> Result<()> {
        let draft = InviteDraft {
            title: self.text(0, cx),
            date: self.text(1, cx),
            time: self.text(2, cx),
            minutes: self.text(3, cx),
            guests: self.text(4, cx),
        };
        let url = draft.google_calendar_url(Local::now().naive_local())?;
        cx.open_url(&url);
        Ok(())
    }

    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for QuickForm {}

impl ModalView for QuickForm {}

impl Focusable for QuickForm {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.fields
            .first()
            .map(|(_, editor)| editor.focus_handle(cx))
            .unwrap_or_else(|| cx.focus_handle())
    }
}

impl Render for QuickForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let (title, submit) = match self.kind {
            FormKind::Todo => ("New to-do", "Add to Today"),
            FormKind::Invite => ("New calendar invite", "Open in Google Calendar"),
        };
        let accent = colors.text_accent;
        v_flex()
            .key_context("BrainzQuickForm")
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(
                cx.listener(|this, _: &menu::SelectNext, window, cx| {
                    this.move_focus(1, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &menu::SelectPrevious, window, cx| {
                this.move_focus(-1, window, cx)
            }))
            .w(rems(30.))
            .elevation_3(cx)
            .p_3()
            .gap_2()
            .child(Headline::new(title).size(HeadlineSize::XSmall))
            .children(self.fields.iter().map(|(label, editor)| {
                v_flex()
                    .gap_0p5()
                    .when(self.fields.len() > 1, |this| {
                        this.child(
                            Label::new(*label)
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    })
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.editor_background)
                            .child(editor.clone()),
                    )
            }))
            .children(
                self.error
                    .clone()
                    .map(|error| Label::new(error).size(LabelSize::Small).color(Color::Error)),
            )
            .child(
                h_flex()
                    .pt_1()
                    .justify_between()
                    .child(
                        Label::new(match self.kind {
                            FormKind::Todo => "Enter to add · Esc to cancel",
                            FormKind::Invite => "Tab between fields · Enter to open",
                        })
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                    )
                    .child(
                        div()
                            .id("brainz-quick-form-submit")
                            .px_2p5()
                            .py_1()
                            .rounded_md()
                            .bg(accent)
                            .text_color(colors.background)
                            .text_sm()
                            .cursor_pointer()
                            .hover(|this| this.bg(accent.opacity(0.88)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm(&menu::Confirm, window, cx)
                            }))
                            .child(submit),
                    ),
            )
    }
}

fn todo_line(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("- [ ] {title}")
}

struct InviteDraft {
    title: String,
    date: String,
    time: String,
    minutes: String,
    guests: String,
}

impl InviteDraft {
    fn google_calendar_url(&self, now: NaiveDateTime) -> Result<String> {
        anyhow::ensure!(!self.title.is_empty(), "Give the invite a title.");
        let date = parse_date(&self.date, now.date())?;
        let dates = if self.time.is_empty() {
            let next = date
                .checked_add_days(Days::new(1))
                .context("date out of range")?;
            format!("{}/{}", date.format("%Y%m%d"), next.format("%Y%m%d"))
        } else {
            let time = parse_time(&self.time)?;
            let minutes: i64 = if self.minutes.is_empty() {
                30
            } else {
                self.minutes
                    .parse()
                    .ok()
                    .filter(|minutes| (1..=24 * 60).contains(minutes))
                    .ok_or_else(|| anyhow!("Minutes should be a number like 30."))?
            };
            let start = Local
                .from_local_datetime(&date.and_time(time))
                .earliest()
                .context("That time doesn't exist on that day (clocks change).")?
                .with_timezone(&Utc);
            let end = start + chrono::Duration::minutes(minutes);
            format!(
                "{}/{}",
                start.format("%Y%m%dT%H%M%SZ"),
                end.format("%Y%m%dT%H%M%SZ")
            )
        };
        let guests = self
            .guests
            .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
            .filter(|guest| !guest.is_empty())
            .collect::<Vec<_>>();
        if let Some(bad) = guests.iter().find(|guest| !guest.contains('@')) {
            anyhow::bail!("\"{bad}\" isn't an email address.");
        }
        let mut params = vec![
            ("action", "TEMPLATE".to_owned()),
            ("text", self.title.clone()),
            ("dates", dates),
        ];
        if !guests.is_empty() {
            params.push(("add", guests.join(",")));
        }
        Ok(
            url::Url::parse_with_params("https://calendar.google.com/calendar/render", params)?
                .to_string(),
        )
    }
}

fn parse_date(text: &str, today: NaiveDate) -> Result<NaiveDate> {
    let lower = text.trim().to_ascii_lowercase();
    match lower.as_str() {
        "" | "today" => Ok(today),
        "tomorrow" => today
            .checked_add_days(Days::new(1))
            .context("date out of range"),
        _ => NaiveDate::parse_from_str(&lower, "%Y-%m-%d")
            .or_else(|_| {
                // "10/22" means this year, or next year once it has passed.
                let with_year = format!("{lower}/{}", today.format("%Y"));
                NaiveDate::parse_from_str(&with_year, "%m/%d/%Y").map(|date| {
                    if date < today {
                        date.with_year(date.year() + 1).unwrap_or(date)
                    } else {
                        date
                    }
                })
            })
            .map_err(|_| anyhow!("Use a date like today, tomorrow, 10/22 or 2026-10-22.")),
    }
}

fn parse_time(text: &str) -> Result<NaiveTime> {
    let compact = text.trim().to_ascii_lowercase().replace([' ', '.'], "");
    let (body, meridiem) = if let Some(body) = compact.strip_suffix("am") {
        (body, Some(false))
    } else if let Some(body) = compact.strip_suffix("pm") {
        (body, Some(true))
    } else {
        (compact.as_str(), None)
    };
    let (hour, minute) = match body.split_once(':') {
        Some((hour, minute)) => (hour.parse::<u32>().ok(), minute.parse::<u32>().ok()),
        None => (body.parse::<u32>().ok(), Some(0)),
    };
    let error = || anyhow!("Use a time like 3pm, 3:30 pm or 15:30.");
    let (mut hour, minute) = (hour.ok_or_else(error)?, minute.ok_or_else(error)?);
    if let Some(pm) = meridiem {
        anyhow::ensure!((1..=12).contains(&hour), error());
        hour = hour % 12 + if pm { 12 } else { 0 };
    }
    NaiveTime::from_hms_opt(hour, minute, 0).ok_or_else(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 21)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap()
    }

    #[test]
    fn times_read_the_way_people_type_them() {
        let hm = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        assert_eq!(parse_time("3pm").unwrap(), hm(15, 0));
        assert_eq!(parse_time("3:30 PM").unwrap(), hm(15, 30));
        assert_eq!(parse_time("12am").unwrap(), hm(0, 0));
        assert_eq!(parse_time("12 p.m.").unwrap(), hm(12, 0));
        assert_eq!(parse_time("15:30").unwrap(), hm(15, 30));
        assert_eq!(parse_time("9").unwrap(), hm(9, 0));
        assert!(parse_time("13pm").is_err());
        assert!(parse_time("noonish").is_err());
    }

    #[test]
    fn dates_accept_words_iso_and_month_day() {
        let today = now().date();
        let day = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();
        assert_eq!(parse_date("", today).unwrap(), today);
        assert_eq!(parse_date("Tomorrow", today).unwrap(), day(10, 22));
        assert_eq!(parse_date("2026-11-03", today).unwrap(), day(11, 3));
        assert_eq!(parse_date("11/3", today).unwrap(), day(11, 3));
        assert_eq!(
            parse_date("1/5", today).unwrap(),
            NaiveDate::from_ymd_opt(2027, 1, 5).unwrap()
        );
        assert!(parse_date("someday", today).is_err());
    }

    #[test]
    fn all_day_invite_spans_one_date_and_carries_guests() {
        let draft = InviteDraft {
            title: "Coffee & a walk".into(),
            date: "tomorrow".into(),
            time: String::new(),
            minutes: "30".into(),
            guests: "sam@example.com, alex@example.com".into(),
        };
        let url = url::Url::parse(&draft.google_calendar_url(now()).unwrap()).unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["action"], "TEMPLATE");
        assert_eq!(params["text"], "Coffee & a walk");
        assert_eq!(params["dates"], "20261022/20261023");
        assert_eq!(params["add"], "sam@example.com,alex@example.com");
    }

    #[test]
    fn timed_invite_ends_after_its_minutes() {
        let draft = InviteDraft {
            title: "Sync".into(),
            date: "2026-10-22".into(),
            time: "3pm".into(),
            minutes: "45".into(),
            guests: String::new(),
        };
        let url = url::Url::parse(&draft.google_calendar_url(now()).unwrap()).unwrap();
        let dates = url
            .query_pairs()
            .find(|(key, _)| key == "dates")
            .unwrap()
            .1
            .into_owned();
        let (start, end) = dates.split_once('/').unwrap();
        let parse = |s: &str| chrono::NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%SZ").unwrap();
        assert_eq!((parse(end) - parse(start)).num_minutes(), 45);
        assert!(!url.query_pairs().any(|(key, _)| key == "add"));
    }

    #[test]
    fn invite_rejects_missing_title_and_bad_guests() {
        let mut draft = InviteDraft {
            title: String::new(),
            date: String::new(),
            time: String::new(),
            minutes: String::new(),
            guests: String::new(),
        };
        assert!(draft.google_calendar_url(now()).is_err());
        draft.title = "Lunch".into();
        draft.guests = "sam".into();
        assert!(draft.google_calendar_url(now()).is_err());
    }

    #[test]
    fn todo_line_collapses_whitespace() {
        assert_eq!(todo_line("  Call   the dentist "), "- [ ] Call the dentist");
    }
}
