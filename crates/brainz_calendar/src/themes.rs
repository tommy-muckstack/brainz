//! Brainz: the Themes tab. Rising and fading themes over the brain's git
//! history, from `ops/themes/signals.json` (written by the signals pass in
//! `themes_signals`). Threads, open loops, and the narrative stay in the
//! files for the bots; the tab shows only the two lists and any pins.

use std::{
    collections::HashSet,
    path::PathBuf,
    time::{Duration, SystemTime},
};

use editor::Editor;
use gpui::{
    App, Entity, EventEmitter, FocusHandle, Focusable, Global, PathBuilder, Subscription, Task,
    WeakEntity, Window, actions, canvas, point,
};
use ui::{ContextMenu, Disclosure, PopoverMenu, Tooltip, prelude::*};
use workspace::{Item, OpenOptions, OpenVisible, Workspace};

use crate::{
    brain_config::BrainConfig,
    themes_signals::{self as signals, Signals, Theme, ThemeCategory, ThemeFilters},
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
const MAX_RELOAD_ATTEMPTS: u32 = 2;
const DAILY_CHECK: Duration = Duration::from_secs(30 * 60);
const DAILY_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const PICKER_LIMIT: usize = 30;

fn trend_line(series: &[u32]) -> impl IntoElement {
    trend_line_sized(series, px(112.))
}

fn trend_line_sized(series: &[u32], width: gpui::Pixels) -> impl IntoElement {
    let series = series.to_vec();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let maximum = series.iter().copied().max().unwrap_or(0).max(1) as f32;
            let count = series.len().max(2);
            let width = (bounds.size.width - px(4.)).max(px(0.));
            let height = (bounds.size.height - px(4.)).max(px(0.));
            let mut path = PathBuilder::stroke(px(1.5));
            for index in 0..count {
                let value = series
                    .get(index)
                    .or_else(|| series.last())
                    .copied()
                    .unwrap_or(0);
                let position = point(
                    bounds.left() + px(2.) + width * (index as f32 / (count - 1) as f32),
                    bounds.bottom() - px(2.) - height * (value as f32 / maximum),
                );
                if index == 0 {
                    path.move_to(position);
                } else {
                    path.line_to(position);
                }
            }
            match path.build() {
                Ok(path) => window.paint_path(path, cx.theme().colors().text_accent),
                Err(error) => log::error!("Could not draw theme trend line: {error}"),
            }
        },
    )
    .w(width)
    .h(px(24.))
    .flex_shrink_0()
}

fn load_filters() -> ThemeFilters {
    let path = paths::config_dir().join("themes-filters.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(filters) => filters,
            Err(error) => {
                log::error!("invalid theme filters: {error}");
                ThemeFilters::default()
            }
        },
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::error!("reading theme filters: {error}");
            }
            ThemeFilters::default()
        }
    }
}

fn save_filters(filters: ThemeFilters) -> anyhow::Result<()> {
    std::fs::create_dir_all(paths::config_dir())?;
    std::fs::write(
        paths::config_dir().join("themes-filters.json"),
        serde_json::to_vec_pretty(&filters)?,
    )?;
    Ok(())
}

pub fn init(cx: &mut App) {
    let global_runner = cx.new(ThemesRunner::new);
    cx.set_global(GlobalRunner(global_runner));
    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        workspace.register_action(|workspace, _: &OpenThemes, window, cx| {
            ThemesView::open(workspace, window, cx);
        });
        // Brainz: Themes is the landing tab. The project's roots arrive a
        // moment after the workspace, so poll briefly; with no folder after
        // two seconds the tab opens as a landing card and attaches itself
        // once a folder shows up.
        if let Some(window) = window {
            cx.spawn_in(window, async move |workspace, cx| {
                for tick in 0..240 {
                    cx.background_executor()
                        .timer(Duration::from_millis(250))
                        .await;
                    let done = workspace.update_in(cx, |workspace, window, cx| {
                        let root = workspace.root_paths(cx).first().map(|p| p.to_path_buf());
                        let existing = workspace.items_of_type::<ThemesView>(cx).next();
                        match (root, existing) {
                            (Some(root), Some(view)) => {
                                view.update(cx, |view, cx| view.attach(root, cx));
                                true
                            }
                            (Some(_), None) => {
                                ThemesView::open(workspace, window, cx);
                                true
                            }
                            (None, None) if tick >= 8 => {
                                ThemesView::open(workspace, window, cx);
                                false
                            }
                            (None, _) => false,
                        }
                    });
                    if done.unwrap_or(true) {
                        return;
                    }
                }
            })
            .detach();
        }
        workspace.register_action(|workspace, _: &RunThemesPass, _window, cx| {
            let Some(root) = workspace
                .root_paths(cx)
                .first()
                .map(|path| path.to_path_buf())
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
    cx.try_global::<GlobalRunner>()
        .map(|global| global.0.clone())
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
        let Some(repo) =
            crate::github_sync::state(cx).and_then(|state| state.read(cx).repo().cloned())
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
                .background_spawn({
                    let repo = repo.clone();
                    async move { signals::run_pass(&repo) }
                })
                .await;
            let succeeded = result.is_ok();
            this.update(cx, |this, cx| {
                this.running = false;
                if let Err(error) = result {
                    log::error!("themes pass failed: {error:#}");
                    this.last_error = Some(format!("{error:#}"));
                }
                cx.notify();
            })
            .ok();
            if succeeded {
                // The brain's proper nouns tune My Man's dictation; a
                // failure here is My Man's problem, not the pass's.
                cx.background_spawn(async move { crate::myman::feed_vocabulary(&repo) })
                    .await;
            }
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
    has_repo: bool,
    config: BrainConfig,
    signals: Option<Signals>,
    filters: ThemeFilters,
    error: Option<String>,
    /// Passes re-run after a failed load before the error is shown.
    reload_attempts: u32,
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
        // No folder open yet: the tab still opens, as a landing card that
        // offers to open one.
        let root = workspace
            .root_paths(cx)
            .first()
            .map(|path| path.to_path_buf());
        let weak = cx.entity().downgrade();
        let view = cx.new(|cx| ThemesView::new(weak, root, cx));
        workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
    }

    fn new(
        workspace: WeakEntity<Workspace>,
        repo: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        let has_repo = repo.is_some();
        let repo = repo.unwrap_or_default();
        let runner_observation = runner(cx).map(|runner| {
            cx.observe(&runner, |this, runner, cx| {
                if !runner.read(cx).is_running() {
                    this.error = runner.read(cx).last_error.clone();
                    if this.error.is_none() {
                        this.reload(cx);
                    }
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
            has_repo,
            config,
            signals: None,
            filters: load_filters(),
            error: None,
            reload_attempts: 0,
            expanded: HashSet::new(),
            renaming: None,
            merging: None,
            _runner_observation: runner_observation,
            _rename_subscription: None,
            _disk_poll: disk_poll,
            _load: None,
        };
        if this.has_repo {
            this.reload(cx);
        }
        this
    }

    /// Points a landing-card view at a folder that was opened after it.
    fn attach(&mut self, repo: PathBuf, cx: &mut Context<Self>) {
        if self.has_repo {
            return;
        }
        self.config = BrainConfig::load(&repo);
        self.themes_md_modified = signals::themes_md_modified(&repo, &self.config);
        self.repo = repo;
        self.has_repo = true;
        self.reload(cx);
        cx.notify();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if !self.has_repo {
            return;
        }
        let repo = self.repo.clone();
        let config = self.config.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { signals::load_signals(&repo, &config) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(signals) => {
                        // A file from an older Brainz lacks the newer fields;
                        // refresh it once rather than show stale shapes.
                        if signals.schema < signals::SCHEMA {
                            this.run_now(cx);
                        }
                        this.signals = Some(signals);
                        this.error = None;
                        this.reload_attempts = 0;
                    }
                    Err(error) => {
                        // A file another Brainz wrote, or a half-written one,
                        // is cured by re-running the pass; only a repeat
                        // failure is worth showing.
                        if this.signals.is_none() {
                            if this.reload_attempts < MAX_RELOAD_ATTEMPTS {
                                this.reload_attempts += 1;
                                log::warn!(
                                    "signals reload failed (attempt {}): {error:#}",
                                    this.reload_attempts
                                );
                                this.run_now(cx);
                            } else {
                                this.error = Some(format!("{error:#}"));
                            }
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn run_now(&mut self, cx: &mut Context<Self>) {
        if !self.has_repo {
            return;
        }
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
                    cx.background_spawn(
                        async move { signals::remove_pin_line(&repo, &config, &line) },
                    )
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

    fn render_filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.weak_entity();
        let filters = self.filters;
        PopoverMenu::new("brainz-themes-filters")
            .trigger_with_tooltip(
                IconButton::new("brainz-themes-filter-toggle", IconName::BrainzSliders)
                    .icon_size(IconSize::Small)
                    .aria_label("Filter themes")
                    .icon_color(if filters == ThemeFilters::default() {
                        Color::Muted
                    } else {
                        Color::Accent
                    }),
                Tooltip::text("Filter themes"),
            )
            .anchor(gpui::Anchor::TopRight)
            .menu(move |window, cx| {
                let view = view.clone();
                Some(ContextMenu::build(window, cx, move |menu, _, _| {
                    let mut menu = menu.header("Show in Themes");
                    for (label, category) in [
                        ("People", ThemeCategory::People),
                        ("Places", ThemeCategory::Places),
                        ("Things", ThemeCategory::Things),
                    ] {
                        let view = view.clone();
                        menu = menu.toggleable_entry(
                            label,
                            filters.includes(category),
                            IconPosition::Start,
                            None,
                            move |_, cx| {
                                if let Some(view) = view.upgrade() {
                                    view.update(cx, |view, cx| {
                                        view.filters.toggle(category);
                                        if let Err(error) = save_filters(view.filters) {
                                            view.error = Some(format!(
                                                "Could not save theme filters: {error:#}"
                                            ));
                                        }
                                        cx.notify();
                                    });
                                }
                            },
                        );
                    }
                    menu
                }))
            })
    }

    fn render_sync_button(&self, cx: &mut Context<Self>) -> AnyElement {
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
            IconButton::new("brainz-themes-refresh", IconName::ArrowCircle)
                .icon_size(IconSize::Small)
                .aria_label("Sync themes")
                .tooltip(Tooltip::text("Sync themes from the brain's git history"))
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

        let name: AnyElement =
            match renaming_editor {
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
            .child(
                div()
                    .min_w(px(110.))
                    .max_w(px(180.))
                    .overflow_hidden()
                    .child(name),
            )
            .child(trend_line(&theme.series))
            .child(
                Label::new(momentum)
                    .size(LabelSize::Small)
                    .color(momentum_color),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Label::new(folders)
                        .size(LabelSize::Small)
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
                                .entry(
                                    "Hide",
                                    None,
                                    move |_, cx| {
                                        hide_view
                                            .update(cx, |view, cx| {
                                                view.curate(
                                                    format!("- hide: {}", hide_theme.id),
                                                    cx,
                                                )
                                            })
                                            .ok();
                                    },
                                )
                            }))
                        }
                    }),
            );

        let mut row = v_flex().w_full().child(header);

        if self.merging.as_deref() == Some(id.as_str()) {
            let mut picker = h_flex().flex_wrap().gap_1().pl_8().py_1().child(
                Label::new("Merge into:")
                    .size(LabelSize::Small)
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
                    Button::new(
                        ("brainz-merge-target", ix * 1000 + candidate_ix),
                        candidate_name,
                    )
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.curate(format!("- merge: {source} => {target}"), cx);
                    })),
                );
            }
            picker = picker.child(
                Button::new(("brainz-merge-cancel", ix), "Cancel")
                    .label_size(LabelSize::Small)
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
            if theme.months.len() >= 2 {
                let months = self
                    .signals
                    .as_ref()
                    .map(|signals| signals.months.clone())
                    .unwrap_or_default();
                let span = match (months.first(), months.last()) {
                    (Some(first), Some(last)) => format!("All time, by month ({first} to {last})"),
                    _ => "All time, by month".to_owned(),
                };
                details = details.child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(trend_line_sized(&theme.months, px(240.)))
                        .child(Label::new(span).size(LabelSize::Small).color(Color::Muted)),
                );
            }
            let mut files = h_flex().flex_wrap().gap_1().child(
                Label::new("Files")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
            for (file_ix, file) in theme.files.iter().enumerate() {
                let path = file.name.clone();
                let label = signals::file_label(&file.name);
                files = files.child(
                    Button::new(("brainz-theme-file", ix * 100 + file_ix), label)
                        .label_size(LabelSize::Small)
                        .start_icon(
                            Icon::new(IconName::File)
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
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
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                );
                for (person_ix, person) in theme.people.iter().enumerate() {
                    let name = person.name.clone();
                    people = people.child(
                        Button::new(
                            ("brainz-theme-person", ix * 100 + person_ix),
                            person.name.clone(),
                        )
                        .label_size(LabelSize::Small)
                        .tooltip(Tooltip::text(format!(
                            "Co-mentioned {} times",
                            person.weight
                        )))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_person(&name, window, cx);
                            },
                        )),
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
                    .weight(gpui::FontWeight::SEMIBOLD),
            )
            .child(
                Label::new(count.to_string())
                    .size(LabelSize::Small)
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
            .filter(|theme| !theme.hidden && self.filters.includes(theme.category))
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
                    .child(Icon::new(IconName::BrainzTheme).color(Color::Muted))
                    .child(Label::new("Themes").size(LabelSize::Large)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(self.render_filters(cx))
                    .when(self.has_repo, |this| {
                        this.child(self.render_sync_button(cx))
                    }),
            );

        let mut body: Vec<AnyElement> = Vec::new();
        if !self.has_repo {
            body.push(
                v_flex()
                    .mt_3()
                    .p_5()
                    .gap_3()
                    .items_start()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .bg(cx.theme().colors().surface_background)
                    .child(Label::new("No brain open yet").size(LabelSize::Large))
                    .child(
                        Label::new(
                            "Brainz works on a folder of Markdown notes, ideally a git repo. \
                             Open one and the file tree, To-Do, and Themes tabs fill in from it.",
                        )
                        .color(Color::Muted),
                    )
                    .child(
                        Button::new("brainz-open-folder", "Open a folder…")
                            .style(ButtonStyle::Filled)
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(workspace::Open::default()), cx);
                            }),
                    )
                    .child(
                        Label::new(
                            "Optional: a brainz.toml at the folder's root sets where the to-do \
                             board, people files, and themes live.",
                        )
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                    )
                    .into_any_element(),
            );
        }
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
                            "Sync computes {} from the brain's git history.",
                            self.config.themes_file(signals::SIGNALS_NAME)
                        ))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    )
                    .into_any_element(),
            );
        }
        if let Some(signals) = self.signals.clone() {
            let today = chrono::Local::now().date_naive();
            let loops =
                signals::open_loops_summary(&signals.open_loops, &self.config.owner_label(), today);
            body.push(
                h_flex()
                    .px_1()
                    .pt_1()
                    .gap_2()
                    .child(
                        Label::new(format!(
                            "{} weeks · {} themes · open loops: {loops}",
                            signals.weeks.len(),
                            signals.themes.iter().filter(|theme| !theme.hidden).count(),
                        ))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    )
                    .child(
                        Button::new("brainz-themes-open-loops", "Flagged in notes")
                            .label_size(LabelSize::Small)
                            .tooltip(Tooltip::text("Open the To-Do tab, where flagged lines can be moved onto the board"))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(crate::todo::OpenTodo), cx);
                            }),
                    )
                    .into_any_element(),
            );
            let pinned: Vec<String> = signals
                .themes
                .iter()
                .filter(|theme| {
                    theme.pinned && !theme.hidden && self.filters.includes(theme.category)
                })
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
            let (rising, _, fading) = signals::ranked_themes(&signals.themes, self.filters);
            body.push(pair(
                self.render_theme_list("Rising", &rising, 0, window, cx),
                self.render_theme_list("Fading", &fading, 2000, window, cx),
            ));
            if !pinned.is_empty() {
                body.push(self.render_theme_list("Pinned", &pinned, 3000, window, cx));
            }
        }

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
