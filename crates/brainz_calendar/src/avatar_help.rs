//! Brainz: what the avatar offers on its own. Pure pickers for the moments
//! it speaks up (a free stretch after a meeting, the end of the day), the
//! meeting's call link, and the "Right now" steps: what to do next given the
//! schedule, the Today list and what the notes say is owed.

use std::path::Path;

use chrono::{DateTime, Local, Timelike as _};

use crate::{
    CalendarEvent,
    brain_config::BrainConfig,
    themes_signals::{self, OpenLoop},
    todo,
};

/// A free stretch shorter than this is not worth a nudge.
const FREE_MINUTES: i64 = 30;
/// The free-time card shows for this long after a meeting ends.
const FREE_SHOWN_MINUTES: i64 = 15;
/// The wrap-up card shows for this long after the day's last meeting.
const WRAP_SHOWN_MINUTES: i64 = 90;

/// Prep shows for a meeting starting within this long.
const PREP_AHEAD_MINUTES: i64 = 180;
/// A prep this close goes first.
const PREP_SOON_MINUTES: i64 = 90;
/// A stretch this long is "a big chunk": a to-do goes ahead of follow-ups.
const BIG_CHUNK_MINUTES: i64 = 45;
/// At most this many owed follow-ups are suggested at once.
const FOLLOW_UP_LIMIT: usize = 2;
/// At most this many next steps show in the menu.
pub const NEXT_STEP_LIMIT: usize = 3;

/// What a next step's button does.
#[derive(Debug, Clone, PartialEq)]
pub enum StepAction {
    /// Opens the meeting's prep note when Brainz has one for it; otherwise
    /// asks Claude to prep with `prompt`.
    Prep {
        event_id: String,
        prompt: String,
    },
    /// Opens Claude with `prompt` ready to send.
    Claude {
        prompt: String,
    },
    OpenTodo,
}

/// One thing worth doing now, picked from the schedule, the Today list and
/// what the notes say is owed.
#[derive(Debug, Clone, PartialEq)]
pub struct NextStep {
    pub key: String,
    pub title: String,
    /// Why now: the time it fits in, or who it is for.
    pub why: String,
    pub button: &'static str,
    pub action: StepAction,
}

/// An open item from the board's Today section.
#[derive(Debug, Clone, PartialEq)]
pub struct TodayItem {
    pub title: String,
    pub high: bool,
}

/// What the avatar knows about the brain, read off the main thread.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub top_today: Option<String>,
    /// Open Today items, the one to do first first.
    pub today: Vec<TodayItem>,
    /// ⏰ lines: what the notes say the owner still owes, oldest first.
    pub owed: Vec<OpenLoop>,
}

pub fn snapshot(brain: &Path) -> Snapshot {
    let config = BrainConfig::load(brain);
    let today = todo::read_board_text(&brain.join(&config.todo))
        .ok()
        .map(|text| today_items(&todo::parse_board(&text)))
        .unwrap_or_default();
    // Read straight from the last signals pass; it is refreshed elsewhere.
    let owed = std::fs::read_to_string(config.themes_path(brain, themes_signals::SIGNALS_NAME))
        .ok()
        .and_then(|text| serde_json::from_str::<themes_signals::Signals>(&text).ok())
        .map(|signals| {
            signals
                .open_loops
                .into_iter()
                .filter(OpenLoop::owed_by_owner)
                .collect()
        })
        .unwrap_or_default();
    Snapshot {
        top_today: today.first().map(|item| item.title.clone()),
        today,
        owed,
    }
}

/// The open Today items in the order to do them: highest priority, then
/// board order.
pub(crate) fn today_items(board: &todo::TodoBoard) -> Vec<TodayItem> {
    let Some(section) = board
        .sections
        .iter()
        .find(|section| section.name.eq_ignore_ascii_case("today"))
    else {
        return Vec::new();
    };
    let mut items = section
        .items
        .iter()
        .filter(|item| !item.done)
        .collect::<Vec<_>>();
    items.sort_by_key(|item| {
        let rank = match item.priority {
            Some(todo::Priority::High) => 0,
            Some(todo::Priority::Medium) => 1,
            Some(todo::Priority::Low) => 2,
            None => 3,
        };
        (rank, item.line)
    });
    items
        .into_iter()
        .map(|item| TodayItem {
            title: item.title.clone(),
            high: item.priority == Some(todo::Priority::High),
        })
        .collect()
}

/// "45 min", "1h", "1h 20m".
fn span(minutes: i64) -> String {
    let minutes = minutes.max(1);
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}

fn first_names(event: &CalendarEvent) -> Vec<String> {
    event
        .attendee_names
        .iter()
        .filter_map(|name| name.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

fn loop_text(open_loop: &OpenLoop) -> String {
    markdown::markdown_to_plain_text(&open_loop.text)
        .replace(themes_signals::LOOP_MARKERS, "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_start_matches(['-', '*', ' '])
        .trim_end_matches(['.', ' '])
        .to_owned()
}

/// What to do now, from the schedule, the Today list and what is owed:
/// prep for a meeting coming up, follow up with someone (the people in the
/// next meeting first), or get a to-do done in the time before the next one.
pub fn next_steps(
    events: &[CalendarEvent],
    snapshot: &Snapshot,
    now: DateTime<Local>,
    dismissed: &std::collections::HashSet<String>,
) -> Vec<NextStep> {
    let current = timed_today(events, now).find(|event| event.start <= now && now < event.end);
    let next = timed_today(events, now)
        .filter(|event| event.start > now)
        .min_by_key(|event| event.start);
    let free_from = current.map_or(now, |event| event.end);

    let prep = next
        .filter(|event| {
            (event.start - now).num_minutes() <= PREP_AHEAD_MINUTES
                && (event.attendees > 1 || !event.attendee_names.is_empty())
        })
        .map(|event| {
            let names = first_names(event);
            let with = match names.as_slice() {
                [] => String::new(),
                [one] => format!(" with {one}"),
                [one, two] => format!(" with {one} and {two}"),
                [one, two, rest @ ..] => format!(" with {one}, {two} +{}", rest.len()),
            };
            let time = event.start.format("%-I:%M %p");
            NextStep {
                key: format!("prep:{}", event.id),
                title: format!("Prep for {}", event.title),
                why: format!(
                    "{time}, in {}{with}",
                    span((event.start - now).num_minutes())
                ),
                button: "Prep",
                action: StepAction::Prep {
                    event_id: event.id.clone(),
                    prompt: format!(
                        "Help me prep for \"{}\" at {time}{with}. Check my notes for what we last discussed, anything I owe them or they owe me, and give me the three things to raise.",
                        event.title
                    ),
                },
            }
        });
    let prep_soon = prep.as_ref().is_some_and(|_| {
        next.is_some_and(|event| (event.start - now).num_minutes() <= PREP_SOON_MINUTES)
    });

    let meeting_names = current
        .into_iter()
        .chain(next)
        .flat_map(first_names)
        .map(|name| name.to_lowercase())
        .collect::<Vec<_>>();
    let seeing = |open_loop: &OpenLoop| {
        open_loop.counterparty.as_ref().is_some_and(|person| {
            let first = person.split_whitespace().next().unwrap_or(person);
            meeting_names.contains(&first.to_lowercase())
        })
    };
    let mut owed = snapshot.owed.iter().collect::<Vec<_>>();
    // People you are about to see first, then the oldest.
    owed.sort_by_key(|open_loop| (!seeing(open_loop), open_loop.first_seen.clone()));
    let today = now.date_naive();
    let follow_ups = owed
        .into_iter()
        .filter(|open_loop| !dismissed.contains(&format!("loop:{}:{}", open_loop.file, open_loop.text)))
        .take(FOLLOW_UP_LIMIT)
        .map(|open_loop| {
            let text = loop_text(open_loop);
            let age = open_loop.age_days(today);
            let note = open_loop
                .file
                .rsplit('/')
                .next()
                .unwrap_or(&open_loop.file)
                .trim_end_matches(".md")
                .to_owned();
            let when = if seeing(open_loop) {
                "you see them today".to_owned()
            } else if age == 0 {
                "from today".to_owned()
            } else {
                format!("{age}d old")
            };
            let (title, prompt) = match &open_loop.counterparty {
                Some(person) => (
                    format!("Message {person}"),
                    format!(
                        "Draft a short Slack message to {person} to close this out: \"{text}\". The note \"{}\" has the context, read it first. Keep it casual and under 80 words.",
                        open_loop.file
                    ),
                ),
                None => (
                    text.clone(),
                    format!(
                        "Help me finish this today: \"{text}\". The note \"{}\" has the context, read it first, then give me the fastest way to close it out.",
                        open_loop.file
                    ),
                ),
            };
            let why = match &open_loop.counterparty {
                Some(_) => format!("{text} · {when} · {note}"),
                None => format!("You owe this · {when} · {note}"),
            };
            NextStep {
                key: format!("loop:{}:{}", open_loop.file, open_loop.text),
                title,
                why,
                button: "Draft",
                action: StepAction::Claude { prompt },
            }
        })
        .collect::<Vec<_>>();

    let window = next.map(|event| (event.start - free_from).num_minutes());
    let to_do = snapshot
        .today
        .iter()
        .find(|item| !dismissed.contains(&format!("todo:{}", item.title)))
        .map(|item| {
            let why = match (current, next, window) {
                (Some(current), _, _) => format!("Next, after {}", current.title),
                (None, Some(next), Some(window)) => {
                    format!("{} free before {}", span(window), next.title)
                }
                _ if now.hour() >= 17 => "Before you call it a day".to_owned(),
                _ => "Your calendar is clear".to_owned(),
            };
            NextStep {
                key: format!("todo:{}", item.title),
                title: item.title.clone(),
                why: if item.high {
                    format!("High priority · {why}")
                } else {
                    why
                },
                button: "Open",
                action: StepAction::OpenTodo,
            }
        });
    let big_chunk = current.is_none() && window.is_none_or(|window| window >= BIG_CHUNK_MINUTES);

    let prep = prep.filter(|step| !dismissed.contains(&step.key));
    let mut steps = Vec::new();
    if prep_soon {
        steps.extend(prep.clone());
    }
    if big_chunk {
        steps.extend(to_do);
        steps.extend(follow_ups);
    } else {
        steps.extend(follow_ups);
        steps.extend(to_do);
    }
    if !prep_soon {
        steps.extend(prep);
    }
    steps.truncate(NEXT_STEP_LIMIT);
    steps
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

    fn owed(text: &str, person: Option<&str>, first_seen: &str) -> OpenLoop {
        OpenLoop {
            file: "people/notes.md".into(),
            line: 3,
            text: format!("- ⏰ {text}"),
            marker: "⏰".into(),
            first_seen: first_seen.into(),
            folder: "people".into(),
            counterparty: person.map(str::to_owned),
        }
    }

    fn snapshot_with(today: &[&str], owed_loops: Vec<OpenLoop>) -> Snapshot {
        Snapshot {
            top_today: today.first().map(|title| (*title).to_owned()),
            today: today
                .iter()
                .map(|title| TodayItem {
                    title: (*title).to_owned(),
                    high: false,
                })
                .collect(),
            owed: owed_loops,
        }
    }

    #[test]
    fn next_steps_fit_the_schedule() {
        let mut review = event("Design review", at(14, 0), 60);
        review.attendee_names = vec!["Patrick Lee".into(), "Ana Ruiz".into()];
        let events = vec![event("standup", at(9, 0), 30), review];
        let snapshot = snapshot_with(
            &["Ship the deck"],
            vec![
                owed("send the contract", Some("Maria"), "2026-11-01"),
                owed("share the mockups", Some("Patrick"), "2026-11-09"),
            ],
        );
        let none = std::collections::HashSet::new();

        // A long free morning: the to-do first, then follow-ups, with the
        // person you see later today ahead of the older one.
        let steps = next_steps(&events, &snapshot, at(10, 0), &none);
        let keys = steps
            .iter()
            .map(|step| step.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(keys, ["Ship the deck", "Message Patrick", "Message Maria"]);
        assert_eq!(steps[0].why, "4h free before Design review");
        assert!(steps[1].why.contains("you see them today"));
        assert!(
            matches!(&steps[1].action, StepAction::Claude { prompt } if prompt.contains("Slack message to Patrick"))
        );

        // An hour before the meeting, prep comes first.
        let steps = next_steps(&events, &snapshot, at(13, 0), &none);
        assert_eq!(steps[0].title, "Prep for Design review");
        assert_eq!(steps[0].why, "2:00 PM, in 1h with Patrick and Ana");
        assert_eq!(steps[1].title, "Ship the deck");

        // Twenty minutes out: prep, then the quick follow-ups.
        let steps = next_steps(&events, &snapshot, at(13, 40), &none);
        let keys = steps
            .iter()
            .map(|step| step.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            ["Prep for Design review", "Message Patrick", "Message Maria"]
        );

        // Dismissed steps make room for the next one.
        let dismissed = std::collections::HashSet::from([steps[0].key.clone()]);
        let steps = next_steps(&events, &snapshot, at(13, 40), &dismissed);
        assert_eq!(
            steps.last().map(|step| step.title.as_str()),
            Some("Ship the deck")
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
