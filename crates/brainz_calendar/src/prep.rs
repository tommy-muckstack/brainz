//! Brainz: calendar-aware prep and capture. Shortly before an event that
//! matches a folder in the brain, a banner above the file tree offers the
//! prep file; shortly after it ends, the same banner offers to log the call
//! through a Claude conversation. Brainz never writes the notes itself.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Local};
use gpui::{App, AppContext as _, Entity, Global, Task, WeakEntity, Window};
use ui::{Tooltip, prelude::*};
use workspace::{OpenOptions, OpenVisible, Workspace};

use crate::{
    CalendarEvent, LoadState,
    brain_config::BrainConfig,
    brain_match::{BrainIndex, Match},
};

/// Opens a new Claude conversation in the panel with this text in the
/// message box, not sent. Handled by the agent panel.
#[derive(Clone, PartialEq, Debug, serde::Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = brainz_calendar)]
pub struct OpenClaudePrefilled {
    pub text: String,
}

const POLL_INTERVAL: Duration = Duration::from_secs(60);
/// How long after the log window opens the "Log it?" banner keeps offering.
const LOG_WINDOW: Duration = Duration::from_secs(3 * 60 * 60);
const MEETINGS_DIR: &str = "MyManBrain/meetings";
const SKILL_PATH: &str = ".claude/skills/granola-to-brain/SKILL.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BannerKind {
    Prep,
    Log,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Banner {
    pub kind: BannerKind,
    pub event_id: String,
    pub who: String,
    pub start: DateTime<Local>,
    pub folder: String,
    pub prep_file: Option<String>,
}

impl Banner {
    /// `<event id>:prep` or `<event id>:log`, what Dismiss remembers.
    fn key(&self) -> String {
        format!(
            "{}:{}",
            self.event_id,
            match self.kind {
                BannerKind::Prep => "prep",
                BannerKind::Log => "log",
            }
        )
    }
}

pub struct PrepState {
    repo: Option<PathBuf>,
    workspace: Option<WeakEntity<Workspace>>,
    banner: Option<Banner>,
    dismissed: HashSet<String>,
    _poll: Option<Task<()>>,
}

struct GlobalPrepState(Entity<PrepState>);

impl Global for GlobalPrepState {}

pub fn init(cx: &mut App) {
    let prep_state = cx.new(|_| PrepState {
        repo: None,
        workspace: None,
        banner: None,
        dismissed: HashSet::new(),
        _poll: None,
    });
    cx.set_global(GlobalPrepState(prep_state));
    // The project's roots arrive a moment after the workspace, and the
    // watchers must not depend on any particular panel being open, so
    // poll briefly for the first root and start everything from here.
    cx.observe_new(|_: &mut Workspace, _, cx| {
        let workspace = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            for _ in 0..240 {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let started = workspace
                    .update(cx, |workspace, cx| {
                        let Some(root) = workspace.root_paths(cx).first().cloned() else {
                            return false;
                        };
                        let root = root.to_path_buf();
                        let weak = cx.weak_entity();
                        if let Some(state) = state(cx) {
                            state.update(cx, |state, cx| {
                                state.ensure_watching(weak, root.clone(), cx)
                            });
                        }
                        crate::status_decay::watch(root, cx);
                        true
                    })
                    .unwrap_or(true);
                if started {
                    return;
                }
            }
        })
        .detach();
    })
    .detach();
}

pub fn state(cx: &App) -> Option<Entity<PrepState>> {
    cx.try_global::<GlobalPrepState>().map(|g| g.0.clone())
}

/// Picks the banner to show for `now`: the nearest upcoming match inside
/// the lead window, else the most recently ended match inside the log
/// window. Pure, so it is testable without a calendar.
pub fn choose_banner(
    events: &[CalendarEvent],
    index: &BrainIndex,
    config: &BrainConfig,
    now: DateTime<Local>,
    dismissed: &HashSet<String>,
) -> Option<Banner> {
    let lead = chrono::Duration::minutes(i64::from(config.calendar.prep_lead_minutes));
    let delay = chrono::Duration::minutes(i64::from(config.calendar.log_delay_minutes));
    let log_window = chrono::Duration::from_std(LOG_WINDOW).unwrap_or(chrono::Duration::hours(3));
    let mut prep: Option<Banner> = None;
    let mut log: Option<Banner> = None;
    for event in events.iter().filter(|event| !event.all_day) {
        let in_prep_window = event.start - lead <= now && now < event.end;
        let in_log_window = event.end + delay <= now && now <= event.end + delay + log_window;
        if !in_prep_window && !in_log_window {
            continue;
        }
        let Some(found) = index.match_event(
            &event.title,
            &event.attendee_names,
            &event.attendee_emails,
            event.start.date_naive(),
        ) else {
            continue;
        };
        let banner = |kind: BannerKind, found: &Match| Banner {
            kind,
            event_id: event.id.clone(),
            who: found.who.clone(),
            start: event.start,
            folder: found.folder.clone(),
            prep_file: found.prep_file.clone(),
        };
        if in_prep_window {
            let candidate = banner(BannerKind::Prep, &found);
            if !dismissed.contains(&candidate.key())
                && prep.as_ref().is_none_or(|current| event.start < current.start)
            {
                prep = Some(candidate);
            }
        } else {
            let candidate = banner(BannerKind::Log, &found);
            if !dismissed.contains(&candidate.key())
                && log.as_ref().is_none_or(|current| event.start > current.start)
            {
                log = Some(candidate);
            }
        }
    }
    prep.or(log)
}

/// Google Doc and Sheet links in a prep file, opened alongside it.
pub fn google_doc_links(text: &str) -> Vec<String> {
    let mut links = Vec::new();
    for token in text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')' | '[' | ']' | '"' | '\'' | '`')) {
        let token = token.trim_end_matches(['.', ',', ';', ':']);
        if (token.starts_with("https://docs.google.com/") || token.starts_with("http://docs.google.com/"))
            && !links.iter().any(|known| known == token)
        {
            links.push(token.to_owned());
        }
    }
    links
}

/// Transcripts My Man saved today, if the folder exists.
fn todays_meeting_files(home: &Path, today: chrono::NaiveDate) -> Vec<PathBuf> {
    let dir = home.join(MEETINGS_DIR);
    let prefix = today.format("%Y-%m-%d").to_string();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect();
    files.sort();
    files
}

/// The message the Log button pre-fills. The agent does the rest.
pub fn log_call_prompt(banner: &Banner, home: &Path, today: chrono::NaiveDate) -> String {
    let mut text = format!(
        "Log the call with {} that just ended. Follow `{SKILL_PATH}`.\n\nFolder: `{}`.\n",
        banner.who, banner.folder
    );
    if let Some(prep) = &banner.prep_file {
        text.push_str(&format!("Prep file: `{prep}`.\n"));
    }
    let meetings = todays_meeting_files(home, today);
    let meetings_dir = home.join(MEETINGS_DIR);
    if meetings.is_empty() {
        text.push_str(&format!(
            "Check `{}` for a transcript dated {} (none was there when this message was written).\n",
            meetings_dir.display(),
            today.format("%Y-%m-%d")
        ));
    } else {
        text.push_str(&format!(
            "Local transcripts from today in `{}`: {}.\n",
            meetings_dir.display(),
            meetings
                .iter()
                .filter_map(|path| path.file_name().and_then(|n| n.to_str()))
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    text.push_str("\nAsk me for my read before you write it up. House rules: no em dashes, absolute dates, headings for structure only.");
    text
}

impl PrepState {
    fn ensure_watching(
        &mut self,
        workspace: WeakEntity<Workspace>,
        repo: PathBuf,
        cx: &mut Context<Self>,
    ) {
        if self.repo.as_deref() == Some(repo.as_path()) {
            return;
        }
        self.repo = Some(repo.clone());
        self.workspace = Some(workspace.clone());
        self.banner = None;
        crate::memory_share::ensure_once(repo.clone(), workspace, cx);
        self._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let repo = repo.clone();
                let dismissed = this
                    .read_with(cx, |this, _| this.dismissed.clone())
                    .unwrap_or_default();
                let banner = cx
                    .background_spawn(async move {
                        let config = BrainConfig::load(&repo);
                        let (state, events) = match crate::load_events() {
                            Ok(result) => result,
                            Err(error) => {
                                log::debug!("brainz prep: {error:#}");
                                return None;
                            }
                        };
                        if state != LoadState::Ready {
                            log::warn!("brainz prep: calendar not readable: {state:?}");
                            return None;
                        }
                        let index = BrainIndex::load(&repo, &config);
                        choose_banner(&events, &index, &config, Local::now(), &dismissed)
                    })
                    .await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        if this.banner != banner {
                            match &banner {
                                Some(banner) => log::info!(
                                    "brainz prep: {:?} banner for {} ({}), prep {:?}",
                                    banner.kind, banner.who, banner.folder, banner.prep_file
                                ),
                                None => log::info!("brainz prep: banner cleared"),
                            }
                            this.banner = banner;
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !keep_going {
                    break;
                }
                cx.background_executor().timer(POLL_INTERVAL).await;
            }
        }));
    }

    pub fn banner(&self) -> Option<&Banner> {
        self.banner.as_ref()
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if let Some(banner) = self.banner.take() {
            self.dismissed.insert(banner.key());
            cx.notify();
        }
    }

    fn open_prep(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(repo), Some(banner), Some(workspace)) =
            (self.repo.clone(), self.banner.clone(), self.workspace.clone())
        else {
            return;
        };
        let Some(prep) = banner.prep_file else {
            return;
        };
        let path = repo.join(&prep);
        for link in google_doc_links(&std::fs::read_to_string(&path).unwrap_or_default()) {
            cx.open_url(&link);
        }
        open_in_workspace(&workspace, path, window, cx);
    }

    fn open_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(repo), Some(banner), Some(workspace)) =
            (self.repo.clone(), self.banner.clone(), self.workspace.clone())
        else {
            return;
        };
        let folder = repo.join(&banner.folder);
        let target = ["CLAUDE.md", "README.md"]
            .iter()
            .map(|name| folder.join(name))
            .find(|path| path.is_file())
            .unwrap_or(folder);
        open_in_workspace(&workspace, target, window, cx);
    }

    fn log_call(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(banner) = self.banner.clone() else {
            return;
        };
        let text = log_call_prompt(
            &banner,
            &util::paths::home_dir(),
            Local::now().date_naive(),
        );
        window.dispatch_action(Box::new(OpenClaudePrefilled { text }), cx);
        self.dismiss(cx);
    }
}

fn open_in_workspace(
    workspace: &WeakEntity<Workspace>,
    path: PathBuf,
    window: &mut Window,
    cx: &mut App,
) {
    workspace
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

fn time_label(start: &DateTime<Local>) -> String {
    use chrono::Timelike;
    let (_, hour) = start.hour12();
    let suffix = if start.hour12().0 { "pm" } else { "am" };
    if start.minute() == 0 {
        format!("{hour}{suffix}")
    } else {
        format!("{hour}:{:02}{suffix}", start.minute())
    }
}

/// The banner the project panel shows above the file tree, under the Sync
/// banner when both are up.
pub fn render_banner(workspace: &WeakEntity<Workspace>, cx: &mut App) -> Option<gpui::AnyElement> {
    let state = state(cx)?;
    let repo = workspace
        .upgrade()?
        .read(cx)
        .root_paths(cx)
        .first()
        .map(|path| path.to_path_buf())?;
    state.update(cx, |state, cx| state.ensure_watching(workspace.clone(), repo, cx));
    let banner = state.read(cx).banner()?.clone();

    let colors = cx.theme().colors();
    let amber = colors.text_accent;
    let ink = gpui::hsla(0., 0., 0.08, 1.);
    let (text, icon) = match banner.kind {
        BannerKind::Prep => (
            format!("{}, {}.", banner.who, time_label(&banner.start)),
            IconName::BrainzCalendar,
        ),
        BannerKind::Log => (format!("Call with {} ended. Log it?", banner.who), IconName::BrainzClaude),
    };
    let has_prep = banner.prep_file.is_some();

    let mut buttons = h_flex().gap_1p5().w_full();
    match banner.kind {
        BannerKind::Prep => {
            if has_prep {
                buttons = buttons.child(
                    Button::new("brainz-prep-open", "Open prep")
                        .style(ButtonStyle::Filled)
                        .tooltip(Tooltip::text("Open the prep note and any Google Doc it links"))
                        .on_click({
                            let state = state.clone();
                            move |_, window, cx| {
                                state.update(cx, |state, cx| state.open_prep(window, cx));
                            }
                        }),
                );
            }
            buttons = buttons.child(
                Button::new("brainz-prep-folder", "Open folder")
                    .style(if has_prep { ButtonStyle::Subtle } else { ButtonStyle::Filled })
                    .tooltip(Tooltip::text(banner.folder))
                    .on_click({
                        let state = state.clone();
                        move |_, window, cx| {
                            state.update(cx, |state, cx| state.open_folder(window, cx));
                        }
                    }),
            );
        }
        BannerKind::Log => {
            buttons = buttons.child(
                Button::new("brainz-prep-log", "Log this call")
                    .style(ButtonStyle::Filled)
                    .tooltip(Tooltip::text(
                        "Open a Claude conversation pre-filled with the folder, prep, and transcripts",
                    ))
                    .on_click({
                        let state = state.clone();
                        move |_, window, cx| {
                            state.update(cx, |state, cx| state.log_call(window, cx));
                        }
                    }),
            );
        }
    }
    buttons = buttons.child(div().flex_1()).child(
        IconButton::new("brainz-prep-dismiss", IconName::Close)
            .icon_size(IconSize::XSmall)
            .icon_color(Color::Custom(ink))
            .tooltip(Tooltip::text("Dismiss"))
            .on_click({
                let state = state.clone();
                move |_, _, cx| {
                    state.update(cx, |state, cx| state.dismiss(cx));
                }
            }),
    );

    Some(
        v_flex()
            .id("brainz-prep-banner")
            .w_full()
            .px_2()
            .pb_2()
            .child(
                v_flex()
                    .w_full()
                    .px_2()
                    .py_1p5()
                    .gap_1p5()
                    .rounded_lg()
                    .bg(amber)
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(Icon::new(icon).size(IconSize::Small).color(Color::Custom(ink)))
                            .child(
                                Label::new(text)
                                    .size(LabelSize::Small)
                                    .color(Color::Custom(ink))
                                    .truncate(),
                            ),
                    )
                    .child(buttons),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain_match::BrainIndex;
    use chrono::TimeZone;

    fn event(id: &str, title: &str, start: DateTime<Local>, minutes: i64) -> CalendarEvent {
        CalendarEvent {
            id: id.into(),
            title: title.into(),
            start,
            end: start + chrono::Duration::minutes(minutes),
            all_day: false,
            calendar: "Work".into(),
            attendees: 0,
            location: None,
            color: None,
            url: None,
            notes: None,
            attendee_names: vec![],
            organizer: None,
            attendee_emails: vec![],
        }
    }

    fn brain() -> (tempfile::TempDir, BrainIndex, BrainConfig) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("companies/acme/2026-10-01")).unwrap();
        std::fs::write(
            root.join("companies/acme/CLAUDE.md"),
            "# Acme\n> **Status 2026-09-30:** loop live. Roster: Hank Scorpio (CEO).\n",
        )
        .unwrap();
        std::fs::write(
            root.join("companies/acme/2026-10-01/scorpio-reconnect-prep.md"),
            "# Prep\nLoop doc: https://docs.google.com/document/d/abc/edit?usp=sharing.\n",
        )
        .unwrap();
        let config = BrainConfig {
            vocabulary_folders: vec!["companies".into()],
            ..BrainConfig::default()
        };
        let index = BrainIndex::load(root, &config);
        (dir, index, config)
    }

    #[test]
    fn prep_banner_ten_minutes_out_then_log_banner_after_the_call() {
        let (_dir, index, config) = brain();
        let start = Local.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap();
        let events = vec![
            event("a", "Hank Scorpio", start, 45),
            event("b", "Dentist", start, 45),
        ];
        let none = HashSet::new();
        let early = start - chrono::Duration::minutes(11);
        assert!(choose_banner(&events, &index, &config, early, &none).is_none());
        let banner = choose_banner(
            &events,
            &index,
            &config,
            start - chrono::Duration::minutes(10),
            &none,
        )
        .unwrap();
        assert_eq!(banner.kind, BannerKind::Prep);
        assert_eq!(banner.who, "Hank Scorpio");
        assert_eq!(
            banner.prep_file.as_deref(),
            Some("companies/acme/2026-10-01/scorpio-reconnect-prep.md")
        );
        // Still prep during the call, nothing in the gap, then log.
        let during = start + chrono::Duration::minutes(20);
        assert_eq!(
            choose_banner(&events, &index, &config, during, &none).unwrap().kind,
            BannerKind::Prep
        );
        let just_ended = start + chrono::Duration::minutes(47);
        assert!(choose_banner(&events, &index, &config, just_ended, &none).is_none());
        let later = start + chrono::Duration::minutes(50);
        let banner = choose_banner(&events, &index, &config, later, &none).unwrap();
        assert_eq!(banner.kind, BannerKind::Log);
        let mut dismissed = HashSet::new();
        dismissed.insert(banner.key());
        assert!(choose_banner(&events, &index, &config, later, &dismissed).is_none());
    }

    #[test]
    fn log_prompt_names_skill_folder_prep_and_transcripts() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(MEETINGS_DIR)).unwrap();
        std::fs::write(home.path().join(MEETINGS_DIR).join("2026-10-01-ABC.md"), "").unwrap();
        std::fs::write(home.path().join(MEETINGS_DIR).join("2026-09-30-OLD.md"), "").unwrap();
        let banner = Banner {
            kind: BannerKind::Log,
            event_id: "a".into(),
            who: "Hank Scorpio".into(),
            start: Local::now(),
            folder: "companies/acme".into(),
            prep_file: Some("companies/acme/2026-10-01/prep.md".into()),
        };
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let text = log_call_prompt(&banner, home.path(), today);
        assert!(text.contains(SKILL_PATH));
        assert!(text.contains("`companies/acme`"));
        assert!(text.contains("companies/acme/2026-10-01/prep.md"));
        assert!(text.contains("2026-10-01-ABC.md"));
        assert!(!text.contains("2026-09-30-OLD.md"));
        assert!(!text.contains('—'));
    }

    #[test]
    fn google_links_are_found_once() {
        let links = google_doc_links(
            "Doc: https://docs.google.com/document/d/abc/edit?usp=sharing. Again (https://docs.google.com/document/d/abc/edit?usp=sharing) and https://example.com",
        );
        assert_eq!(links, vec!["https://docs.google.com/document/d/abc/edit?usp=sharing".to_owned()]);
    }
}
