//! Brainz: the Themes tab. Rising, fading, cross-folder threads, and open
//! loops over the brain's git history, from `ops/themes/signals.json`
//! (written by the signals pass in `themes_signals`), plus the Grokbot
//! narrative block of `ops/themes/themes.md` rendered as Markdown.

use std::{
    collections::HashSet,
    path::PathBuf,
    time::{Duration, SystemTime},
};

use editor::Editor;
use gpui::{
    App, Entity, EventEmitter, FocusHandle, Focusable, Global, Subscription, Task, WeakEntity,
    Window, actions,
};
use markdown::{Markdown, MarkdownElement, MarkdownStyle};
use ui::{ContextMenu, Disclosure, PopoverMenu, Tooltip, prelude::*};
use workspace::{
    HideStatusItem, Item, ItemHandle, OpenOptions, OpenVisible, StatusItemView, Workspace,
};

use crate::{
    brain_config::BrainConfig,
    themes_signals::{self as signals, Signals, Theme},
};

actions!(
    brainz_themes,
    [
        /// Opens the Themes tab.
        OpenThemes,
        /// Runs the themes signals pass on the brain now.
        RunThemesPass
    ]
);

const DISK_POLL: Duration = Duration::from_secs(10);
const DAILY_CHECK: Duration = Duration::from_secs(30 * 60);
const DAILY_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const PICKER_LIMIT: usize = 30;

pub fn init(cx: &mut App) {
    let global_runner = cx.new(ThemesRunner::new);
    cx.set_global(GlobalRunner(global_runner));
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &OpenThemes, window, cx| {
            ThemesView::open(workspace, window, cx);
        });
        workspace.register_action(|workspace, _: &RunThemesPass, _window, cx| {
            let Some(root) = workspace.root_paths(cx).first().map(|path| path.to_path_buf())
            else {
                return;
            };
            if let Some(runner) = runner(cx) {
                runner.update(cx, |runner, cx| runner.run(root, cx));
            }
        });
    })
    .detach();
}

struct GlobalRunner(Entity<ThemesRunner>);

impl Global for GlobalRunner {}

pub fn runner(cx: &App) -> Option<Entity<ThemesRunner>> {
    cx.try_global::<GlobalRunner>().map(|global| global.0.clone())
}

/// Owns the "a pass is running" state so the tab and the daily schedule
/// never run two passes at once, and runs the pass once a day while Brainz
/// is open if `signals.json` is older than a day.
pub struct ThemesRunner {
    running: bool,
    last_error: Option<String>,
    _task: Option<Task<()>>,
    _schedule: Task<()>,
}

impl ThemesRunner {
    fn new(cx: &mut Context<Self>) -> Self {
        let schedule = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(DAILY_CHECK).await;
                if this.update(cx, |this, cx| this.daily_tick(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            running: false,
            last_error: None,
            _task: None,
            _schedule: schedule,
        }
    }

    fn daily_tick(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Some(repo) = crate::github_sync::state(cx)
            .and_then(|state| state.read(cx).repo().cloned())
        else {
            return;
        };
        let config = BrainConfig::load(&repo);
        let stale = signals::signals_age(&repo, &config).is_none_or(|age| age > DAILY_AGE);
        if stale {
            self.run(repo, cx);
        }
    }

    pub fn run(&mut self, repo: PathBuf, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        self.running = true;
        self.last_error = None;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { signals::run_pass(&repo) })
                .await;
            this.update(cx, |this, cx| {
                this.running = false;
                if let Err(error) = result {
                    log::error!("themes pass failed: {error:#}");
                    this.last_error = Some(format!("{error:#}"));
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn is_running(&self) -> bool {
        self.running
    }
}

pub struct ThemesView {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    repo: PathBuf,
    config: BrainConfig,
    signals: Option<Signals>,
    error: Option<String>,
    narrative_text: Option<String>,
    narrative: Option<Entity<Markdown>>,
    themes_md_modified: Option<SystemTime>,
    expanded: HashSet<String>,
    renaming: Option<(String, Entity<Editor>)>,
    merging: Option<String>,
    _runner_observation: Option<Subscription>,
    _rename_subscription: Option<Subscription>,
    _disk_poll: Task<()>,
    _load: Option<Task<()>>,
}

impl ThemesView {
    pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        let existing = workspace.items_of_type::<ThemesView>(cx).next();
        if let Some(existing) = existing {
            workspace.activate_item(&existing, true, true, window, cx);
            existing.update(cx, |view, cx| view.reload(cx));
            return;
        }
        let Some(root) = workspace.root_paths(cx).first().map(|path| path.to_path_buf()) else {
            return;
        };
        let weak = cx.entity().downgrade();
        let view = cx.new(|cx| ThemesView::new(weak, root, cx));
        workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
    }

    fn new(workspace: WeakEntity<Workspace>, repo: PathBuf, cx: &mut Context<Self>) -> Self {
        let runner_observation = runner(cx).map(|runner| {
            cx.observe(&runner, |this, runner, cx| {
                if !runner.read(cx).is_running() {
                    this.error = runner.read(cx).last_error.clone();
                    this.reload(cx);
                }
                cx.notify();
            })
        });
        let disk_poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(DISK_POLL).await;
                let changed = this.update(cx, |this, cx| {
                    let modified = signals::themes_md_modified(&this.repo, &this.config);
                    if modified != this.themes_md_modified {
                        this.themes_md_modified = modified;
                        this.reload(cx);
                        true
                    } else {
                        false
                    }
                });
                if changed.is_err() {
                    break;
                }
            }
        });
        let config = BrainConfig::load(&repo);
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            workspace,
            themes_md_modified: signals::themes_md_modified(&repo, &config),
            repo,
            config,
            signals: None,
            error: None,
            narrative_text: None,
            narrative: None,
            expanded: HashSet::new(),
            renaming: None,
            merging: None,
            _runner_observation: runner_observation,
            _rename_subscription: None,
            _disk_poll: disk_poll,
            _load: None,
        };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        let config = self.config.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let signals = signals::load_signals(&repo, &config);
                    let narrative = signals::load_narrative(&repo, &config);
                    (signals, narrative)
                })
                .await;
            this.update(cx, |this, cx| {
                let (signals, narrative) = result;
                match signals {
                    Ok(signals) => {
                        this.signals = Some(signals);
                        this.error = None;
                    }
                    Err(error) => {
                        if this.signals.is_none() {
                            this.error = Some(format!("{error:#}"));
                        }
                    }
                }
                if narrative != this.narrative_text {
                    this.narrative = narrative.as_ref().map(|text| {
                        cx.new(|cx| Markdown::new(text.clone().into(), None, None, cx))
                    });
                    this.narrative_text = narrative;
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn run_now(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        if let Some(runner) = runner(cx) {
            runner.update(cx, |runner, cx| runner.run(repo, cx));
        }
    }

    /// Appends one line to `pins.md` and re-runs the pass.
    fn curate(&mut self, line: String, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        let config = self.config.clone();
        self.merging = None;
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = {
                let repo = repo.clone();
                cx.background_spawn(async move { signals::append_pin(&repo, &config, &line) })
                    .await
            };
            this.update(cx, |this, cx| match result {
                Ok(()) => this.run_now(cx),
                Err(error) => {
                    this.error = Some(format!("{error:#}"));
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn toggle_pin(&mut self, theme: &Theme, cx: &mut Context<Self>) {
        let line = format!("- pin: {}", theme.id);
        if theme.pinned {
            let repo = self.repo.clone();
            let config = self.config.clone();
            self._load = Some(cx.spawn(async move |this, cx| {
                let result = {
                    let repo = repo.clone();
                    cx.background_spawn(async move {
                        signals::remove_pin_line(&repo, &config, &line)
                    })
                    .await
                };
                this.update(cx, |this, cx| match result {
                    Ok(()) => this.run_now(cx),
                    Err(error) => {
                        this.error = Some(format!("{error:#}"));
                        cx.notify();
                    }
                })
                .ok();
            }));
        } else {
            self.curate(line, cx);
        }
    }

    fn start_rename(&mut self, theme: &Theme, window: &mut Window, cx: &mut Context<Self>) {
        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_text(theme.name.clone(), window, cx);
            editor
        });
        self._rename_subscription = Some(cx.subscribe_in(
            &editor,
            window,
            |this, _editor, event: &editor::EditorEvent, window, cx| {
                if matches!(event, editor::EditorEvent::Blurred) {
                    this.commit_rename(window, cx);
                }
            },
        ));
        editor.update(cx, |editor, cx| {
            editor.select_all(&editor::actions::SelectAll, window, cx);
            editor.focus_handle(cx).focus(window, cx);
        });
        self.renaming = Some((theme.id.clone(), editor));
        cx.notify();
    }

    fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, editor)) = self.renaming.take() else {
            return;
        };
        self._rename_subscription = None;
        let name = editor.read(cx).text(cx).trim().to_owned();
        self.focus_handle.focus(window, cx);
        let unchanged = self
            .theme(&id)
            .is_some_and(|theme| theme.name == name || name.is_empty());
        if !unchanged {
            self.curate(format!("- rename: {id} => {name}"), cx);
        }
        cx.notify();
    }

    fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            self._rename_subscription = None;
            self.focus_handle.focus(window, cx);
            cx.notify();
        }
    }

    fn theme(&self, id: &str) -> Option<&Theme> {
        self.signals
            .as_ref()?
            .themes
            .iter()
            .find(|theme| theme.id == id)
    }

    fn open_relative(&self, relative: &str, window: &mut Window, cx: &mut Context<Self>) {
        let path = signals::repo_path(&self.repo, relative);
        if !path.exists() {
            return;
        }
        self.workspace
            .update(cx, |workspace, cx| {
                workspace
                    .open_abs_path(
                        path,
                        OpenOptions {
                            visible: Some(OpenVisible::None),
                            ..Default::default()
                        },
                        window,
                        cx,
                    )
                    .detach_and_log_err(cx);
            })
            .ok();
    }

    fn open_person(&self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let relative = self.config.person_file(name);
        self.open_relative(&relative, window, cx);
    }

    fn render_run_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let running = runner(cx).is_some_and(|runner| runner.read(cx).is_running());
        let amber = cx.theme().colors().text_accent;
        if running {
            h_flex()
                .h(px(24.))
                .px_3()
                .items_center()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().colors().border)
                .child(ui::bouncing_dots("brainz-themes-running", amber))
                .into_any_element()
        } else {
            Button::new("brainz-themes-run", "Run now")
                .style(ButtonStyle::Filled)
                .label_size(LabelSize::Small)
                .tooltip(Tooltip::text(
                    "Recompute themes from the brain's git history (a few seconds)",
                ))
                .on_click(cx.listener(|this, _, _, cx| this.run_now(cx)))
                .into_any_element()
        }
    }

    fn render_theme_row(
        &self,
        theme: &Theme,
        ix: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = theme.id.clone();
        let expanded = self.expanded.contains(&id);
        let renaming_editor = self
            .renaming
            .as_ref()
            .filter(|(renaming_id, _)| *renaming_id == id)
            .map(|(_, editor)| editor.clone());
        let folders = theme
            .folders
            .iter()
            .map(|folder| folder.name.as_str())
            .collect::<Vec<_>>()
            .join(" · ");
        let momentum = signals::momentum_label(theme);
        let momentum_color = if theme.is_new || theme.momentum.is_some_and(|m| m >= 1.2) {
            Color::Accent
        } else if theme.momentum.is_some_and(|m| m <= 0.5) {
            Color::Muted
        } else {
            Color::Default
        };

        let name: AnyElement = match renaming_editor {
            Some(editor) => div()
                .min_w(px(160.))
                .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| {
                    this.commit_rename(window, cx)
                }))
                .on_action(cx.listener(|this, _: &menu::Cancel, window, cx| {
                    this.cancel_rename(window, cx)
                }))
                .child(editor)
                .into_any_element(),
            None => Button::new(("brainz-theme-name", ix), theme.name.clone())
                .label_size(LabelSize::Default)
                .tooltip(Tooltip::text("Show files and people"))
                .on_click(cx.listener({
                    let id = id.clone();
                    move |this, _, _, cx| {
                        if !this.expanded.remove(&id) {
                            this.expanded.insert(id.clone());
                        }
                        cx.notify();
                    }
                }))
                .into_any_element(),
        };

        let pin_theme = theme.clone();
        let menu_theme = theme.clone();
        let header = h_flex()
            .w_full()
            .items_center()
            .gap_2()
            .px_1()
            .py_1()
            .rounded_md()
            .hover(|this| this.bg(cx.theme().colors().element_hover))
            .child(
                Disclosure::new(("brainz-theme-disclosure", ix), expanded).on_click(cx.listener({
                    let id = id.clone();
                    move |this, _, _, cx| {
                        if !this.expanded.remove(&id) {
                            this.expanded.insert(id.clone());
                        }
                        cx.notify();
                    }
                })),
            )
            .child(div().min_w(px(110.)).max_w(px(180.)).overflow_hidden().child(name))
            .child(
                Label::new(signals::sparkline(&theme.series))
                    .buffer_font(cx)
                    .color(Color::Accent),
            )
            .child(
                Label::new(momentum)
                    .size(LabelSize::XSmall)
                    .color(momentum_color),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Label::new(folders)
                        .size(LabelSize::XSmall)
                        .color(Color::Muted)
                        .truncate(),
                ),
            )
            .child(
                IconButton::new(("brainz-theme-pin", ix), IconName::Star)
                    .icon_size(IconSize::XSmall)
                    .icon_color(if theme.pinned {
                        Color::Accent
                    } else {
                        Color::Muted
                    })
                    .tooltip(Tooltip::text(if theme.pinned { "Unpin" } else { "Pin" }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_pin(&pin_theme, cx);
                    })),
            )
            .child(
                PopoverMenu::new(("brainz-theme-menu", ix))
                    .trigger(
                        IconButton::new(("brainz-theme-more", ix), IconName::Ellipsis)
                            .icon_size(IconSize::XSmall)
                            .icon_color(Color::Muted),
                    )
                    .menu({
                        let view = cx.entity().downgrade();
                        move |window, cx| {
                            let theme = menu_theme.clone();
                            let view = view.clone();
                            Some(ContextMenu::build(window, cx, move |menu, _, _| {
                                let rename_theme = theme.clone();
                                let rename_view = view.clone();
                                let merge_theme = theme.clone();
                                let merge_view = view.clone();
                                let hide_theme = theme.clone();
                                let hide_view = view;
                                menu.entry("Rename", None, move |window, cx| {
                                    rename_view
                                        .update(cx, |view, cx| {
                                            view.start_rename(&rename_theme, window, cx)
                                        })
                                        .ok();
                                })
                                .entry("Merge into…", None, move |_, cx| {
                                    merge_view
                                        .update(cx, |view, cx| {
                                            view.merging = Some(merge_theme.id.clone());
                                            cx.notify();
                                        })
                                        .ok();
                                })
                                .separator()
                                .entry("Hide", None, move |_, cx| {
                                    hide_view
                                        .update(cx, |view, cx| {
                                            view.curate(format!("- hide: {}", hide_theme.id), cx)
                                        })
                                        .ok();
                                })
                            }))
                        }
                    }),
            );

        let mut row = v_flex().w_full().child(header);

        if self.merging.as_deref() == Some(id.as_str()) {
            let mut picker = h_flex().flex_wrap().gap_1().pl_8().py_1().child(
                Label::new("Merge into:")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            );
            let candidates: Vec<(String, String)> = self
                .signals
                .as_ref()
                .map(|signals| {
                    signals
                        .themes
                        .iter()
                        .filter(|other| other.id != id && !other.hidden)
                        .take(PICKER_LIMIT)
                        .map(|other| (other.id.clone(), other.name.clone()))
                        .collect()
                })
                .unwrap_or_default();
            for (candidate_ix, (_, candidate_name)) in candidates.into_iter().enumerate() {
                let source = id.clone();
                let target = candidate_name.clone();
                picker = picker.child(
                    Button::new(("brainz-merge-target", ix * 1000 + candidate_ix), candidate_name)
                        .label_size(LabelSize::XSmall)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.curate(format!("- merge: {source} => {target}"), cx);
                        })),
                );
            }
            picker = picker.child(
                Button::new(("brainz-merge-cancel", ix), "Cancel")
                    .label_size(LabelSize::XSmall)
                    .color(Color::Muted)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.merging = None;
                        cx.notify();
                    })),
            );
            row = row.child(picker);
        }

        if expanded {
            let mut details = v_flex().pl_8().pb_2().gap_1();
            let mut files = h_flex().flex_wrap().gap_1().child(
                Label::new("Files")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            );
            for (file_ix, file) in theme.files.iter().enumerate() {
                let path = file.name.clone();
                let label = signals::file_label(&file.name);
                files = files.child(
                    Button::new(("brainz-theme-file", ix * 100 + file_ix), label)
                        .label_size(LabelSize::XSmall)
                        .start_icon(Icon::new(IconName::File).size(IconSize::XSmall).color(Color::Muted))
                        .tooltip(Tooltip::text(format!("{} · {}", file.name, file.weight)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_relative(&path, window, cx);
                        })),
                );
            }
            details = details.child(files);
            if !theme.people.is_empty() {
                let mut people = h_flex().flex_wrap().gap_1().child(
                    Label::new("People")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                );
                for (person_ix, person) in theme.people.iter().enumerate() {
                    let name = person.name.clone();
                    people = people.child(
                        Button::new(("brainz-theme-person", ix * 100 + person_ix), person.name.clone())
                            .label_size(LabelSize::XSmall)
                            .tooltip(Tooltip::text(format!("Co-mentioned {} times", person.weight)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_person(&name, window, cx);
                            })),
                    );
                }
                details = details.child(people);
            }
            row = row.child(details);
        }
        row.into_any_element()
    }

    fn section_title(&self, title: &'static str, count: usize) -> AnyElement {
        h_flex()
            .px_1()
            .pt_4()
            .pb_1()
            .gap_2()
            .child(
                Label::new(title)
                    .size(LabelSize::Small)
                    .weight(gpui::FontWeight::SEMIBOLD)
                    .color(Color::Accent),
            )
            .child(
                Label::new(count.to_string())
                    .size(LabelSize::XSmall)
                    .color(Color::Placeholder),
            )
            .into_any_element()
    }

    fn render_theme_list(
        &self,
        title: &'static str,
        ids: &[String],
        ix_base: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let themes: Vec<Theme> = ids
            .iter()
            .filter_map(|id| self.theme(id).cloned())
            .filter(|theme| !theme.hidden)
            .collect();
        let mut block = v_flex()
            .w_full()
            .child(self.section_title(title, themes.len()));
        if themes.is_empty() {
            block = block.child(
                div().px_2().py_1().child(
                    Label::new("Nothing here yet")
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                ),
            );
        }
        for (ix, theme) in themes.iter().enumerate() {
            block = block.child(self.render_theme_row(theme, ix_base + ix, window, cx));
        }
        block.into_any_element()
    }

    fn render_open_loops(&self, signals: &Signals) -> AnyElement {
        let total_now: u32 = signals.open_loops.iter().map(|loops| loops.now).sum();
        let mut block = v_flex()
            .w_full()
            .child(self.section_title("Open loops", total_now as usize));
        for loops in &signals.open_loops {
            let delta = loops.now as i64 - loops.week_ago as i64;
            block = block.child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py_0p5()
                    .gap_3()
                    .child(div().w(px(180.)).child(Label::new(loops.folder.clone())))
                    .child(
                        Label::new(format!("{} now", loops.now)).size(LabelSize::Small),
                    )
                    .child(
                        Label::new(format!("{} a week ago", loops.week_ago))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new(format!("{delta:+}"))
                            .size(LabelSize::Small)
                            .color(if delta > 0 {
                                Color::Warning
                            } else if delta < 0 {
                                Color::Success
                            } else {
                                Color::Placeholder
                            }),
                    ),
            );
        }
        block.into_any_element()
    }

    fn render_narrative(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut block = v_flex()
            .w_full()
            .child(self.section_title("Narrative", 0));
        match &self.narrative {
            Some(markdown) => {
                let style = MarkdownStyle {
                    base_text_style: window.text_style(),
                    syntax: cx.theme().syntax().clone(),
                    selection_background_color: cx.theme().colors().element_selection_background,
                    ..Default::default()
                };
                block = block.child(
                    div()
                        .px_2()
                        .child(MarkdownElement::new(markdown.clone(), style)),
                );
            }
            None => {
                block = block.child(
                    div().px_2().py_1().child(
                        Label::new(format!(
                            "No narrative yet. Whatever bot you use writes prose into the narrative \
                             block of {}; see the prompt file next to it.",
                            self.config.themes_file(signals::THEMES_NAME)
                        ))
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                    ),
                );
            }
        }
        block.into_any_element()
    }
}

impl EventEmitter<()> for ThemesView {}

impl Focusable for ThemesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for ThemesView {
    type Event = ();

    fn to_item_events(_: &Self::Event, _: &mut dyn FnMut(workspace::item::ItemEvent)) {}

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Themes".into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::BrainzTheme))
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        None
    }
}

impl Render for ThemesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::BrainzTheme).color(Color::Accent))
                    .child(Label::new("Themes").size(LabelSize::Large)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(self.render_run_button(cx))
                    .child(
                        IconButton::new("brainz-themes-refresh", IconName::ArrowCircle)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Reload from disk"))
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    ),
            );

        let mut body: Vec<AnyElement> = Vec::new();
        if let Some(error) = &self.error {
            body.push(
                v_flex()
                    .mt_3()
                    .p_4()
                    .gap_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .bg(cx.theme().colors().surface_background)
                    .child(Label::new(error.clone()))
                    .child(
                        Label::new(format!(
                            "Run now computes {} from the brain's git history.",
                            self.config.themes_file(signals::SIGNALS_NAME)
                        ))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    )
                    .into_any_element(),
            );
        }
        if let Some(signals) = self.signals.clone() {
            let pinned: Vec<String> = signals
                .themes
                .iter()
                .filter(|theme| theme.pinned)
                .map(|theme| theme.id.clone())
                .collect();
            // Two lists side by side so Rising and Fading read together
            // without scrolling, even with the conversation panel open.
            let pair = |left: AnyElement, right: AnyElement| {
                h_flex()
                    .w_full()
                    .items_start()
                    .gap_6()
                    .child(div().flex_1().min_w_0().child(left))
                    .child(div().flex_1().min_w_0().child(right))
                    .into_any_element()
            };
            body.push(pair(
                self.render_theme_list("Rising", &signals.rising, 0, window, cx),
                self.render_theme_list("Fading", &signals.fading, 2000, window, cx),
            ));
            body.push(pair(
                self.render_theme_list("New", &signals.fresh, 1000, window, cx),
                self.render_theme_list("Pinned", &pinned, 3000, window, cx),
            ));
            body.push(self.render_open_loops(&signals));
        }
        body.push(self.render_narrative(window, cx));

        v_flex()
            .id("brainz-themes")
            .key_context("BrainzThemes")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .bg(cx.theme().colors().editor_background)
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(1240.))
                    .mx_auto()
                    .pt_6()
                    .pb_10()
                    .px_4()
                    .gap_1()
                    .child(header)
                    .children(body),
            )
    }
}

/// Status bar button that opens the Themes tab.
pub struct ThemesButton {
    pane_item_focus_handle: Option<FocusHandle>,
    active: bool,
}

impl ThemesButton {
    pub fn new() -> Self {
        Self {
            pane_item_focus_handle: None,
            active: false,
        }
    }
}

impl Render for ThemesButton {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.pane_item_focus_handle.clone();
        let active = self.active;
        div().child(
            IconButton::new("brainz-themes-button", IconName::BrainzTheme)
                .icon_size(IconSize::Small)
                .toggle_state(active)
                .icon_color(if active { Color::Accent } else { Color::Default })
                .tooltip(move |_window, cx| {
                    if let Some(focus_handle) = &focus_handle {
                        Tooltip::for_action_in("Themes", &OpenThemes, focus_handle, cx)
                    } else {
                        Tooltip::for_action("Themes", &OpenThemes, cx)
                    }
                })
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(OpenThemes), cx);
                }),
        )
    }
}

impl StatusItemView for ThemesButton {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_item_focus_handle = active_pane_item.map(|item| item.item_focus_handle(cx));
        self.active =
            active_pane_item.is_some_and(|item| item.downcast::<ThemesView>().is_some());
        cx.notify();
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}
