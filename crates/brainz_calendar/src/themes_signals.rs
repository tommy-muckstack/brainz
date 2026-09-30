//! Brainz: the deterministic signals pass behind the Themes tab. Every number
//! here comes from `git log` on the brain; no model is involved. Output lands
//! in `ops/themes/` (`signals.json`, the generated block of `themes.md`), and
//! `pins.md` / `vocabulary.md` are read for curation but never rewritten once
//! they exist.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use anyhow::{Context as _, Result, bail};
use chrono::{Datelike, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::brain_config::BrainConfig;

pub const SIGNALS_NAME: &str = "signals.json";
pub const THEMES_NAME: &str = "themes.md";
pub const PINS_NAME: &str = "pins.md";
pub const VOCABULARY_NAME: &str = "vocabulary.md";

pub const SERIES_WEEKS: usize = 12;
/// A term needs this much weight over at least `MIN_WEEKS` distinct weeks to
/// become a theme on its own; pinned terms skip the bar.
const MIN_WEIGHT: u32 = 5;
const MIN_WEEKS: usize = 2;
/// Momentum is only ranked when the last two weeks carry this much weight,
/// so a theme with three mentions can't top the Rising list.
const MOMENTUM_SUPPORT: u32 = 5;
/// A theme with no prior history needs this much recent weight to count as
/// new and rising; otherwise every fresh proper noun would top the list.
const NEW_SUPPORT: u32 = 8;
const RISING_MIN: f64 = 1.2;
const FADING_MAX: f64 = 0.5;
/// Fading needs real history first: at least this much weight in the six
/// weeks before the last two.
const FADING_PRIOR_MIN: u32 = 6;
const LIST_LIMIT: usize = 10;
const TOP_LIMIT: usize = 6;
const MIN_TERM_CHARS: usize = 3;
const MAX_TERM_CHARS: usize = 60;
const MAX_RUN_WORDS: usize = 4;

const GENERATED_START: &str = "<!-- generated:start -->";
const GENERATED_END: &str = "<!-- generated:end -->";
const NARRATIVE_START: &str = "<!-- narrative:start -->";
const NARRATIVE_END: &str = "<!-- narrative:end -->";

const STOPLIST: &[&str] = &[
    "tommy", "claude", "read", "status", "next", "the", "monday", "tuesday", "wednesday",
    "thursday", "friday", "saturday", "sunday", "mon", "tue", "tues", "wed", "thu", "thur",
    "thurs", "fri", "sat", "sun", "january", "february", "march", "april", "may", "june", "july",
    "august", "september", "october", "november", "december", "jan", "feb", "mar", "apr", "jun",
    "jul", "aug", "sep", "sept", "oct", "nov", "dec", "note", "notes", "todo", "done", "yes", "no",
    "see", "also", "this", "that", "new", "last", "first", "not", "and", "but", "for", "with",
    "all", "any", "from", "into", "only", "over", "than", "then", "when", "what", "who", "why",
    "how", "are", "was", "were", "will", "can", "must", "should", "never", "always", "every",
    "each", "more", "most", "some", "such", "very", "just", "now", "here", "there", "out", "off",
    "own", "today", "tomorrow", "yesterday", "week", "day", "time", "update", "updated", "open",
    "closed", "pending", "why", "because", "before", "after", "one", "two", "three", "total",
    "source", "et", "pdf", "png", "docx", "link", "links", "file", "files", "folder", "email",
];
/// Threads need this much total weight, so a term that brushed three folders
/// once does not read as a cross-folder thread.
const THREAD_MIN_TOTAL: u32 = 15;
const THREAD_LIMIT: usize = 20;
/// Bold spans and wiki links longer than this are sentences, not terms.
const MAX_TERM_WORDS: usize = 4;
/// More digits than this means a date, a time, or an id.
const MAX_TERM_DIGITS: usize = 3;

const VOCABULARY_SEEDS: &[&str] = &[
    "PLG", "PLS", "MCP", "WHOOP", "ACP", "ICP", "OKR", "ARR", "NPS", "SEO", "CTA",
];

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Signals {
    pub generated_at: String,
    pub commit: String,
    pub commits: usize,
    pub weeks: Vec<String>,
    pub themes: Vec<Theme>,
    pub threads: Vec<Thread>,
    pub open_loops: Vec<OpenLoops>,
    pub rising: Vec<String>,
    /// Themes with no history before the last two weeks.
    pub fresh: Vec<String>,
    pub fading: Vec<String>,
    pub run_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub total: u32,
    pub weeks_active: usize,
    pub series: Vec<u32>,
    /// Mean of the last two weeks over the mean of the six before; `None`
    /// when the prior six weeks are empty (see `is_new`).
    pub momentum: Option<f64>,
    pub recent: u32,
    pub prior: u32,
    pub is_new: bool,
    pub folders: Vec<Weighted>,
    pub files: Vec<Weighted>,
    pub people: Vec<Weighted>,
    pub pinned: bool,
    pub hidden: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Weighted {
    pub name: String,
    pub weight: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Thread {
    pub theme: String,
    pub folders: Vec<String>,
    pub links: Vec<LinkCheck>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct LinkCheck {
    pub from: String,
    pub from_file: String,
    pub to: String,
    pub to_file: String,
    pub linked: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct OpenLoops {
    pub folder: String,
    pub now: u32,
    pub week_ago: u32,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Pins {
    pub pin: Vec<String>,
    pub rename: Vec<(String, String)>,
    pub merge: Vec<(Vec<String>, String)>,
    pub hide: Vec<String>,
}

impl Pins {
    /// Maps a raw term key onto its curated key, following renames and
    /// merges, and returns the display name to use when one was given.
    fn canonical(&self, key: &str) -> (String, Option<String>) {
        for (from, to) in &self.rename {
            if from == key {
                return (normalize_key(to), Some(to.clone()));
            }
        }
        for (sources, target) in &self.merge {
            if sources.iter().any(|source| source == key) {
                return (normalize_key(target), Some(target.clone()));
            }
        }
        (key.to_owned(), None)
    }

    fn is_pinned(&self, key: &str) -> bool {
        self.pin.iter().any(|pin| pin == key)
    }

    fn is_hidden(&self, key: &str) -> bool {
        self.hide.iter().any(|hide| hide == key)
    }
}

/// `- pin: x`, `- rename: a => b`, `- merge: a, b => c`, `- hide: x`.
/// Anything else, including HTML comments, is ignored.
pub fn parse_pins(text: &str) -> Pins {
    let mut pins = Pins::default();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("<!--") {
            continue;
        }
        let Some(rest) = line.strip_prefix("- ") else {
            continue;
        };
        let Some((kind, value)) = rest.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match kind.trim() {
            "pin" => pins.pin.push(normalize_key(value)),
            "hide" => pins.hide.push(normalize_key(value)),
            "rename" => {
                if let Some((from, to)) = value.split_once("=>") {
                    pins.rename
                        .push((normalize_key(from), to.trim().to_owned()));
                }
            }
            "merge" => {
                if let Some((sources, target)) = value.split_once("=>") {
                    let sources = sources
                        .split(',')
                        .map(normalize_key)
                        .filter(|source| !source.is_empty())
                        .collect::<Vec<_>>();
                    if !sources.is_empty() {
                        pins.merge.push((sources, target.trim().to_owned()));
                    }
                }
            }
            _ => {}
        }
    }
    pins
}

/// Lowercase, punctuation stripped to spaces, single-spaced.
pub fn normalize_key(term: &str) -> String {
    let mut key = String::with_capacity(term.len());
    let mut last_space = true;
    for ch in term.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_alphanumeric() || ch == '-' || ch == '&' || ch == '\'' {
            key.push(ch);
            last_space = false;
        } else if !last_space {
            key.push(' ');
            last_space = true;
        }
    }
    key.trim().to_owned()
}

fn is_stopword(word: &str) -> bool {
    STOPLIST.contains(&word.to_ascii_lowercase().as_str())
}

/// A term the pass will count, with the display casing to show for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Term {
    pub key: String,
    pub display: String,
}

fn make_term(display: &str) -> Option<Term> {
    let display = display.trim().trim_matches(|c: char| !c.is_alphanumeric() && c != '&');
    let key = normalize_key(display);
    if key.chars().count() < MIN_TERM_CHARS || key.chars().count() > MAX_TERM_CHARS {
        return None;
    }
    if key.chars().all(|c| c.is_ascii_digit() || c == '-' || c == ' ') {
        return None;
    }
    if key.chars().filter(|c| c.is_ascii_digit()).count() > MAX_TERM_DIGITS {
        return None;
    }
    if key.split(' ').count() > MAX_TERM_WORDS {
        return None;
    }
    // snake_case identifiers (WHOOP metric names, code) are not themes.
    if display.contains('_') {
        return None;
    }
    if key.split(' ').any(is_stopword) {
        return None;
    }
    Some(Term {
        key,
        display: display.to_owned(),
    })
}

/// The proper-noun vocabulary built from the repo plus the acronym allowlist.
#[derive(Debug, Default, Clone)]
pub struct Vocabulary {
    /// (lowercase match text, display name, is a person)
    entries: Vec<(String, String, bool)>,
}

impl Vocabulary {
    pub fn from_tree(paths: &[String], acronyms: &[String], config: &BrainConfig) -> Self {
        let people_prefix = format!("{}/", config.people_dir.trim_end_matches('/'));
        let folder_prefixes: Vec<String> = config
            .vocabulary_folders
            .iter()
            .map(|folder| format!("{}/", folder.trim_end_matches('/')))
            .collect();
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        let mut add = |stem: &str, person: bool, entries: &mut Vec<(String, String, bool)>| {
            let display = title_case(stem);
            let key = normalize_key(&display);
            if key.chars().count() < MIN_TERM_CHARS || !seen.insert(key.clone()) {
                return;
            }
            entries.push((key, display, person));
        };
        for path in paths {
            if let Some(stem) = path
                .strip_prefix(people_prefix.as_str())
                .and_then(|rest| rest.strip_suffix(".md"))
                && !stem.contains('/')
                && stem != "CLAUDE"
            {
                add(stem, true, &mut entries);
            }
            for prefix in &folder_prefixes {
                if let Some(rest) = path.strip_prefix(prefix.as_str())
                    && let Some((folder, _)) = rest.split_once('/')
                    && folder != "archive"
                {
                    add(folder, false, &mut entries);
                }
            }
        }
        for acronym in acronyms {
            let key = acronym.trim().to_ascii_lowercase();
            if key.len() >= 2 && seen.insert(key.clone()) {
                entries.push((key, acronym.trim().to_owned(), false));
            }
        }
        Self { entries }
    }

    pub fn is_person(&self, key: &str) -> bool {
        self.entries
            .iter()
            .any(|(entry_key, _, person)| *person && entry_key == key)
    }
}

/// `kelly-jacobs` → `Kelly Jacobs`; `24Mason` stays `24Mason`.
fn title_case(stem: &str) -> String {
    stem.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) if part.chars().skip(1).all(|c| c.is_lowercase() || c.is_ascii_digit()) => {
                    first.to_uppercase().collect::<String>() + chars.as_str()
                }
                _ => part.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn contains_word(haystack: &str, needle: &str) -> bool {
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
            return true;
        }
        start = end;
    }
    false
}

fn strip_token(token: &str) -> &str {
    token.trim_matches(|c: char| {
        !(c.is_alphanumeric() || c == '&' || c == '\'' || c == '-')
    })
}

fn is_capitalized_word(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_uppercase()
        && chars.all(|c| c.is_alphanumeric() || c == '\'' || c == '-' || c == '&')
}

/// Candidate terms in one added line: bold spans, wiki links, vocabulary
/// hits, and capitalized runs of two to four words.
pub fn extract_terms(line: &str, vocabulary: &Vocabulary) -> Vec<Term> {
    let mut terms: Vec<Term> = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |term: Option<Term>| {
        if let Some(term) = term
            && seen.insert(term.key.clone())
        {
            terms.push(term);
        }
    };
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") || trimmed.starts_with("---") {
        return terms;
    }

    let mut rest = line;
    while let Some(open) = rest.find("**") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("**") else {
            break;
        };
        push(make_term(&after[..close]));
        rest = &after[close + 2..];
    }

    let mut rest = line;
    while let Some(open) = rest.find("[[") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("]]") else {
            break;
        };
        let target = after[..close].split('|').next().unwrap_or("");
        let target = target.split('#').next().unwrap_or("");
        let target = target.rsplit('/').next().unwrap_or("");
        let target = target.strip_suffix(".md").unwrap_or(target);
        push(make_term(&title_case(target)));
        rest = &after[close + 2..];
    }

    let lower = line.to_ascii_lowercase();
    for (key, display, _) in &vocabulary.entries {
        if contains_word(&lower, key) {
            push(make_term(display));
        }
    }

    let mut run: Vec<&str> = Vec::new();
    let flush = |run: &mut Vec<&str>, push: &mut dyn FnMut(Option<Term>)| {
        if run.len() >= 2 && run.len() <= MAX_RUN_WORDS {
            push(make_term(&run.join(" ")));
        }
        run.clear();
    };
    for token in line.split_whitespace() {
        let word = strip_token(token);
        let ends_sentence = token.ends_with(['.', ':', ',', ';', '!', '?']);
        if is_capitalized_word(word) && word.chars().count() >= 2 {
            run.push(word);
            if ends_sentence {
                flush(&mut run, &mut push);
            }
        } else {
            flush(&mut run, &mut push);
        }
    }
    flush(&mut run, &mut push);
    terms
}

pub fn excluded(path: &str, config: &BrainConfig) -> bool {
    let themes_prefix = format!("{}/", config.themes_dir.trim_end_matches('/'));
    if path.starts_with(&themes_prefix) {
        return true;
    }
    if config
        .exclude_prefixes
        .iter()
        .any(|prefix| path.starts_with(&format!("{}/", prefix.trim_end_matches('/'))))
    {
        return true;
    }
    for dir in &config.dated_exclude_dirs {
        if let Some(rest) = path.strip_prefix(&format!("{}/", dir.trim_end_matches('/'))) {
            let name = rest.rsplit('/').next().unwrap_or(rest);
            if looks_like_date(name) {
                return true;
            }
        }
    }
    false
}

fn looks_like_date(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.windows(10).any(|window| {
        window[4] == b'-'
            && window[7] == b'-'
            && window.iter().enumerate().all(|(i, b)| {
                if i == 4 || i == 7 {
                    true
                } else {
                    b.is_ascii_digit()
                }
            })
    })
}

pub fn top_folder(path: &str) -> String {
    match path.split_once('/') {
        Some((folder, _)) => folder.to_owned(),
        None => "(root)".to_owned(),
    }
}

pub fn week_label(date: NaiveDate) -> String {
    let week = date.iso_week();
    format!("{}-W{:02}", week.year(), week.week())
}

/// The last `SERIES_WEEKS` ISO weeks ending with the week of `today`.
pub fn recent_weeks(today: NaiveDate) -> Vec<String> {
    (0..SERIES_WEEKS)
        .rev()
        .map(|back| week_label(today - chrono::Duration::weeks(back as i64)))
        .collect()
}

/// Mean of the last two weeks over the mean of the six before, plus the raw
/// sums so callers can apply support floors. `None` when the prior six
/// weeks are empty.
pub fn momentum(series: &[u32]) -> (Option<f64>, u32, u32) {
    let len = series.len();
    if len < 8 {
        return (None, 0, 0);
    }
    let recent: u32 = series[len - 2..].iter().sum();
    let prior: u32 = series[len - 8..len - 2].iter().sum();
    if prior == 0 {
        return (None, recent, prior);
    }
    let recent_mean = recent as f64 / 2.0;
    let prior_mean = prior as f64 / 6.0;
    (Some(recent_mean / prior_mean), recent, prior)
}

pub fn sparkline(series: &[u32]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = series.iter().copied().max().unwrap_or(0);
    series
        .iter()
        .map(|&value| {
            if value == 0 || max == 0 {
                BARS[0]
            } else {
                let level = ((value as f64 / max as f64) * 7.0).floor() as usize;
                BARS[level.clamp(1, 7)]
            }
        })
        .collect()
}

fn command(repo: &Path, program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(repo);
    let path = std::env::var("PATH").unwrap_or_default();
    cmd.env(
        "PATH",
        format!("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:{path}"),
    );
    if std::env::var_os("DEVELOPER_DIR").is_none()
        && Path::new("/Library/Developer/CommandLineTools").is_dir()
    {
        cmd.env("DEVELOPER_DIR", "/Library/Developer/CommandLineTools");
    }
    cmd
}

/// Blocking on purpose: the pass only ever runs on the background executor.
#[allow(clippy::disallowed_methods)]
fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = command(repo, "git")
        .args(args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!("`git {}` failed ({}): {stderr}", args.join(" "), output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[derive(Default)]
struct TermAgg {
    display: String,
    /// Votes per spelling, so "Growth Loops" wins over "GROWTH LOOPS".
    casings: HashMap<String, u32>,
    weeks: BTreeMap<String, u32>,
    folders: HashMap<String, u32>,
    files: HashMap<String, u32>,
    people: HashMap<String, u32>,
    total: u32,
}

fn read_list_file(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- ").map(|s| s.trim().to_owned()))
        .filter(|entry| !entry.is_empty())
        .collect()
}

fn default_vocabulary() -> String {
    let mut text = String::from(
        "# Themes vocabulary\n\nAcronyms and short terms the signals pass counts as themes even \
         though they are not capitalized runs. One per line as `- TERM`. Matching is whole-word \
         and case-insensitive. Brainz reads this file and never rewrites it.\n\n",
    );
    for seed in VOCABULARY_SEEDS {
        text.push_str("- ");
        text.push_str(seed);
        text.push('\n');
    }
    text
}

fn default_pins() -> String {
    "# Themes pins\n\nHuman curation for the signals pass. The Themes tab in Brainz appends lines \
     here; editing by hand works too. The pass reads this file and never rewrites it. Hidden themes \
     stay in `signals.json`, they just stop rendering.\n\nOne line per rule, in these forms \
     (uncomment to use):\n\n<!-- - pin: tekmetric -->\n<!-- - rename: pls => product-led sales -->\n\
     <!-- - merge: roofer, acme => acme -->\n<!-- - hide: read -->\n"
        .to_owned()
}

fn top_weighted(map: &HashMap<String, u32>, limit: usize) -> Vec<Weighted> {
    let mut entries: Vec<Weighted> = map
        .iter()
        .map(|(name, weight)| Weighted {
            name: name.clone(),
            weight: *weight,
        })
        .collect();
    entries.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.name.cmp(&b.name)));
    entries.truncate(limit);
    entries
}

/// Runs the whole pass against the brain checkout at `repo` and writes the
/// output files. Returns the signals for immediate display.
pub fn run_pass(repo: &Path) -> Result<Signals> {
    let started = Instant::now();
    let config = BrainConfig::load(repo);
    let themes_dir = repo.join(config.themes_dir.trim_end_matches('/'));
    std::fs::create_dir_all(&themes_dir)
        .with_context(|| format!("creating {}", themes_dir.display()))?;
    let vocabulary_path = config.themes_path(repo, VOCABULARY_NAME);
    if !vocabulary_path.exists() {
        std::fs::write(&vocabulary_path, default_vocabulary())
            .with_context(|| format!("writing {}", vocabulary_path.display()))?;
    }
    let pins_path = config.themes_path(repo, PINS_NAME);
    if !pins_path.exists() {
        std::fs::write(&pins_path, default_pins())
            .with_context(|| format!("writing {}", pins_path.display()))?;
    }
    let pins = parse_pins(&std::fs::read_to_string(&pins_path).unwrap_or_default());
    let acronyms = read_list_file(&vocabulary_path);

    let head = git(repo, &["rev-parse", "--short", "HEAD"])?.trim().to_owned();
    let tree: Vec<String> = git(repo, &["ls-tree", "-r", "--name-only", "HEAD"])?
        .lines()
        .map(str::to_owned)
        .collect();
    let vocabulary = Vocabulary::from_tree(&tree, &acronyms, &config);

    let log = git(
        repo,
        &[
            "log",
            "--reverse",
            "--no-color",
            "--date=short",
            "--format=%x01%H %ad",
            "-p",
            "--",
            "*.md",
        ],
    )?;

    let mut aggregates: HashMap<String, TermAgg> = HashMap::new();
    let mut commits = 0usize;
    let mut week = String::new();
    let mut path: Option<String> = None;
    for line in log.lines() {
        if let Some(header) = line.strip_prefix('\u{1}') {
            commits += 1;
            let date = header.split(' ').nth(1).unwrap_or("");
            week = NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .map(week_label)
                .unwrap_or_default();
            path = None;
            continue;
        }
        if let Some(file) = line.strip_prefix("+++ ") {
            let file = file.strip_prefix("b/").unwrap_or(file);
            path = (file != "/dev/null" && file.ends_with(".md") && !excluded(file, &config))
                .then(|| file.to_owned());
            continue;
        }
        let Some(file) = path.as_ref() else {
            continue;
        };
        if line.starts_with("+++") || !line.starts_with('+') {
            continue;
        }
        let added = &line[1..];
        let terms = extract_terms(added, &vocabulary);
        if terms.is_empty() {
            continue;
        }
        let people: Vec<String> = terms
            .iter()
            .filter(|term| vocabulary.is_person(&term.key))
            .map(|term| term.display.clone())
            .collect();
        let folder = top_folder(file);
        for term in &terms {
            let (key, display_override) = pins.canonical(&term.key);
            let agg = aggregates.entry(key).or_default();
            match display_override {
                Some(display) => agg.display = display,
                None => *agg.casings.entry(term.display.clone()).or_default() += 1,
            }
            agg.total += 1;
            *agg.weeks.entry(week.clone()).or_default() += 1;
            *agg.folders.entry(folder.clone()).or_default() += 1;
            *agg.files.entry(file.clone()).or_default() += 1;
            for person in &people {
                if normalize_key(person) != term.key {
                    *agg.people.entry(person.clone()).or_default() += 1;
                }
            }
        }
    }

    let today = Local::now().date_naive();
    let weeks = recent_weeks(today);
    let mut themes: Vec<Theme> = aggregates
        .into_iter()
        .filter(|(key, agg)| {
            pins.is_pinned(key) || (agg.total >= MIN_WEIGHT && agg.weeks.len() >= MIN_WEEKS)
        })
        .map(|(key, agg)| {
            let name = if agg.display.is_empty() {
                agg.casings
                    .iter()
                    .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                    .map(|(display, _)| display.clone())
                    .unwrap_or_else(|| key.clone())
            } else {
                agg.display.clone()
            };
            let series: Vec<u32> = weeks
                .iter()
                .map(|week| agg.weeks.get(week).copied().unwrap_or(0))
                .collect();
            let (momentum, recent, prior) = momentum(&series);
            Theme {
                pinned: pins.is_pinned(&key),
                hidden: pins.is_hidden(&key),
                name,
                total: agg.total,
                weeks_active: agg.weeks.len(),
                is_new: prior == 0 && recent >= NEW_SUPPORT,
                momentum,
                recent,
                prior,
                folders: top_weighted(&agg.folders, TOP_LIMIT),
                files: top_weighted(&agg.files, TOP_LIMIT),
                people: top_weighted(&agg.people, TOP_LIMIT),
                series,
                id: key,
            }
        })
        .collect();
    themes.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.id.cmp(&b.id)));

    let visible = || themes.iter().filter(|theme| !theme.hidden);
    let mut rising: Vec<&Theme> = visible()
        .filter(|theme| {
            theme.recent >= MOMENTUM_SUPPORT && theme.momentum.is_some_and(|m| m >= RISING_MIN)
        })
        .collect();
    // Rank by attention gained (recent weekly mean minus prior weekly mean),
    // not by ratio: a ratio makes anything with a near-zero prior look huge.
    let gain = |theme: &Theme| theme.recent as f64 / 2.0 - theme.prior as f64 / 6.0;
    rising.sort_by(|a, b| {
        gain(b)
            .partial_cmp(&gain(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.recent.cmp(&a.recent))
    });
    let rising: Vec<String> = rising.iter().take(LIST_LIMIT).map(|t| t.id.clone()).collect();
    let mut fresh: Vec<&Theme> = visible().filter(|theme| theme.is_new).collect();
    fresh.sort_by(|a, b| b.recent.cmp(&a.recent).then_with(|| a.id.cmp(&b.id)));
    let fresh: Vec<String> = fresh.iter().take(LIST_LIMIT).map(|t| t.id.clone()).collect();
    let mut fading: Vec<&Theme> = visible()
        .filter(|theme| {
            theme.prior >= FADING_PRIOR_MIN && theme.momentum.is_some_and(|m| m <= FADING_MAX)
        })
        .collect();
    fading.sort_by(|a, b| {
        gain(a)
            .partial_cmp(&gain(b))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.prior.cmp(&a.prior))
    });
    let fading: Vec<String> = fading.iter().take(LIST_LIMIT).map(|t| t.id.clone()).collect();

    let threads = find_threads(repo, &themes);
    let open_loops = open_loops(repo)?;

    let signals = Signals {
        generated_at: Local::now().format("%Y-%m-%d %H:%M").to_string(),
        commit: head,
        commits,
        weeks,
        themes,
        threads,
        open_loops,
        rising,
        fresh,
        fading,
        run_ms: started.elapsed().as_millis() as u64,
    };

    let signals_path = config.themes_path(repo, SIGNALS_NAME);
    let json = serde_json::to_string_pretty(&signals).context("serializing signals")?;
    std::fs::write(&signals_path, json + "\n")
        .with_context(|| format!("writing {}", signals_path.display()))?;
    write_themes_md(repo, &config, &signals)?;
    Ok(signals)
}

/// Themes that live in three or more top-level folders, with a check for
/// whether the top file in each folder links to the top file in the others.
fn find_threads(repo: &Path, themes: &[Theme]) -> Vec<Thread> {
    let mut threads = Vec::new();
    for theme in themes
        .iter()
        .filter(|theme| !theme.hidden && theme.total >= THREAD_MIN_TOTAL)
    {
        let folders: Vec<&Weighted> = theme
            .folders
            .iter()
            .filter(|folder| folder.name != "(root)" && folder.name != ".claude")
            .collect();
        if folders.len() < 3 {
            continue;
        }
        let top_file = |folder: &str| -> Option<String> {
            theme
                .files
                .iter()
                .find(|file| top_folder(&file.name) == folder)
                .map(|file| file.name.clone())
        };
        let mut links = Vec::new();
        for (i, from) in folders.iter().enumerate() {
            for to in folders.iter().skip(i + 1) {
                let (Some(from_file), Some(to_file)) = (top_file(&from.name), top_file(&to.name))
                else {
                    continue;
                };
                let linked = file_links_to(repo, &from_file, &to_file)
                    || file_links_to(repo, &to_file, &from_file);
                links.push(LinkCheck {
                    from: from.name.clone(),
                    from_file,
                    to: to.name.clone(),
                    to_file,
                    linked,
                });
            }
        }
        threads.push(Thread {
            theme: theme.id.clone(),
            folders: folders.iter().map(|folder| folder.name.clone()).collect(),
            links,
        });
    }
    threads.sort_by_key(|thread| std::cmp::Reverse(thread.folders.len()));
    threads.truncate(THREAD_LIMIT);
    threads
}

fn file_links_to(repo: &Path, from: &str, to: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(repo.join(from)) else {
        return false;
    };
    let stem = to.rsplit('/').next().unwrap_or(to);
    let stem = stem.strip_suffix(".md").unwrap_or(stem);
    text.contains(to) || text.contains(&format!("[[{stem}")) || text.contains(&format!("[[{to}"))
}

fn open_loops(repo: &Path) -> Result<Vec<OpenLoops>> {
    let mut by_folder: BTreeMap<String, OpenLoops> = BTreeMap::new();
    let now = git(
        repo,
        &["grep", "-c", "-e", "⏳", "-e", "⏰", "--", "*.md"],
    )
    .unwrap_or_default();
    for line in now.lines() {
        if let Some((path, count)) = line.rsplit_once(':')
            && let Ok(count) = count.trim().parse::<u32>()
        {
            let folder = top_folder(path);
            by_folder
                .entry(folder.clone())
                .or_insert_with(|| OpenLoops {
                    folder,
                    ..Default::default()
                })
                .now += count;
        }
    }
    let week_ago_rev = git(repo, &["rev-list", "-1", "--before=7 days ago", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_owned();
    if !week_ago_rev.is_empty() {
        let then = git(
            repo,
            &["grep", "-c", "-e", "⏳", "-e", "⏰", &week_ago_rev, "--", "*.md"],
        )
        .unwrap_or_default();
        for line in then.lines() {
            let Some(rest) = line.strip_prefix(&week_ago_rev) else {
                continue;
            };
            let rest = rest.strip_prefix(':').unwrap_or(rest);
            if let Some((path, count)) = rest.rsplit_once(':')
                && let Ok(count) = count.trim().parse::<u32>()
            {
                let folder = top_folder(path);
                by_folder
                    .entry(folder.clone())
                    .or_insert_with(|| OpenLoops {
                        folder,
                        ..Default::default()
                    })
                    .week_ago += count;
            }
        }
    }
    Ok(by_folder.into_values().collect())
}

pub fn momentum_label(theme: &Theme) -> String {
    if theme.is_new {
        "new".to_owned()
    } else if let Some(momentum) = theme.momentum {
        format!("x{momentum:.1}")
    } else {
        "quiet".to_owned()
    }
}

fn render_generated(signals: &Signals) -> String {
    let theme_by_id: HashMap<&str, &Theme> = signals
        .themes
        .iter()
        .map(|theme| (theme.id.as_str(), theme))
        .collect();
    let mut out = String::new();
    out.push_str(&format!(
        "> **Status {}:** signals pass ran at {} on commit `{}` over {} commits in {:.1}s; {} themes, {} threads.\n\n",
        &signals.generated_at[..10],
        &signals.generated_at[11..],
        signals.commit,
        signals.commits,
        signals.run_ms as f64 / 1000.0,
        signals.themes.iter().filter(|t| !t.hidden).count(),
        signals.threads.len(),
    ));
    let table = |ids: &[String], out: &mut String| {
        out.push_str("| Theme | 12 weeks | Momentum | Folders |\n|---|---|---|---|\n");
        for id in ids {
            let Some(theme) = theme_by_id.get(id.as_str()) else {
                continue;
            };
            let folders = theme
                .folders
                .iter()
                .map(|folder| folder.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "| {} | `{}` | {} | {} |\n",
                theme.name,
                sparkline(&theme.series),
                momentum_label(theme),
                folders
            ));
        }
        if ids.is_empty() {
            out.push_str("| (none) | | | |\n");
        }
    };
    out.push_str("## Rising\n\n");
    table(&signals.rising, &mut out);
    out.push_str("\n## New\n\n");
    table(&signals.fresh, &mut out);
    out.push_str("\n## Fading\n\n");
    table(&signals.fading, &mut out);
    out.push_str("\n## Threads\n\n");
    if signals.threads.is_empty() {
        out.push_str("No theme spans three or more folders yet.\n");
    }
    for thread in &signals.threads {
        let name = theme_by_id
            .get(thread.theme.as_str())
            .map(|theme| theme.name.as_str())
            .unwrap_or(thread.theme.as_str());
        out.push_str(&format!("- **{}** .. {}.", name, thread.folders.join(", ")));
        let gaps: Vec<String> = thread
            .links
            .iter()
            .filter(|link| !link.linked)
            .map(|link| format!("`{}` and `{}` do not link", link.from_file, link.to_file))
            .collect();
        if gaps.is_empty() {
            out.push_str(" All folders link.\n");
        } else {
            out.push_str(&format!(" Gaps: {}.\n", gaps.join("; ")));
        }
    }
    out.push_str("\n## Open loops\n\n| Folder | Now | 7 days ago | Change |\n|---|---|---|---|\n");
    for loops in &signals.open_loops {
        let delta = loops.now as i64 - loops.week_ago as i64;
        out.push_str(&format!(
            "| {} | {} | {} | {:+} |\n",
            loops.folder, loops.now, loops.week_ago, delta
        ));
    }
    out.push_str("\n## Pinned\n\n");
    let pinned: Vec<String> = signals
        .themes
        .iter()
        .filter(|theme| theme.pinned && !theme.hidden)
        .map(|theme| theme.id.clone())
        .collect();
    table(&pinned, &mut out);
    out
}

fn write_themes_md(repo: &Path, config: &BrainConfig, signals: &Signals) -> Result<()> {
    let path = config.themes_path(repo, THEMES_NAME);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let narrative = existing
        .split_once(NARRATIVE_START)
        .and_then(|(_, rest)| rest.split_once(NARRATIVE_END))
        .map(|(narrative, _)| narrative.trim().to_owned())
        .unwrap_or_default();
    let narrative = if narrative.is_empty() {
        "_No narrative yet. Paste `grokbot-prompt.md` into Grokbot to write one._".to_owned()
    } else {
        narrative
    };
    let text = format!(
        "# Themes\n\nWhat the brain has been about, computed from git history by Brainz. The \
         generated block is overwritten by every signals pass and is never hand-edited; the \
         narrative block is written by Grokbot from `grokbot-prompt.md`. Curation lives in \
         `pins.md`. See `CLAUDE.md` in this folder.\n\n{GENERATED_START}\n{}\n{GENERATED_END}\n\n\
         {NARRATIVE_START}\n{narrative}\n{NARRATIVE_END}\n",
        render_generated(signals).trim_end()
    );
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub fn load_signals(repo: &Path, config: &BrainConfig) -> Result<Signals> {
    let path = config.themes_path(repo, SIGNALS_NAME);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// The Grokbot-written block of `themes.md`, if any.
pub fn load_narrative(repo: &Path, config: &BrainConfig) -> Option<String> {
    let text = std::fs::read_to_string(config.themes_path(repo, THEMES_NAME)).ok()?;
    let (_, rest) = text.split_once(NARRATIVE_START)?;
    let (narrative, _) = rest.split_once(NARRATIVE_END)?;
    let narrative = narrative.trim();
    (!narrative.is_empty()).then(|| narrative.to_owned())
}

/// Appends one curation line to `pins.md` (creating the file if needed).
pub fn append_pin(repo: &Path, config: &BrainConfig, line: &str) -> Result<()> {
    let path = config.themes_path(repo, PINS_NAME);
    let mut text = if path.exists() {
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?
    } else {
        std::fs::create_dir_all(repo.join(config.themes_dir.trim_end_matches('/')))?;
        default_pins()
    };
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line.trim());
    text.push('\n');
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Drops a `- pin: x` line so unpinning is symmetrical with pinning.
pub fn remove_pin_line(repo: &Path, config: &BrainConfig, line: &str) -> Result<()> {
    let path = config.themes_path(repo, PINS_NAME);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let wanted = line.trim();
    let kept: Vec<&str> = text.lines().filter(|l| l.trim() != wanted).collect();
    let mut output = kept.join("\n");
    output.push('\n');
    std::fs::write(&path, output).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub fn signals_age(repo: &Path, config: &BrainConfig) -> Option<std::time::Duration> {
    let modified = std::fs::metadata(config.themes_path(repo, SIGNALS_NAME))
        .ok()?
        .modified()
        .ok()?;
    modified.elapsed().ok()
}

pub fn themes_md_modified(repo: &Path, config: &BrainConfig) -> Option<std::time::SystemTime> {
    std::fs::metadata(config.themes_path(repo, THEMES_NAME))
        .ok()?
        .modified()
        .ok()
}

pub fn repo_path(repo: &Path, relative: &str) -> PathBuf {
    repo.join(relative)
}

/// `interviews/companies/tekmetric/CLAUDE.md` → `tekmetric / CLAUDE`.
pub fn file_label(path: &str) -> String {
    let mut parts = path.rsplit('/');
    let file = parts.next().unwrap_or(path);
    let file = file.strip_suffix(".md").unwrap_or(file);
    match parts.next() {
        Some(parent) => format!("{parent} / {file}"),
        None => file.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> Vocabulary {
        Vocabulary::from_tree(
            &[
                "network/rich-turing.md".into(),
                "network/CLAUDE.md".into(),
                "interviews/companies/tekmetric/notes.md".into(),
                "interviews/companies/archive/old.md".into(),
                "muckstack/projects/Course-and-Cloth/CLAUDE.md".into(),
            ],
            &["PLS".into(), "MCP".into()],
            &BrainConfig::default(),
        )
    }

    #[test]
    fn extracts_bold_wiki_vocabulary_and_capitalized_runs() {
        let terms = extract_terms(
            "**Cold Start Problem** with [[tekmetric/roster|the roster]] and pls for Alan Turing at Acme Inc.",
            &vocab(),
        );
        let keys: Vec<&str> = terms.iter().map(|t| t.key.as_str()).collect();
        assert!(keys.contains(&"cold start problem"), "{keys:?}");
        assert!(keys.contains(&"roster"), "{keys:?}");
        assert!(keys.contains(&"pls"), "{keys:?}");
        assert!(keys.contains(&"rich turing"), "{keys:?}");
        assert!(keys.contains(&"acme inc"), "{keys:?}");
        assert!(keys.contains(&"tekmetric"), "{keys:?}");
        assert!(!keys.contains(&"archive"));
    }

    #[test]
    fn stoplist_drops_noise_words() {
        let terms = extract_terms(
            "Status Next Tommy Keeley met The Team on Monday Morning; **Read** this.",
            &vocab(),
        );
        let keys: Vec<&str> = terms.iter().map(|t| t.key.as_str()).collect();
        assert!(!keys.iter().any(|k| k.contains("tommy")), "{keys:?}");
        assert!(!keys.iter().any(|k| k.contains("status")), "{keys:?}");
        assert!(!keys.iter().any(|k| k.contains("monday")), "{keys:?}");
        assert!(!keys.contains(&"read"), "{keys:?}");
    }

    #[test]
    fn dates_times_identifiers_and_sentences_are_not_terms() {
        let terms = extract_terms(
            "**Fri 2026-09-25 1:00pm ET** and **calories_kcal** and **calendar personal wins over toni email where they disagree** but **Cold Start**",
            &vocab(),
        );
        let keys: Vec<&str> = terms.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(keys, vec!["cold start"], "{keys:?}");
    }

    #[test]
    fn vocabulary_matches_whole_words_only() {
        let terms = extract_terms("please pulse the mcps and mcp server", &vocab());
        let keys: Vec<&str> = terms.iter().map(|t| t.key.as_str()).collect();
        assert!(!keys.contains(&"pls"), "{keys:?}");
        assert!(keys.contains(&"mcp"), "{keys:?}");
    }

    #[test]
    fn parses_pins_forms_and_ignores_comments() {
        let pins = parse_pins(
            "# Pins\n<!-- - pin: ignored -->\n- pin: Tekmetric\n- rename: pls => product-led sales\n- merge: roofer, Acme Inc => acme\n- hide: read\n- bogus: x\n",
        );
        assert_eq!(pins.pin, vec!["tekmetric"]);
        assert_eq!(
            pins.rename,
            vec![("pls".to_owned(), "product-led sales".to_owned())]
        );
        assert_eq!(
            pins.merge,
            vec![(vec!["roofer".to_owned(), "acme inc".to_owned()], "acme".to_owned())]
        );
        assert_eq!(pins.hide, vec!["read"]);
        assert_eq!(pins.canonical("pls").0, "product-led sales");
        assert_eq!(pins.canonical("roofer").0, "acme");
        assert_eq!(pins.canonical("other").0, "other");
    }

    #[test]
    fn momentum_compares_last_two_weeks_to_prior_six() {
        let series = [0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 3, 3];
        let (m, recent, prior) = momentum(&series);
        assert_eq!(recent, 6);
        assert_eq!(prior, 6);
        assert!((m.unwrap() - 3.0).abs() < 1e-9);
        let quiet = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4];
        assert_eq!(momentum(&quiet).0, None);
        let fading = [0, 0, 0, 0, 4, 4, 4, 4, 4, 4, 0, 1];
        assert!(momentum(&fading).0.unwrap() < FADING_MAX);
    }

    #[test]
    fn sparkline_scales_to_series_max() {
        assert_eq!(sparkline(&[0, 4, 8]), "▁▄█");
        assert_eq!(sparkline(&[0, 0]), "▁▁");
    }

    #[test]
    fn exclusions_cover_themes_reports_and_dated_health_files() {
        let config = BrainConfig::default();
        assert!(excluded("ops/themes/signals.json", &config));
        assert!(excluded(".claude/librarian-reports/2026-09-21.md", &config));
        assert!(excluded("health/2026-09-21.md", &config));
        assert!(excluded("health/whoop/sleep-2026-09-21.md", &config));
        assert!(!excluded("health/rolling-summary.md", &config));
        assert!(!excluded("ops/desk/TODO.md", &config));
        let custom = BrainConfig {
            themes_dir: "meta/themes".into(),
            exclude_prefixes: vec!["archive".into()],
            dated_exclude_dirs: vec![],
            ..BrainConfig::default()
        };
        assert!(excluded("meta/themes/signals.json", &custom));
        assert!(excluded("archive/old.md", &custom));
        assert!(!excluded("health/2026-09-21.md", &custom));
    }

    #[test]
    fn week_labels_and_recent_window() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
        assert_eq!(week_label(date), "2026-W40");
        let weeks = recent_weeks(date);
        assert_eq!(weeks.len(), SERIES_WEEKS);
        assert_eq!(weeks.last().unwrap(), "2026-W40");
        assert_eq!(weeks.first().unwrap(), "2026-W29");
    }
}

#[cfg(test)]
mod real_brain {
    use super::*;

    /// Runs the pass on the real brain checkout. Writes `ops/themes/` there.
    /// `cargo test -p brainz_calendar real_brain -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn run_on_tommy_brain() {
        let repo = PathBuf::from(std::env::var("HOME").unwrap()).join("tommy-brain");
        let signals = run_pass(&repo).unwrap();
        println!(
            "commit {} commits {} run {}ms themes {}",
            signals.commit,
            signals.commits,
            signals.run_ms,
            signals.themes.len()
        );
        for theme in signals.themes.iter().take(40) {
            println!(
                "{:<32} total {:>4} weeks {:>2} {} {:>6} folders {:?}",
                theme.name,
                theme.total,
                theme.weeks_active,
                sparkline(&theme.series),
                momentum_label(theme),
                theme.folders.iter().map(|f| f.name.as_str()).collect::<Vec<_>>()
            );
        }
        println!("RISING {:?}", signals.rising);
        println!("FADING {:?}", signals.fading);
        for t in &signals.threads {
            println!("THREAD {} {:?} gaps {}", t.theme, t.folders, t.links.iter().filter(|l| !l.linked).count());
        }
        println!("LOOPS {:?}", signals.open_loops.iter().map(|l| (l.folder.as_str(), l.now, l.week_ago)).collect::<Vec<_>>());
        for want in ["tekmetric", "pls", "product-led sales"] {
            if let Some(t) = signals.themes.iter().find(|t| t.id == want) {
                println!("CHECK {want}: {:?}", t.series);
            } else {
                println!("CHECK {want}: not a theme");
            }
        }
    }
}
