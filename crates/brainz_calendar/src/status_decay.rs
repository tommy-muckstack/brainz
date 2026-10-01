//! Brainz: status-line decay. Every company and project `CLAUDE.md` opens
//! with a status callout carrying a date. When a sibling note or a dated
//! subfolder is newer than that date, the folder is tinted in the file tree
//! so a stale summary is visible without opening it.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{Datelike, NaiveDate};
use gpui::{App, AppContext as _, Context, Entity, Global, Task};

use crate::brain_config::BrainConfig;

const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
const DEBOUNCE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decay {
    pub folder: String,
    pub status_date: NaiveDate,
    pub newest_date: NaiveDate,
    /// The sibling file or folder name that carries the newest date.
    pub newest_source: String,
}

impl Decay {
    /// "Status 9/22, newest note 9/30."
    pub fn tooltip(&self) -> String {
        format!(
            "Status {}/{}, newest note {}/{}.",
            self.status_date.month(),
            self.status_date.day(),
            self.newest_date.month(),
            self.newest_date.day()
        )
    }
}

/// The first date in the first blockquote of a `CLAUDE.md`. Handles
/// `> **Status 2026-09-22:**`, `> ## 🟢 DONE 9/22`, and `9/22/2026`.
pub fn status_date(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    let mut in_quote = false;
    let mut quote = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('>') {
            in_quote = true;
            quote.push_str(trimmed.trim_start_matches('>'));
            quote.push('\n');
        } else if in_quote && !trimmed.is_empty() {
            break;
        } else if in_quote {
            // A blank line inside a quote keeps the quote going only if the
            // next line is also quoted; stop here otherwise.
            continue;
        }
    }
    if quote.is_empty() {
        return None;
    }
    first_date(&quote, today)
}

/// The first date-looking token in `text`. A full date (`2026-09-22`,
/// `9/22/2026`) anywhere wins over a bare `9/22`, since `1/2` and `3/4` are
/// as often fractions as dates.
pub fn first_date(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    let tokens: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || matches!(c, '*' | ':' | ',' | ';' | '(' | ')' | '[' | ']' | '`' | '\u{201c}' | '\u{201d}'))
        .map(|token| token.trim_matches(|c: char| !(c.is_ascii_digit())))
        .filter(|token| !token.is_empty())
        .collect();
    tokens
        .iter()
        .find_map(|token| parse_date_token(token, today, false))
        .or_else(|| tokens.iter().find_map(|token| parse_date_token(token, today, true)))
}

fn parse_date_token(token: &str, today: NaiveDate, allow_month_day: bool) -> Option<NaiveDate> {
    if let Ok(date) = NaiveDate::parse_from_str(token, "%Y-%m-%d") {
        return Some(date);
    }
    let parts: Vec<&str> = token.split('/').collect();
    match parts.as_slice() {
        [month, day, year] => {
            let year: i32 = year.parse().ok()?;
            let year = if year < 100 { 2000 + year } else { year };
            NaiveDate::from_ymd_opt(year, month.parse().ok()?, day.parse().ok()?)
        }
        [month, day] if allow_month_day => {
            let month: u32 = month.parse().ok()?;
            let day: u32 = day.parse().ok()?;
            let this_year = NaiveDate::from_ymd_opt(today.year(), month, day)?;
            // A month/day more than a week in the future belongs to last year.
            if this_year > today + chrono::Duration::days(7) {
                NaiveDate::from_ymd_opt(today.year() - 1, month, day)
            } else {
                Some(this_year)
            }
        }
        _ => None,
    }
}

/// `2026-09-30-notes.md` or `2026-10-01` → that date.
pub fn date_in_name(name: &str) -> Option<NaiveDate> {
    let bytes = name.as_bytes();
    bytes.windows(10).find_map(|window| {
        let text = std::str::from_utf8(window).ok()?;
        NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
    })
}

/// Compares a folder's status date with its newest sibling date.
pub fn compare(
    status_date: NaiveDate,
    siblings: &[(String, NaiveDate)],
) -> Option<(NaiveDate, String)> {
    let (name, newest) = siblings
        .iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(name, date)| (name.clone(), *date))?;
    (newest > status_date).then_some((newest, name))
}

fn folder_decay(repo: &Path, folder: &str, today: NaiveDate) -> Option<Decay> {
    let dir = repo.join(folder);
    let text = std::fs::read_to_string(dir.join("CLAUDE.md")).ok()?;
    let status = status_date(&text, today)?;
    let mut siblings = Vec::new();
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(date) = date_in_name(&name) {
            siblings.push((name, date));
        }
    }
    let (newest, source) = compare(status, &siblings)?;
    Some(Decay {
        folder: folder.to_owned(),
        status_date: status,
        newest_date: newest,
        newest_source: source,
    })
}

/// Every stale folder under the match and vocabulary folders, keyed by
/// absolute path. Blocking file I/O: run on the background executor.
pub fn scan(repo: &Path, config: &BrainConfig, today: NaiveDate) -> HashMap<PathBuf, Decay> {
    let mut parents: Vec<String> = config.match_dirs();
    for folder in &config.vocabulary_folders {
        let folder = folder.trim_matches('/').to_owned();
        if !parents.contains(&folder) {
            parents.push(folder);
        }
    }
    let mut stale = HashMap::new();
    for parent in parents {
        let Ok(entries) = std::fs::read_dir(repo.join(&parent)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with('.') || name == "archive" {
                continue;
            }
            let relative = format!("{parent}/{name}");
            if let Some(decay) = folder_decay(repo, &relative, today) {
                stale.insert(path, decay);
            }
        }
    }
    stale
}

pub struct DecayState {
    repo: Option<PathBuf>,
    stale: HashMap<PathBuf, Decay>,
    _poll: Option<Task<()>>,
    _debounce: Option<Task<()>>,
}

struct GlobalDecayState(Entity<DecayState>);

impl Global for GlobalDecayState {}

pub fn init(cx: &mut App) {
    let state = cx.new(|_| DecayState {
        repo: None,
        stale: HashMap::new(),
        _poll: None,
        _debounce: None,
    });
    cx.set_global(GlobalDecayState(state));
}

pub fn state(cx: &App) -> Option<Entity<DecayState>> {
    cx.try_global::<GlobalDecayState>().map(|global| global.0.clone())
}

/// Starts watching `repo`; a no-op once it is being watched.
pub fn watch(repo: PathBuf, cx: &mut App) {
    if let Some(state) = state(cx) {
        state.update(cx, |state, cx| state.ensure_watching(repo, cx));
    }
}

/// The decay for `folder` (absolute), if its status line is stale.
pub fn lookup(folder: &Path, cx: &App) -> Option<Decay> {
    state(cx)?.read(cx).stale.get(folder).cloned()
}

/// Re-scans soon; called when files change so a fixed callout clears
/// without waiting for the next poll.
pub fn request_refresh(cx: &mut App) {
    if let Some(state) = state(cx) {
        state.update(cx, |state, cx| state.refresh_soon(cx));
    }
}

impl DecayState {
    fn ensure_watching(&mut self, repo: PathBuf, cx: &mut Context<Self>) {
        if self.repo.as_deref() == Some(repo.as_path()) {
            return;
        }
        self.repo = Some(repo);
        self._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(POLL_INTERVAL).await;
            }
        }));
    }

    fn refresh_soon(&mut self, cx: &mut Context<Self>) {
        if self.repo.is_none() {
            return;
        }
        self._debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            this.update(cx, |this, cx| this.refresh(cx)).ok();
        }));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let stale = cx
                .background_spawn(async move {
                    let config = BrainConfig::load(&repo);
                    scan(&repo, &config, chrono::Local::now().date_naive())
                })
                .await;
            this.update(cx, |this, cx| {
                if this.stale != stale {
                    let mut names: Vec<&str> =
                        stale.values().map(|decay| decay.folder.as_str()).collect();
                    names.sort_unstable();
                    log::info!("brainz status decay: {} stale folder(s): {}", names.len(), names.join(", "));
                    this.stale = stale;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn status_date_reads_both_callout_styles() {
        let today = day(2026, 10, 1);
        assert_eq!(
            status_date("# Acme\n\n> **Status 2026-09-22:** loop live.\n\nBody 2026-09-30.\n", today),
            Some(day(2026, 9, 22))
        );
        assert_eq!(
            status_date("# Acme\n> ## 🟢 HOUSTON ONSITE DAY ONE DONE 9/28. Debrief later.\n>\n> more 9/30\n", today),
            Some(day(2026, 9, 28))
        );
        assert_eq!(
            status_date("> **Brand:** Acme. Decided 12/15 last year.\n", today),
            Some(day(2025, 12, 15))
        );
        assert_eq!(
            status_date("> ✅ First call done (2026-06-05) .. went well.\n", today),
            Some(day(2026, 6, 5))
        );
        assert_eq!(
            status_date("> **Status:** 1/2 the team is out; onsite was 2026-09-28 and 3/4 of the loop is done.\n", today),
            Some(day(2026, 9, 28))
        );
        assert_eq!(status_date("# No quote\n\nJust text 2026-09-30.\n", today), None);
        assert_eq!(status_date("> Hi Team,\n> no dates here\n", today), None);
    }

    #[test]
    fn newest_sibling_beats_the_status_only_when_newer() {
        let status = day(2026, 9, 22);
        let siblings = vec![
            ("2026-09-15".to_owned(), day(2026, 9, 15)),
            ("2026-09-30-notes.md".to_owned(), day(2026, 9, 30)),
            ("correspondence-log.md".to_owned(), day(2026, 9, 1)),
        ];
        assert_eq!(
            compare(status, &siblings),
            Some((day(2026, 9, 30), "2026-09-30-notes.md".to_owned()))
        );
        assert_eq!(compare(day(2026, 9, 30), &siblings), None);
        assert_eq!(compare(day(2026, 10, 1), &siblings), None);
        assert_eq!(date_in_name("turing-2026-10-01-prep.md"), Some(day(2026, 10, 1)));
        assert_eq!(date_in_name("CLAUDE.md"), None);
    }

    #[test]
    fn scan_tints_only_stale_folders_and_updating_the_callout_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let today = day(2026, 10, 1);
        std::fs::create_dir_all(repo.join("companies/acme/2026-09-30")).unwrap();
        std::fs::create_dir_all(repo.join("companies/globex")).unwrap();
        std::fs::create_dir_all(repo.join("companies/archive/old/2026-09-30")).unwrap();
        std::fs::write(repo.join("companies/acme/CLAUDE.md"), "> **Status 2026-09-22:** x\n").unwrap();
        std::fs::write(repo.join("companies/globex/CLAUDE.md"), "> **Status 2026-09-30:** x\n").unwrap();
        std::fs::write(repo.join("companies/globex/2026-09-29-notes.md"), "").unwrap();
        std::fs::write(repo.join("companies/archive/old/CLAUDE.md"), "> **Status 2026-01-01:** x\n").unwrap();
        let config = BrainConfig {
            vocabulary_folders: vec!["companies".into()],
            ..BrainConfig::default()
        };
        let stale = scan(repo, &config, today);
        assert_eq!(stale.len(), 1, "{stale:?}");
        let decay = stale.get(&repo.join("companies/acme")).unwrap();
        assert_eq!(decay.tooltip(), "Status 9/22, newest note 9/30.");
        std::fs::write(repo.join("companies/acme/CLAUDE.md"), "> **Status 2026-09-30:** x\n").unwrap();
        assert!(scan(repo, &config, today).is_empty());
    }
}
