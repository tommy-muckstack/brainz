//! Brainz: the My Man brain. My Man (screenshots, dictation, meeting
//! recordings, notes) exports everything it captures as Markdown into one
//! folder, `~/MyManBrain` by default, with a `catalog.json` allowlist and a
//! bundled MCP companion. Brainz reads that folder for the Brief, the prep
//! banner, and the To-Do tab, feeds the brain's proper nouns into My Man's
//! dictation vocabulary, and keeps the agent credential My Man issues so
//! the connectors can act on the app.
//!
//! Sync is one-way app → folder; Brainz only ever appends to
//! `vocabulary.md`, which My Man treats as the user's own list.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::brain_config::BrainConfig;

/// Folder name under the home directory when `brainz.toml` says nothing.
pub const DEFAULT_ROOT: &str = "MyManBrain";
/// Env var My Man's companion servers read for the folder.
pub const ROOT_ENV: &str = "MYMAN_BRAIN_ROOT";
pub const TOKEN_ENV: &str = "MYMAN_AGENT_TOKEN";
pub const MACHINE_ENV: &str = "MYMAN_MACHINE_ID";
/// Name of the read connector in the hosted catalog.
pub const READ_CONNECTOR: &str = "myman";
/// Name of the app-actions connector in the hosted catalog.
pub const ACTIONS_CONNECTOR: &str = "myman-actions";
/// My Man caps the user vocabulary at this many lines.
const VOCABULARY_CAP: usize = 150;
/// Tag every board item moved from My Man carries.
pub const FROM_MYMAN_TAG: &str = "from-myman";

/// One recorded meeting, from `catalog.json` plus the note's front matter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meeting {
    pub id: String,
    /// `meetings/2026-10-02-D255EB5E.md`, relative to the root.
    pub path: String,
    pub title: String,
    pub started: DateTime<Local>,
    pub ended: Option<DateTime<Local>>,
    pub participants: Vec<String>,
    pub complete: bool,
    pub low_content: bool,
}

impl Meeting {
    pub fn minutes(&self) -> Option<i64> {
        self.ended
            .map(|ended| (ended - self.started).num_minutes().max(0))
    }

    /// The file-name stem a brain note would cite, `2026-10-02-D255EB5E`.
    pub fn stem(&self) -> &str {
        self.path
            .rsplit('/')
            .next()
            .unwrap_or(&self.path)
            .trim_end_matches(".md")
    }

    /// Participants other than the brain's owner, for row labels.
    pub fn others(&self, owner: &str) -> Vec<String> {
        let owner = owner.to_lowercase();
        self.participants
            .iter()
            .filter(|name| {
                let lower = name.to_lowercase();
                let is_owner = !owner.is_empty()
                    && lower
                        .split(|c: char| !c.is_alphanumeric())
                        .any(|word| word == owner);
                !lower.starts_with("speaker") && !is_owner
            })
            .cloned()
            .collect()
    }
}

/// One open item from My Man's `tasks.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MyManTask {
    pub title: String,
    /// `meeting`, `note`, `dictation`: where My Man extracted it.
    pub source: String,
    pub date: Option<NaiveDate>,
}

impl MyManTask {
    /// What the handled list remembers.
    pub fn key(&self) -> String {
        format!(
            "{}|{}",
            self.title.to_lowercase(),
            self.date.map(|d| d.to_string()).unwrap_or_default()
        )
    }

    pub fn board_line(&self) -> String {
        let when = self.date.map(|date| format!(" {date}")).unwrap_or_default();
        format!(
            "- [ ] {} (from My Man {}{when}) `{FROM_MYMAN_TAG}`",
            self.title.trim_end_matches(['.', ' ']),
            self.source
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MyMan {
    pub root: PathBuf,
}

impl MyMan {
    /// The configured or default folder, only when it is really there.
    pub fn detect(config: &BrainConfig) -> Option<Self> {
        let root = match &config.myman.root {
            Some(root) => expand_home(root),
            None => util::paths::home_dir().join(DEFAULT_ROOT),
        };
        root.join("catalog.json").is_file().then_some(Self { root })
    }

    /// The folder for the brain open at `workspace_root`, or the default
    /// one when no brain is open, for the connectors.
    pub fn detect_for(workspace_root: Option<&Path>) -> Option<Self> {
        let config = workspace_root.map(BrainConfig::load).unwrap_or_default();
        Self::detect(&config)
    }

    /// Whether this My Man ships the bundled MCP companion the connectors
    /// launch (1.1.9x and later).
    pub fn has_companion(&self) -> bool {
        self.root.join("tools/server.mjs").is_file()
    }

    pub fn join(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Every recorded meeting the catalog lists, newest first.
    pub fn meetings(&self) -> Result<Vec<Meeting>> {
        let path = self.root.join("catalog.json");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let catalog: Catalog =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let mut meetings = Vec::new();
        for entry in catalog.exports.into_iter().filter(|e| e.kind == "meetings") {
            let note = std::fs::read_to_string(self.root.join(&entry.path)).unwrap_or_default();
            let front = front_matter(&note);
            let started = front
                .get("started")
                .and_then(|value| parse_time(value))
                .or_else(|| parse_time(&entry.timestamp));
            let Some(started) = started else {
                continue;
            };
            meetings.push(Meeting {
                id: entry.item_id,
                path: entry.path,
                title: entry.title.trim().to_owned(),
                started,
                ended: front.get("ended").and_then(|value| parse_time(value)),
                participants: list_values(&note, "participants"),
                complete: front
                    .get("status")
                    .is_none_or(|status| status == "complete"),
                low_content: front
                    .get("low_content")
                    .is_some_and(|value| value == "true"),
            });
        }
        meetings.sort_by_key(|meeting| std::cmp::Reverse(meeting.started));
        Ok(meetings)
    }

    /// Meetings from the last `days` days that no note in the brain cites
    /// yet (by file-name stem, tracked or not), newest first.
    pub fn unlogged_meetings(&self, brain: &Path, days: i64, now: DateTime<Local>) -> Vec<Meeting> {
        let since = now - chrono::Duration::days(days);
        let Ok(meetings) = self.meetings() else {
            return Vec::new();
        };
        let recent: Vec<Meeting> = meetings
            .into_iter()
            .filter(|meeting| meeting.complete && !meeting.low_content)
            .filter(|meeting| meeting.started >= since && meeting.started <= now)
            .filter(|meeting| meeting.minutes().is_none_or(|minutes| minutes >= 5))
            .collect();
        let stems: Vec<&str> = recent.iter().map(Meeting::stem).collect();
        let cited = brain_cites(brain, &stems);
        recent
            .into_iter()
            .filter(|meeting| !cited.contains(meeting.stem()))
            .collect()
    }

    /// The most recent completed recording that looks like an earlier
    /// instance of this event: same title, or a full attendee name among
    /// the recording's participants, or the matched person in its title.
    pub fn last_meeting_for(
        &self,
        title: &str,
        attendee_names: &[String],
        who: Option<&str>,
        before: DateTime<Local>,
    ) -> Option<Meeting> {
        let meetings = self.meetings().ok()?;
        let wanted_title = normalize(title);
        let names: Vec<String> = attendee_names
            .iter()
            .map(String::as_str)
            .chain(who)
            .map(normalize)
            .filter(|name| name.split(' ').count() >= 2)
            .collect();
        meetings
            .into_iter()
            .filter(|meeting| meeting.complete && !meeting.low_content)
            .filter(|meeting| meeting.started < before)
            .find(|meeting| {
                let meeting_title = normalize(&meeting.title);
                if !wanted_title.is_empty() && meeting_title == wanted_title {
                    return true;
                }
                let participants: Vec<String> =
                    meeting.participants.iter().map(|p| normalize(p)).collect();
                names.iter().any(|name| {
                    participants.iter().any(|p| p == name) || meeting_title.contains(name.as_str())
                })
            })
    }

    /// Unchecked items in `tasks.md`.
    pub fn open_tasks(&self) -> Vec<MyManTask> {
        let text = std::fs::read_to_string(self.root.join("tasks.md")).unwrap_or_default();
        parse_tasks(&text)
    }

    /// Appends brain terms My Man does not know yet to its dictation
    /// vocabulary. Terms Brainz added before and the user then removed are
    /// left out for good. Returns how many were added.
    pub fn sync_vocabulary(&self, terms: &[String]) -> Result<usize> {
        self.sync_vocabulary_recording_at(terms, &vocabulary_record_path())
    }

    fn sync_vocabulary_recording_at(&self, terms: &[String], record_path: &Path) -> Result<usize> {
        let path = self.root.join("vocabulary.md");
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let present: HashSet<String> = existing
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_lowercase)
            .collect();
        let mut record: VocabularyRecord = std::fs::read_to_string(record_path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let room = VOCABULARY_CAP.saturating_sub(present.len());
        let mut added = Vec::new();
        for term in terms {
            if added.len() >= room {
                break;
            }
            let key = term.trim().to_lowercase();
            if key.is_empty()
                || present.contains(&key)
                || record.added.iter().any(|known| known.to_lowercase() == key)
                || added
                    .iter()
                    .any(|known: &String| known.to_lowercase() == key)
            {
                continue;
            }
            added.push(term.trim().to_owned());
        }
        if added.is_empty() {
            return Ok(0);
        }
        let mut output = if existing.trim().is_empty() {
            "# Vocabulary\n".to_owned()
        } else {
            existing
        };
        if !output.ends_with('\n') {
            output.push('\n');
        }
        for term in &added {
            output.push_str(term);
            output.push('\n');
        }
        std::fs::write(&path, output).with_context(|| format!("writing {}", path.display()))?;
        record.added.extend(added.iter().cloned());
        if let Some(parent) = record_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(record_path, serde_json::to_string_pretty(&record)?)
            .with_context(|| format!("writing {}", record_path.display()))?;
        Ok(added.len())
    }
}

/// After a themes pass: push the brain's proper nouns into My Man's
/// dictation vocabulary when a My Man folder is configured.
pub fn feed_vocabulary(repo: &Path) {
    let config = BrainConfig::load(repo);
    let Some(myman) = MyMan::detect(&config) else {
        return;
    };
    match crate::themes_signals::dictation_vocabulary(repo)
        .and_then(|terms| myman.sync_vocabulary(&terms))
    {
        Ok(0) => {}
        Ok(added) => log::info!(
            "brainz myman: added {added} brain terms to {}",
            myman.root.join("vocabulary.md").display()
        ),
        Err(error) => log::warn!("brainz myman: vocabulary feed failed: {error:#}"),
    }
}

#[derive(Deserialize)]
struct Catalog {
    #[serde(default)]
    exports: Vec<CatalogEntry>,
}

#[derive(Deserialize)]
struct CatalogEntry {
    #[serde(default)]
    item_id: String,
    kind: String,
    path: String,
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    title: String,
}

#[derive(Serialize, Deserialize, Default)]
struct VocabularyRecord {
    added: Vec<String>,
}

fn vocabulary_record_path() -> PathBuf {
    paths::data_dir().join("myman-vocabulary.json")
}

/// Which My Man tasks the To-Do tab already moved or dismissed.
#[derive(Serialize, Deserialize, Default)]
pub struct HandledTasks {
    pub keys: Vec<String>,
}

fn handled_tasks_path() -> PathBuf {
    paths::data_dir().join("myman-tasks.json")
}

pub fn handled_tasks() -> HandledTasks {
    std::fs::read_to_string(handled_tasks_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn mark_task_handled(task: &MyManTask) -> Result<()> {
    let mut handled = handled_tasks();
    let key = task.key();
    if !handled.keys.contains(&key) {
        handled.keys.push(key);
    }
    let path = handled_tasks_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&handled)?)
        .with_context(|| format!("writing {}", path.display()))
}

/// Open My Man tasks the board has not taken yet.
pub fn pending_tasks(myman: &MyMan) -> Vec<MyManTask> {
    let handled = handled_tasks();
    myman
        .open_tasks()
        .into_iter()
        .filter(|task| !handled.keys.contains(&task.key()))
        .collect()
}

/// `- [ ] Share my screen — Ada  <!-- meeting, 2026-09-25 -->` lines above
/// the "## Done" heading.
pub fn parse_tasks(text: &str) -> Vec<MyManTask> {
    let mut tasks = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ")
            && heading.trim().eq_ignore_ascii_case("done")
        {
            break;
        }
        let Some(rest) = trimmed.strip_prefix("- [ ] ") else {
            continue;
        };
        let (body, comment) = match rest.split_once("<!--") {
            Some((body, comment)) => (body, comment.trim_end_matches("-->").trim()),
            None => (rest, ""),
        };
        let mut body = body.trim();
        // The assignee suffix My Man adds ("— Ada"); a dash inside the task
        // text itself ("Follow up - send the deck") is left alone.
        for dash in [" — ", " – ", " - "] {
            if let Some((head, tail)) = body.rsplit_once(dash) {
                if !head.trim().is_empty() && looks_like_assignee(tail) {
                    body = head.trim();
                }
                break;
            }
        }
        if body.is_empty() {
            continue;
        }
        let mut parts = comment.split(',').map(str::trim);
        let source = parts.next().unwrap_or("").to_owned();
        let date = parts
            .next()
            .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok());
        tasks.push(MyManTask {
            title: body.to_owned(),
            source: if source.is_empty() {
                "task".to_owned()
            } else {
                source
            },
            date,
        });
    }
    tasks
}

/// One or two capitalised words, no digits: a person, not task text.
fn looks_like_assignee(tail: &str) -> bool {
    let words: Vec<&str> = tail.split_whitespace().collect();
    (1..=2).contains(&words.len())
        && tail.chars().count() <= 30
        && words.iter().all(|word| {
            word.chars().next().is_some_and(char::is_uppercase)
                && word
                    .chars()
                    .all(|c| c.is_alphabetic() || c == '\'' || c == '-')
        })
}

/// Which of the meeting stems some file in the brain mentions, in one git
/// grep. Tracked and untracked files both count, so a note written minutes
/// ago already clears its recording.
fn brain_cites(brain: &Path, stems: &[&str]) -> HashSet<String> {
    use std::io::Write as _;
    let wanted: Vec<&str> = stems
        .iter()
        .copied()
        .filter(|stem| stem.len() >= 8)
        .collect();
    if wanted.is_empty() {
        return HashSet::new();
    }
    #[allow(clippy::disallowed_methods, reason = "runs on the background executor")]
    let child = std::process::Command::new("git")
        .current_dir(brain)
        .args([
            "grep",
            "-I",
            "-o",
            "-h",
            "-F",
            "--untracked",
            "-f",
            "-",
            "--",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            log::debug!("brainz myman: git grep failed to start: {error}");
            return HashSet::new();
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let patterns = wanted.join("\n") + "\n";
        if let Err(error) = stdin.write_all(patterns.as_bytes()) {
            log::debug!("brainz myman: git grep stdin: {error}");
        }
    }
    #[allow(clippy::disallowed_methods, reason = "runs on the background executor")]
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => {
            log::debug!("brainz myman: git grep failed: {error}");
            return HashSet::new();
        }
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect()
}

/// Serialises the To-Do tab's read-modify-write of the board and the
/// handled list, so two quick clicks cannot lose each other's write.
pub static TASK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `key: value` pairs of the YAML front matter, scalars only.
fn front_matter(note: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut lines = note.lines();
    if lines.next().map(str::trim) != Some("---") {
        return map;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim().trim_matches('"').to_owned();
            map.insert(key.trim().to_owned(), value);
        }
    }
    map
}

/// The `- item` lines under a front-matter list key.
fn list_values(note: &str, key: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut inside = false;
    let mut in_front = false;
    for (ix, line) in note.lines().enumerate() {
        if ix == 0 {
            in_front = line.trim() == "---";
            continue;
        }
        if !in_front {
            break;
        }
        if line.trim() == "---" {
            break;
        }
        if inside {
            if let Some(item) = line.trim().strip_prefix("- ") {
                values.push(item.trim().trim_matches('"').to_owned());
                continue;
            }
            if line.starts_with(' ') {
                continue;
            }
            inside = false;
        }
        if let Some(rest) = line.strip_prefix(key)
            && let Some(rest) = rest.strip_prefix(':')
        {
            let rest = rest.trim();
            if let Some(inline) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                values.extend(
                    inline
                        .split(',')
                        .map(|item| item.trim().trim_matches('"').to_owned())
                        .filter(|item| !item.is_empty()),
                );
            } else if rest.is_empty() {
                inside = true;
            }
        }
    }
    values
}

fn parse_time(value: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|time| time.with_timezone(&Local))
}

fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        util::paths::home_dir().join(rest)
    } else if path == "~" {
        util::paths::home_dir().to_path_buf()
    } else {
        PathBuf::from(path)
    }
}

/// The machine id My Man's agent registry holds, read from its own
/// Application Support folder so the user never has to copy it.
pub fn machine_id() -> Option<String> {
    let path = util::paths::home_dir()
        .join("Library/Application Support/MyMan/AgentIdentity/identities.json");
    let text = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("machineID")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
}

/// The env the companion servers need: folder, machine id, and the agent
/// credential when one has been saved.
pub fn companion_env(root: &Path) -> Vec<(String, String)> {
    let mut env = vec![(ROOT_ENV.to_owned(), root.to_string_lossy().into_owned())];
    if let Some(machine) = machine_id() {
        env.push((MACHINE_ENV.to_owned(), machine));
    }
    if let Some(token) = saved_token() {
        env.push((TOKEN_ENV.to_owned(), token));
    }
    env
}

fn token_path() -> PathBuf {
    paths::config_dir().join("myman-agent-token")
}

/// The credential My Man issues for Brainz itself at launch (1.1.108+),
/// in a file only this login can read; no paste needed.
fn issued_credential_path() -> PathBuf {
    util::paths::home_dir().join("Library/Application Support/MyMan/AgentIdentity/brainz.env")
}

/// The credential: the one My Man issued for Brainz when it has, else the
/// one pasted from My Man's Settings → Agents, if any.
pub fn saved_token() -> Option<String> {
    let issued = std::fs::read_to_string(issued_credential_path())
        .ok()
        .and_then(|text| token_from_env_file(&text));
    issued.or_else(|| {
        std::fs::read_to_string(token_path())
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
    })
}

/// `MYMAN_AGENT_TOKEN=…` out of a `KEY=VALUE` file.
fn token_from_env_file(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| line.trim().split_once('='))
        .find(|(key, _)| key.trim() == TOKEN_ENV)
        .map(|(_, value)| value.trim().trim_matches('"').to_owned())
        .filter(|value| !value.is_empty())
}

/// Stores the credential, owner-readable only.
pub fn save_token(token: &str) -> Result<()> {
    let token = token.trim();
    anyhow::ensure!(!token.is_empty(), "paste the credential first");
    let path = token_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, format!("{token}\n"))
        .with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("restricting {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "---\nid: D255EB5E-CAC6-437F-9C95-781E515D9E9D\nstarted: 2026-10-03T00:38:59.258Z\nended: 2026-10-03T01:17:39.338Z\nstatus: complete\nflagged_hotwords: []\nparticipants:\n  - Speaker 2\n  - Ada Lovelace\nkind: meeting\nscreenshots: [\"screenshots/a.md\"]\n---\n\n# Ada / Grace\n";

    fn fixture() -> (tempfile::TempDir, MyMan) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("meetings")).unwrap();
        std::fs::write(root.join("meetings/2026-10-02-D255EB5E.md"), NOTE).unwrap();
        std::fs::write(
            root.join("meetings/2026-09-02-25061560.md"),
            "---\nstarted: 2026-09-02T15:52:42.673Z\nended: 2026-09-02T16:30:00.000Z\nstatus: complete\nparticipants:\n  - Grace Hopper\n---\n# Interview\n",
        )
        .unwrap();
        std::fs::write(
            root.join("catalog.json"),
            r#"{"exports":[
              {"item_id":"meeting-1","kind":"meetings","path":"meetings/2026-10-02-D255EB5E.md","timestamp":"2026-10-03T00:38:59.258Z","title":"Ada / Grace"},
              {"item_id":"meeting-2","kind":"meetings","path":"meetings/2026-09-02-25061560.md","timestamp":"2026-09-02T15:52:42.673Z","title":"Acme Interview: Grace"},
              {"item_id":"note-1","kind":"notes","path":"notes/x.md","timestamp":"2026-09-02T15:52:42.673Z","title":"x"}
            ]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("tasks.md"),
            "# Tasks\n\n- [ ] Share my screen — Ada  <!-- meeting, 2026-09-25 -->\n- [ ] find rate  <!-- meeting, 2026-09-25 -->\n\n## Done\n\n- [x] Finalize H2 Strategy  <!-- meeting, 2026-09-19 -->\n",
        )
        .unwrap();
        let myman = MyMan {
            root: root.to_path_buf(),
        };
        (dir, myman)
    }

    #[test]
    fn meetings_read_the_catalog_and_front_matter() {
        let (_dir, myman) = fixture();
        let meetings = myman.meetings().unwrap();
        assert_eq!(meetings.len(), 2);
        let newest = &meetings[0];
        assert_eq!(newest.title, "Ada / Grace");
        assert_eq!(newest.stem(), "2026-10-02-D255EB5E");
        assert_eq!(newest.participants, vec!["Speaker 2", "Ada Lovelace"]);
        assert_eq!(newest.minutes(), Some(38));
        assert!(newest.complete && !newest.low_content);
        assert_eq!(newest.others("ada"), Vec::<String>::new());
        let mixed = Meeting {
            participants: vec!["Adam Smith".into(), "Ada Lovelace".into()],
            ..newest.clone()
        };
        assert_eq!(mixed.others("ada"), vec!["Adam Smith".to_owned()]);
    }

    #[test]
    fn last_meeting_matches_title_or_attendee() {
        let (_dir, myman) = fixture();
        let now = Local::now();
        let by_title = myman
            .last_meeting_for("ada / grace", &[], None, now)
            .unwrap();
        assert_eq!(by_title.stem(), "2026-10-02-D255EB5E");
        let by_name = myman
            .last_meeting_for("Sync", &["Grace Hopper".to_owned()], None, now)
            .unwrap();
        assert_eq!(by_name.stem(), "2026-09-02-25061560");
        assert!(
            myman
                .last_meeting_for("Dentist", &["Grace".to_owned()], None, now)
                .is_none()
        );
    }

    #[test]
    fn tasks_drop_the_assignee_and_stop_at_done() {
        let (_dir, myman) = fixture();
        let tasks = myman.open_tasks();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].title, "Share my screen");
        let dashed = parse_tasks(
            "- [ ] Follow up - send the deck — Ada  <!-- note, 2026-09-25 -->\n- [ ] Ship v2 - beta  <!-- meeting -->\n",
        );
        assert_eq!(dashed[0].title, "Follow up - send the deck");
        assert_eq!(dashed[1].title, "Ship v2 - beta");
        assert_eq!(tasks[0].source, "meeting");
        assert_eq!(tasks[0].date, NaiveDate::from_ymd_opt(2026, 9, 25));
        assert_eq!(
            tasks[1].board_line(),
            "- [ ] find rate (from My Man meeting 2026-09-25) `from-myman`"
        );
    }

    #[test]
    fn unlogged_meetings_skip_ones_the_brain_cites() {
        let (_dir, myman) = fixture();
        let brain = tempfile::tempdir().unwrap();
        #[allow(clippy::disallowed_methods, reason = "test setup")]
        fn run(brain: &Path, args: &[&str]) {
            let output = std::process::Command::new("git")
                .current_dir(brain)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
        }
        run(brain.path(), &["init", "-q"]);
        std::fs::write(
            brain.path().join("call.md"),
            "Transcript: ~/MyManBrain/meetings/2026-10-02-D255EB5E.md\n",
        )
        .unwrap();
        let now = Local.with_ymd_and_hms(2026, 10, 4, 9, 0, 0).unwrap();
        let unlogged = myman.unlogged_meetings(brain.path(), 60, now);
        assert_eq!(unlogged.len(), 1);
        assert_eq!(unlogged[0].stem(), "2026-09-02-25061560");
    }

    #[test]
    fn vocabulary_appends_only_new_terms_and_never_re_adds_removed_ones() {
        let (_dir, myman) = fixture();
        let vocabulary = myman.root.join("vocabulary.md");
        let record = myman.root.join("record.json");
        std::fs::write(&vocabulary, "# Vocabulary\nAcme\n").unwrap();
        let terms = vec![
            "acme".to_owned(),
            "Grace Hopper".to_owned(),
            "Globex".to_owned(),
        ];
        let added = myman.sync_vocabulary_recording_at(&terms, &record).unwrap();
        assert_eq!(added, 2);
        assert_eq!(
            std::fs::read_to_string(&vocabulary).unwrap(),
            "# Vocabulary\nAcme\nGrace Hopper\nGlobex\n"
        );
        // The user deletes Globex in My Man; the next feed leaves it out.
        std::fs::write(&vocabulary, "# Vocabulary\nAcme\nGrace Hopper\n").unwrap();
        let added = myman.sync_vocabulary_recording_at(&terms, &record).unwrap();
        assert_eq!(added, 0);
        assert_eq!(
            std::fs::read_to_string(&vocabulary).unwrap(),
            "# Vocabulary\nAcme\nGrace Hopper\n"
        );
    }

    #[test]
    fn issued_credential_file_yields_the_token() {
        assert_eq!(
            token_from_env_file("MYMAN_AGENT_TOKEN=abc123==\nMYMAN_MACHINE_ID=X\n").as_deref(),
            Some("abc123==")
        );
        assert!(token_from_env_file("MYMAN_MACHINE_ID=X\n").is_none());
        assert!(token_from_env_file("MYMAN_AGENT_TOKEN=\n").is_none());
    }

    use chrono::TimeZone as _;
}
