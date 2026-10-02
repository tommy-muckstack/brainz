//! Brainz: matching the outside world (a calendar event, an email
//! screenshot) to a folder in the brain. Deterministic and conservative: a
//! full name or an email domain found in a company folder's own files, or
//! the folder's name in an event title. A first name alone never matches.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use crate::{brain_config::BrainConfig, themes_signals::normalize_key};

/// Mail hosts that never identify a company.
const FREE_MAIL: &[&str] = &[
    "gmail.com",
    "googlemail.com",
    "yahoo.com",
    "hotmail.com",
    "outlook.com",
    "live.com",
    "icloud.com",
    "me.com",
    "mac.com",
    "aol.com",
    "proton.me",
    "protonmail.com",
    "hey.com",
    "fastmail.com",
];

/// Title words that join two names or sides and never name anyone.
const TITLE_JOINERS: &[&str] = &[
    "with",
    "and",
    "x",
    "vs",
    "re",
    "call",
    "chat",
    "sync",
    "meeting",
    "interview",
    "intro",
    "coffee",
    "catch",
    "up",
    "catch-up",
    "prep",
    "debrief",
    "round",
    "reconnect",
    "zoom",
    "google",
    "meet",
    "the",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// Relative path, `companies/acme`.
    pub path: String,
    /// `acme`.
    pub name: String,
    /// Normalized folder name for whole-word matching.
    key: String,
    /// Lowercased text of the folder's own notes (CLAUDE.md and the other
    /// top-level Markdown files), used for roster and name matching.
    text: String,
    domains: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub name: String,
    pub key: String,
    /// Relative path of the person's file.
    pub file: String,
    pub emails: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub folder: String,
    pub folder_name: String,
    /// The person the match hinged on, or the folder's display name.
    pub who: String,
    /// `companies/acme/2026-10-01/acme-prep.md` when a dated folder for the
    /// day holds one.
    pub prep_file: Option<String>,
    pub score: u32,
}

#[derive(Debug, Clone, Default)]
pub struct BrainIndex {
    pub repo: PathBuf,
    pub folders: Vec<Folder>,
    pub people: Vec<Person>,
    stop_words: Vec<String>,
}

impl BrainIndex {
    /// Reads the match folders and the people directory. Blocking file I/O:
    /// callers run it on the background executor.
    pub fn load(repo: &Path, config: &BrainConfig) -> Self {
        let mut folders = Vec::new();
        for dir in config.match_dirs() {
            let Ok(entries) = std::fs::read_dir(repo.join(&dir)) else {
                continue;
            };
            let mut children: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect();
            children.sort();
            for child in children {
                let Some(name) = child.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if name.starts_with('.') || name == "archive" {
                    continue;
                }
                let mut text = String::new();
                if let Ok(files) = std::fs::read_dir(&child) {
                    let mut files: Vec<PathBuf> = files
                        .flatten()
                        .map(|entry| entry.path())
                        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("md"))
                        .collect();
                    files.sort();
                    for file in files {
                        if let Ok(contents) = std::fs::read_to_string(&file) {
                            text.push_str(&contents.to_lowercase());
                            text.push('\n');
                        }
                    }
                }
                let domains = email_domains(&text);
                folders.push(Folder {
                    path: format!("{dir}/{name}"),
                    name: name.to_owned(),
                    key: normalize_key(&display_name(name)),
                    text,
                    domains,
                });
            }
        }
        let mut people = Vec::new();
        let people_dir = repo.join(config.people_dir.trim_end_matches('/'));
        if let Ok(entries) = std::fs::read_dir(&people_dir) {
            let mut files: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("md"))
                .collect();
            files.sort();
            for file in files {
                let Some(stem) = file.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if stem == "CLAUDE" || stem == "README" {
                    continue;
                }
                let text = std::fs::read_to_string(&file).unwrap_or_default();
                let name = display_name(stem);
                people.push(Person {
                    key: normalize_key(&name),
                    name,
                    file: format!("{}/{stem}.md", config.people_dir.trim_end_matches('/')),
                    emails: emails_in(&text),
                });
            }
        }
        Self {
            repo: repo.to_path_buf(),
            folders,
            people,
            stop_words: config.stop_words.clone(),
        }
    }

    /// Matches a calendar event. `date` is the event's day, used to find a
    /// prep file in a dated subfolder.
    pub fn match_event(
        &self,
        title: &str,
        attendee_names: &[String],
        attendee_emails: &[String],
        date: NaiveDate,
    ) -> Option<Match> {
        let mut names: Vec<String> = attendee_names
            .iter()
            .filter(|name| self.is_full_name(name))
            .cloned()
            .collect();
        names.extend(
            title_names(title)
                .into_iter()
                .filter(|n| self.is_full_name(n)),
        );
        let mut found = self.score(Some(title), &names, attendee_emails)?;
        found.prep_file = self.prep_file(&found.folder, date);
        Some(found)
    }

    /// Matches the sender of an email screenshot.
    pub fn match_sender(&self, name: Option<&str>, email: Option<&str>) -> Option<Match> {
        let names: Vec<String> = name
            .filter(|name| self.is_full_name(name))
            .map(|name| vec![name.to_owned()])
            .unwrap_or_default();
        let emails: Vec<String> = email.map(|e| vec![e.to_owned()]).unwrap_or_default();
        self.score(None, &names, &emails)
    }

    fn is_full_name(&self, name: &str) -> bool {
        let words: Vec<&str> = name.split_whitespace().collect();
        if !(2..=4).contains(&words.len()) {
            return false;
        }
        if !words.iter().all(|word| {
            word.chars().next().is_some_and(|c| c.is_uppercase())
                && word
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '\'' || c == '-' || c == '.')
        }) {
            return false;
        }
        // The brain's owner is never the counterparty.
        !words.iter().any(|word| {
            self.stop_words
                .iter()
                .any(|stop| stop.eq_ignore_ascii_case(word))
        })
    }

    fn score(&self, title: Option<&str>, names: &[String], emails: &[String]) -> Option<Match> {
        let title_lower = title.map(|t| t.to_lowercase()).unwrap_or_default();
        // Attendee emails that belong to a known person add that person's name.
        let mut names: Vec<String> = names.to_vec();
        for email in emails {
            let email = email.trim().to_lowercase();
            for person in &self.people {
                if person.emails.contains(&email)
                    && !names.iter().any(|n| normalize_key(n) == person.key)
                {
                    names.push(person.name.clone());
                }
            }
        }
        let domains: Vec<String> = emails
            .iter()
            .filter_map(|email| email.rsplit_once('@').map(|(_, d)| d.trim().to_lowercase()))
            .filter(|domain| !FREE_MAIL.contains(&domain.as_str()))
            .collect();

        let mut best: Vec<(u32, &Folder, Option<String>)> = Vec::new();
        for folder in &self.folders {
            let mut score = 0u32;
            let mut who = None;
            if !title_lower.is_empty() && contains_word(&normalize_key(&title_lower), &folder.key) {
                score += 10;
            }
            for domain in &domains {
                if folder.domains.contains(domain) {
                    score += 8;
                }
            }
            for name in &names {
                let key = normalize_key(name);
                if contains_word(&folder.text, &key) {
                    score += 5;
                    if who.is_none() {
                        who = Some(name.clone());
                    }
                }
            }
            if score > 0 {
                best.push((score, folder, who));
            }
        }
        best.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
        let (score, folder, who) = best.first()?.clone();
        // Two folders tied on the same evidence means we would be guessing.
        if best.get(1).is_some_and(|second| second.0 == score) {
            return None;
        }
        Some(Match {
            folder: folder.path.clone(),
            folder_name: display_name(&folder.name),
            who: who.unwrap_or_else(|| display_name(&folder.name)),
            prep_file: None,
            score,
        })
    }

    /// `<folder>/<YYYY-MM-DD>/*-prep.md`, first by name.
    pub fn prep_file(&self, folder: &str, date: NaiveDate) -> Option<String> {
        let day = date.format("%Y-%m-%d").to_string();
        let dir = self.repo.join(folder).join(&day);
        let mut preps: Vec<String> = std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with("-prep.md") || name == "prep.md")
            .collect();
        preps.sort();
        preps.first().map(|name| format!("{folder}/{day}/{name}"))
    }

    /// The first known person named in a line of text, for open loops.
    pub fn counterparty_in(&self, line: &str) -> Option<String> {
        let lower = line.to_lowercase();
        let normalized = normalize_key(&lower);
        let mut hits: BTreeMap<usize, &str> = BTreeMap::new();
        for person in &self.people {
            if let Some(at) = find_word(&normalized, &person.key) {
                hits.entry(at).or_insert(person.name.as_str());
            }
        }
        hits.into_values().next().map(str::to_owned)
    }
}

/// `grace-hopper` → `Grace Hopper`; `3Dprint` stays.
pub fn display_name(stem: &str) -> String {
    stem.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first)
                    if part
                        .chars()
                        .skip(1)
                        .all(|c| c.is_lowercase() || c.is_ascii_digit()) =>
                {
                    first.to_uppercase().collect::<String>() + chars.as_str()
                }
                _ => part.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Capitalized two-to-four word runs in an event title, split on the usual
/// separators ("Grace Hopper", "Call with Alan Turing / Acme").
pub fn title_names(title: &str) -> Vec<String> {
    let mut names = Vec::new();
    for segment in title.split(['/', '|', ':', ',', '(', ')', '<', '>', '[', ']', '·']) {
        let segment = segment.replace(" - ", " .. ").replace(" .. ", " | ");
        for part in segment.split('|') {
            let mut run: Vec<&str> = Vec::new();
            let mut flush = |run: &mut Vec<&str>| {
                if (2..=4).contains(&run.len()) {
                    names.push(run.join(" "));
                }
                run.clear();
            };
            for token in part.split_whitespace() {
                let word = token.trim_matches(|c: char| {
                    !(c.is_alphanumeric() || c == '\'' || c == '-' || c == '.')
                });
                let lower = word.to_lowercase();
                let capitalized = word.chars().next().is_some_and(|c| c.is_uppercase())
                    && word
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '\'' || c == '-' || c == '.');
                if capitalized && !TITLE_JOINERS.contains(&lower.as_str()) {
                    run.push(word);
                } else {
                    flush(&mut run);
                }
            }
            flush(&mut run);
        }
    }
    names
}

/// Every email address in a text, lowercased.
pub fn emails_in(text: &str) -> Vec<String> {
    let mut emails = Vec::new();
    for token in text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '<' | '>' | '(' | ')' | '[' | ']' | '"' | ',' | ';' | '*' | '`' | '|'
            )
    }) {
        let token = token.trim_matches(|c: char| matches!(c, '.' | ':' | '\''));
        let token = token.strip_prefix("mailto:").unwrap_or(token);
        if let Some((user, domain)) = token.split_once('@')
            && !user.is_empty()
            && domain.contains('.')
            && domain
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            && !domain.ends_with('.')
        {
            let email = format!("{}@{}", user, domain).to_lowercase();
            if !emails.contains(&email) {
                emails.push(email);
            }
        }
    }
    emails
}

fn email_domains(text: &str) -> HashSet<String> {
    emails_in(text)
        .into_iter()
        .filter_map(|email| email.rsplit_once('@').map(|(_, d)| d.to_owned()))
        .filter(|domain| !FREE_MAIL.contains(&domain.as_str()))
        .collect()
}

fn find_word(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let mut start = 0;
    while let Some(found) = haystack[start..].find(needle) {
        let at = start + found;
        let end = at + needle.len();
        let before_ok = at == 0
            || !haystack[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric());
        let after_ok = end >= haystack.len()
            || !haystack[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric());
        if before_ok && after_ok {
            return Some(at);
        }
        start = end;
    }
    None
}

fn contains_word(haystack: &str, needle: &str) -> bool {
    find_word(haystack, needle).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> BrainIndex {
        BrainIndex {
            repo: PathBuf::from("/nonexistent"),
            folders: vec![
                Folder {
                    path: "companies/acme".into(),
                    name: "acme".into(),
                    key: "acme".into(),
                    text: "# acme\n**status:** loop live. roster: grace hopper (cpo), alan turing (ceo). recruiter ada@acme.com\n".into(),
                    domains: ["acme.com".to_owned()].into_iter().collect(),
                },
                Folder {
                    path: "companies/globex".into(),
                    name: "globex".into(),
                    key: "globex".into(),
                    text: "# globex\nroster: grace hopper, hank scorpio. contact hank@globex.io\n".into(),
                    domains: ["globex.io".to_owned()].into_iter().collect(),
                },
            ],
            people: vec![Person {
                name: "Alan Turing".into(),
                key: "alan turing".into(),
                file: "people/alan-turing.md".into(),
                emails: vec!["alan.personal@gmail.com".into()],
            }],
            stop_words: vec!["ada".into()],
        }
    }

    #[test]
    fn full_name_in_title_matches_the_roster_folder() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let found = index().match_event("Hank Scorpio", &[], &[], date).unwrap();
        assert_eq!(found.folder, "companies/globex");
        assert_eq!(found.who, "Hank Scorpio");
        assert_eq!(found.folder_name, "Globex");
        assert!(found.prep_file.is_none());
    }

    #[test]
    fn first_name_alone_never_matches() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        assert!(index().match_event("Hank", &[], &[], date).is_none());
        assert!(
            index()
                .match_event("Coffee with Hank", &["Hank".into()], &[], date)
                .is_none()
        );
    }

    #[test]
    fn shared_name_is_ambiguous_until_title_or_domain_breaks_the_tie() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let index = index();
        assert!(index.match_event("Grace Hopper", &[], &[], date).is_none());
        let found = index
            .match_event("Grace Hopper / Acme", &[], &[], date)
            .unwrap();
        assert_eq!(found.folder, "companies/acme");
        let found = index
            .match_event(
                "Sync",
                &["Grace Hopper".into()],
                &["grace@globex.io".into()],
                date,
            )
            .unwrap();
        assert_eq!(found.folder, "companies/globex");
        assert_eq!(found.who, "Grace Hopper");
    }

    #[test]
    fn known_person_email_resolves_to_their_name_and_owner_is_skipped() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let found = index()
            .match_event("Call", &[], &["alan.personal@gmail.com".into()], date)
            .unwrap();
        assert_eq!(found.folder, "companies/acme");
        assert_eq!(found.who, "Alan Turing");
        // "Ada Lovelace" contains the owner's stop word, so it is not a counterparty.
        assert!(
            index()
                .match_event("Ada Lovelace", &[], &[], date)
                .is_none()
        );
    }

    #[test]
    fn sender_matching_uses_name_or_domain() {
        let index = index();
        let found = index.match_sender(Some("Hank Scorpio"), None).unwrap();
        assert_eq!(found.folder, "companies/globex");
        let found = index.match_sender(None, Some("someone@acme.com")).unwrap();
        assert_eq!(found.folder, "companies/acme");
        assert!(index.match_sender(None, Some("x@gmail.com")).is_none());
    }

    #[test]
    fn title_names_and_emails_parse() {
        assert_eq!(
            title_names("Call with Alan Turing / Acme"),
            vec!["Alan Turing".to_owned()]
        );
        assert_eq!(title_names("Grace Hopper"), vec!["Grace Hopper".to_owned()]);
        assert_eq!(
            title_names("Ada x Grace Hopper - Reconnect"),
            vec!["Grace Hopper".to_owned()]
        );
        assert_eq!(
            emails_in("Grace <grace@globex.io>, cc: mailto:ada@Acme.com. Not an email: a@b"),
            vec!["grace@globex.io".to_owned(), "ada@acme.com".to_owned()]
        );
    }

    #[test]
    fn counterparty_is_the_first_known_person() {
        let index = index();
        assert_eq!(
            index.counterparty_in("⏳ waiting on Alan Turing for the band read"),
            Some("Alan Turing".to_owned())
        );
        assert_eq!(index.counterparty_in("⏰ send the deck"), None);
    }
}
