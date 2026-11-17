//! Brainz: what the avatar offers on its own. Pure pickers for the moments
//! it speaks up (a free stretch after a meeting, the end of the day), the
//! meeting link behind Join, and the "For you" items it gathers from the
//! suggested to-dos and My Man's tasks.

use std::path::Path;

use anyhow::{Context as _, Result};
use chrono::{DateTime, Local, Timelike as _};

use crate::{
    CalendarEvent,
    brain_config::BrainConfig,
    myman::{self, MyManTask},
    open_loops, todo,
};

/// A free stretch shorter than this is not worth a nudge.
const FREE_MINUTES: i64 = 30;
/// The free-time card shows for this long after a meeting ends.
const FREE_SHOWN_MINUTES: i64 = 15;
/// The wrap-up card shows for this long after the day's last meeting.
const WRAP_SHOWN_MINUTES: i64 = 90;

#[derive(Debug, Clone, PartialEq)]
pub enum ForYouSource {
    Suggested { raw: String },
    MyMan(MyManTask),
}

/// Something the avatar thinks belongs on the board, waiting for a yes or no.
#[derive(Debug, Clone, PartialEq)]
pub struct ForYou {
    pub title: String,
    pub source: ForYouSource,
}

impl ForYou {
    pub fn key(&self) -> String {
        match &self.source {
            ForYouSource::Suggested { raw } => format!("s:{raw}"),
            ForYouSource::MyMan(task) => format!("m:{}", task.key()),
        }
    }

    pub fn label(&self) -> &'static str {
        match self.source {
            ForYouSource::Suggested { .. } => "Suggested",
            ForYouSource::MyMan(_) => "From My Man",
        }
    }
}

/// What the avatar knows about the brain, read off the main thread.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub for_you: Vec<ForYou>,
    pub top_today: Option<String>,
}

pub fn snapshot(brain: &Path) -> Snapshot {
    let config = BrainConfig::load(brain);
    let mut for_you = todo::load_suggestions(&brain.join(todo::suggestions_relative(&config.todo)))
        .into_iter()
        .map(|suggestion| ForYou {
            title: suggestion.title,
            source: ForYouSource::Suggested {
                raw: suggestion.raw,
            },
        })
        .collect::<Vec<_>>();
    if let Some(myman) = myman::MyMan::detect(&config) {
        for_you.extend(myman::pending_tasks(&myman).into_iter().map(|task| ForYou {
            title: task.title.clone(),
            source: ForYouSource::MyMan(task),
        }));
    }
    let top_today = todo::read_board_text(&brain.join(&config.todo))
        .ok()
        .and_then(|text| top_today(&todo::parse_board(&text)));
    Snapshot { for_you, top_today }
}

/// The open Today item to do first: highest priority, then board order.
pub(crate) fn top_today(board: &todo::TodoBoard) -> Option<String> {
    board
        .sections
        .iter()
        .find(|section| section.name.eq_ignore_ascii_case("today"))?
        .items
        .iter()
        .filter(|item| !item.done)
        .min_by_key(|item| {
            let rank = match item.priority {
                Some(todo::Priority::High) => 0,
                Some(todo::Priority::Medium) => 1,
                Some(todo::Priority::Low) => 2,
                None => 3,
            };
            (rank, item.line)
        })
        .map(|item| item.title.clone())
}

/// Puts a "For you" item under Today (or drops it), and takes it off its list.
pub fn resolve(brain: &Path, item: &ForYou, add: bool) -> Result<()> {
    let config = BrainConfig::load(brain);
    let board = brain.join(&config.todo);
    let add_line = |line: &str| -> Result<()> {
        let text = todo::read_board_text(&board)?;
        todo::write_board_text(&board, &open_loops::insert_board_line(&text, line, true))
    };
    match &item.source {
        ForYouSource::Suggested { raw } => {
            let _serialised = todo::BOARD_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let path = brain.join(todo::suggestions_relative(&config.todo));
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let remaining = todo::remove_suggestion(&text, raw)?;
            if add {
                add_line(&format!("- [ ] {}", item.title))?;
            }
            todo::write_board_text(&path, &remaining)
        }
        ForYouSource::MyMan(task) => {
            let _serialised = myman::TASK_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if add {
                add_line(&task.board_line())?;
            }
            myman::mark_task_handled(task)
        }
    }
}

const MEETING_HOSTS: &[&str] = &[
    "zoom.us/j/",
    "zoom.us/my/",
    "zoom.us/w/",
    "meet.google.com/",
    "teams.microsoft.com/l/meetup-join",
    "teams.live.com/meet",
    "webex.com/",
    "whereby.com/",
    "around.co/",
    "app.gather.town/",
];

/// The video-call link in an event's URL, location or notes, if it has one.
pub fn meeting_link(event: &CalendarEvent) -> Option<String> {
    [
        event.url.as_deref(),
        event.location.as_deref(),
        event.notes.as_deref(),
    ]
    .into_iter()
    .flatten()
    .flat_map(|text| {
        text.split(|c: char| {
            c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')' | '[' | ']' | '"' | '\'')
        })
    })
    .map(|token| token.trim_end_matches(['.', ',', ';', ':']))
    .find(|token| {
        token.starts_with("https://") && MEETING_HOSTS.iter().any(|host| token.contains(host))
    })
    .map(str::to_owned)
}

fn timed_today(
    events: &[CalendarEvent],
    now: DateTime<Local>,
) -> impl Iterator<Item = &CalendarEvent> {
    let today = now.date_naive();
    events
        .iter()
        .filter(move |event| !event.all_day && event.start.date_naive() == today)
}

/// Right after a meeting, when the next one is at least half an hour off:
/// the key of that free stretch and when it ends (none: the day is clear).
pub fn free_time(
    events: &[CalendarEvent],
    now: DateTime<Local>,
) -> Option<(String, Option<DateTime<Local>>)> {
    if !(8..18).contains(&now.hour()) {
        return None;
    }
    if timed_today(events, now).any(|event| event.start <= now && now < event.end) {
        return None;
    }
    let ended = timed_today(events, now)
        .filter(|event| {
            event.end <= now && now - event.end < chrono::Duration::minutes(FREE_SHOWN_MINUTES)
        })
        .max_by_key(|event| event.end)?;
    let until = timed_today(events, now)
        .filter(|event| event.start > now)
        .map(|event| event.start)
        .min();
    if until.is_some_and(|until| until - now < chrono::Duration::minutes(FREE_MINUTES)) {
        return None;
    }
    Some((format!("free@{}", ended.end.to_rfc3339()), until))
}

/// After the day's last meeting (from 3 pm), or at 5 pm on a day without
/// any: the key of today's wrap-up.
pub fn wrap_up(events: &[CalendarEvent], now: DateTime<Local>) -> Option<String> {
    if !(15..21).contains(&now.hour()) {
        return None;
    }
    if timed_today(events, now).any(|event| event.end > now) {
        return None;
    }
    let shown_from = match timed_today(events, now).map(|event| event.end).max() {
        Some(last) => last,
        None => now.with_hour(17)?.with_minute(0)?.with_second(0)?,
    };
    let since = now - shown_from;
    (since >= chrono::Duration::zero() && since < chrono::Duration::minutes(WRAP_SHOWN_MINUTES))
        .then(|| format!("wrap@{}", now.date_naive()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    fn at(hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, 11, hour, minute, 0)
            .single()
            .expect("a valid local time")
    }

    fn event(id: &str, start: DateTime<Local>, minutes: i64) -> CalendarEvent {
        CalendarEvent {
            id: id.into(),
            title: id.to_owned().into(),
            start,
            end: start + chrono::Duration::minutes(minutes),
            all_day: false,
            calendar: "Work".into(),
            attendees: 0,
            location: None,
            color: None,
            url: None,
            notes: None,
            attendee_names: Vec::new(),
            organizer: None,
            attendee_emails: Vec::new(),
        }
    }

    #[test]
    fn join_finds_the_call_link_wherever_it_is() {
        let mut call = event("call", at(9, 0), 30);
        assert_eq!(meeting_link(&call), None);
        call.notes = Some("Agenda\nJoin: <https://acme.zoom.us/j/123?pwd=x>.".into());
        assert_eq!(
            meeting_link(&call).as_deref(),
            Some("https://acme.zoom.us/j/123?pwd=x")
        );
        call.location = Some("https://meet.google.com/abc-defg-hij".into());
        assert_eq!(
            meeting_link(&call).as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
    }

    #[test]
    fn free_time_follows_a_meeting_with_room_after_it() {
        let events = vec![
            event("standup", at(9, 0), 30),
            event("lunch", at(12, 0), 60),
            event("review", at(13, 15), 15),
        ];
        assert_eq!(free_time(&events, at(8, 50)), None);
        let (key, until) = free_time(&events, at(9, 35)).expect("free after standup");
        assert!(key.starts_with("free@"));
        assert_eq!(until, Some(at(12, 0)));
        // Too long after the meeting, during one, or with the next one close.
        assert_eq!(free_time(&events, at(9, 50)), None);
        assert_eq!(free_time(&events, at(12, 30)), None);
        assert_eq!(free_time(&events, at(13, 2)), None);
        // The day is clear after the last one.
        assert_eq!(
            free_time(&events, at(13, 31)).map(|(_, until)| until),
            Some(None)
        );
    }

    #[test]
    fn wrap_up_comes_after_the_last_meeting_of_the_afternoon() {
        let events = vec![event("standup", at(9, 0), 30), event("sync", at(15, 0), 30)];
        assert_eq!(wrap_up(&events, at(15, 10)), None);
        assert!(wrap_up(&events, at(15, 40)).is_some());
        assert_eq!(wrap_up(&events, at(17, 10)), None);
        assert!(wrap_up(&[], at(17, 20)).is_some());
        assert_eq!(wrap_up(&[], at(16, 20)), None);
    }
}
