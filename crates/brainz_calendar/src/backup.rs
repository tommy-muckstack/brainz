//! Brainz: keeps a second copy of a brain's notes outside GitHub, in an
//! Obsidian vault folder or in Notion. The copy is one way: Brainz writes it
//! after every successful sync (and on "Back up now") and never reads it back.
//!
//! Obsidian gets the Markdown files as they are, folders included. Only files
//! Brainz wrote are updated or removed, so the rest of the vault is left
//! alone. Notion gets a private "Brainz backup" page with one page per folder
//! and one page per note, written through a personal access token.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write as _,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow};
use gpui::{App, AppContext as _};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use util::ResultExt as _;

use crate::github_sync::{self, SyncStatus};

const NOTION_VERSION: &str = "2026-03-11";
const NOTION_API: &str = "https://api.notion.com/v1";
/// Notion allows about three requests a second per connection.
const NOTION_PAUSE: Duration = Duration::from_millis(340);
pub const NOTION_TOKEN_URL: &str = "https://www.notion.so/developers/tokens";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    #[default]
    Off,
    Obsidian,
    Notion,
}

/// What Brainz last wrote for one note.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct BackedUp {
    hash: String,
    /// The Notion page holding the note; none for Obsidian.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    page: Option<String>,
}

/// One brain's backup settings and what its last run wrote.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrainBackup {
    pub target: Target,
    /// The folder notes are copied into; the default when unset.
    pub obsidian_vault: Option<PathBuf>,
    /// The private Notion page the backup lives under, and its link.
    notion_root: Option<String>,
    pub notion_url: Option<String>,
    /// Folder path ("" for none) → its Notion page.
    folders: BTreeMap<String, String>,
    /// Note path relative to the brain → what was written for it.
    notes: BTreeMap<String, BackedUp>,
    pub last_run: Option<chrono::DateTime<chrono::Utc>>,
    pub last_count: usize,
    pub last_error: Option<String>,
}

impl BrainBackup {
    /// The vault folder in use: the chosen one, else Obsidian's iCloud
    /// folder when it exists (so iCloud keeps a copy too), else Documents.
    pub fn vault(&self, brain: &Path) -> PathBuf {
        if let Some(vault) = &self.obsidian_vault {
            return vault.clone();
        }
        let name = brain
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Brainz".into());
        let icloud =
            paths::home_dir().join("Library/Mobile Documents/iCloud~md~obsidian/Documents");
        if icloud.is_dir() {
            icloud.join(name)
        } else {
            paths::home_dir().join("Documents/Obsidian").join(name)
        }
    }

    /// Forgets what was written, so the next run starts the copy afresh.
    fn forget_copy(&mut self) {
        self.notion_root = None;
        self.notion_url = None;
        self.folders.clear();
        self.notes.clear();
        self.last_count = 0;
        self.last_error = None;
    }
}

fn settings_path() -> PathBuf {
    paths::data_dir().join("notes-backup.json")
}

fn brain_key(brain: &Path) -> String {
    brain.to_string_lossy().into_owned()
}

fn load_all() -> BTreeMap<String, BrainBackup> {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Settings and the last run for one brain.
pub fn settings(brain: &Path) -> BrainBackup {
    load_all().remove(&brain_key(brain)).unwrap_or_default()
}

fn store(brain: &Path, backup: &BrainBackup) -> Result<()> {
    let _guard = SETTINGS_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut all = load_all();
    all.insert(brain_key(brain), backup.clone());
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
    file.write_all(serde_json::to_string_pretty(&all)?.as_bytes())?;
    file.persist(&path)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

static SETTINGS_LOCK: Mutex<()> = Mutex::new(());
/// Brains with a backup running, so a sync and a click never overlap.
static RUNNING: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

pub fn is_running(brain: &Path) -> bool {
    RUNNING
        .lock()
        .map(|running| running.contains(brain))
        .unwrap_or(false)
}

/// Chooses where the brain's notes go. A new target starts a fresh copy.
pub fn set_target(brain: &Path, target: Target, vault: Option<PathBuf>) -> Result<()> {
    let mut backup = settings(brain);
    if let Some(vault) = &vault {
        check_vault(brain, vault)?;
    }
    if backup.target != target || (vault.is_some() && vault != backup.obsidian_vault) {
        backup.forget_copy();
    }
    backup.target = target;
    if vault.is_some() {
        backup.obsidian_vault = vault;
    }
    store(brain, &backup)
}

fn check_vault(brain: &Path, vault: &Path) -> Result<()> {
    anyhow::ensure!(
        !vault.starts_with(brain) && !brain.starts_with(vault),
        "Pick a folder outside the brain; a copy inside it would be synced back to GitHub"
    );
    Ok(())
}

/// Copies the brain's notes to the chosen target. Blocking; run it on the
/// background executor. Returns how many notes the copy holds.
pub fn run(brain: &Path) -> Result<usize> {
    {
        let mut running = RUNNING.lock().unwrap_or_else(|error| error.into_inner());
        anyhow::ensure!(
            running.insert(brain.to_path_buf()),
            "A backup is already running"
        );
    }
    let result = run_locked(brain);
    RUNNING
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(brain);
    result
}

fn run_locked(brain: &Path) -> Result<usize> {
    let mut backup = settings(brain);
    let notes = list_notes(brain)?;
    let result = match backup.target {
        Target::Off => return Ok(0),
        Target::Obsidian => copy_to_vault(brain, &notes, &mut backup),
        Target::Notion => {
            let token = saved_notion_token().context("Paste a Notion token first")?;
            let notion = Notion { token };
            copy_to_notion(brain, &notes, &mut backup, &notion)
        }
    };
    backup.last_run = Some(chrono::Utc::now());
    match &result {
        Ok(failed) if failed.is_empty() => backup.last_error = None,
        Ok(failed) => {
            backup.last_error = Some(format!(
                "{} {} couldn't be backed up: {}",
                failed.len(),
                if failed.len() == 1 { "note" } else { "notes" },
                failed.join(", ")
            ))
        }
        Err(error) => backup.last_error = Some(format!("{error:#}")),
    }
    backup.last_count = backup.notes.len();
    store(brain, &backup)?;
    result.map(|_| backup.last_count)
}

/// Markdown notes in the brain, tracked or new but not ignored, skipping
/// dot-folders such as .claude.
#[allow(
    clippy::disallowed_methods,
    reason = "backups run on the background executor"
)]
pub(crate) fn list_notes(brain: &Path) -> Result<Vec<String>> {
    let output = github_sync::command(brain, "git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .context("listing the brain's notes")?;
    anyhow::ensure!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(notes_from_ls_files(&output.stdout))
}

fn notes_from_ls_files(output: &[u8]) -> Vec<String> {
    let mut notes: Vec<String> = output
        .split(|byte| *byte == 0)
        .filter_map(|path| std::str::from_utf8(path).ok())
        .filter(|path| {
            let lower = path.to_ascii_lowercase();
            (lower.ends_with(".md") || lower.ends_with(".markdown"))
                && !path.split('/').any(|part| part.starts_with('.'))
        })
        .map(str::to_owned)
        .collect();
    notes.sort();
    notes.dedup();
    notes
}

/// FNV-1a: stable across builds, which `DefaultHasher` does not promise.
fn content_hash(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Returns the notes that could not be copied.
fn copy_to_vault(brain: &Path, notes: &[String], backup: &mut BrainBackup) -> Result<Vec<String>> {
    let vault = backup.vault(brain);
    check_vault(brain, &vault)?;
    std::fs::create_dir_all(&vault).with_context(|| format!("creating {}", vault.display()))?;
    let mut failed = Vec::new();
    for note in notes {
        let Ok(bytes) = std::fs::read(brain.join(note)) else {
            failed.push(note.clone());
            continue;
        };
        let hash = content_hash(&bytes);
        let destination = vault.join(note);
        if backup.notes.get(note).is_some_and(|done| done.hash == hash) && destination.is_file() {
            continue;
        }
        let written = destination
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&destination, &bytes));
        match written {
            Ok(()) => {
                backup
                    .notes
                    .insert(note.clone(), BackedUp { hash, page: None });
            }
            Err(error) => {
                log::warn!("brainz backup: copying {note}: {error}");
                failed.push(note.clone());
            }
        }
    }
    let current: BTreeSet<&String> = notes.iter().collect();
    let gone: Vec<String> = backup
        .notes
        .keys()
        .filter(|note| !current.contains(note))
        .cloned()
        .collect();
    for note in gone {
        let done = backup.notes.remove(&note).unwrap_or_default();
        let copy = vault.join(&note);
        // A copy someone edited in Obsidian is theirs now; keep it.
        if std::fs::read(&copy).is_ok_and(|bytes| content_hash(&bytes) == done.hash) {
            std::fs::remove_file(&copy).log_err();
        }
    }
    Ok(failed)
}

fn notion_token_path() -> PathBuf {
    paths::config_dir().join("notion-backup-token")
}

pub fn saved_notion_token() -> Option<String> {
    std::fs::read_to_string(notion_token_path())
        .ok()
        .map(|token| token.trim().to_owned())
        .filter(|token| !token.is_empty())
}

/// Stores the token readable only by this user, checking it with Notion
/// first so a bad paste fails here rather than at the next sync.
pub fn save_notion_token(token: &str) -> Result<()> {
    let token = token.trim();
    anyhow::ensure!(!token.is_empty(), "Paste the token first");
    Notion {
        token: token.to_owned(),
    }
    .request("GET", "/users/me", None)
    .context("Notion didn't accept that token")?;
    let path = notion_token_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("writing {}", path.display()))?;
    writeln!(file, "{token}")?;
    Ok(())
}

pub fn forget_notion_token() -> Result<()> {
    match std::fs::remove_file(notion_token_path()) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

struct Notion {
    token: String,
}

#[derive(Debug)]
struct NotionError {
    status: u16,
    message: String,
}

impl std::fmt::Display for NotionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Notion said {} ({})", self.message, self.status)
    }
}

impl std::error::Error for NotionError {}

impl Notion {
    /// One API call through curl, retrying when Notion asks us to slow down.
    /// The token goes in on stdin so it never shows in the process list.
    #[allow(
        clippy::disallowed_methods,
        reason = "backups run on the background executor"
    )]
    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut body_file = tempfile::NamedTempFile::new()?;
            if let Some(body) = body {
                body_file.write_all(serde_json::to_string(body)?.as_bytes())?;
            }
            let mut command = std::process::Command::new("/usr/bin/curl");
            command.args([
                "-sS",
                "--max-time",
                "120",
                "-X",
                method,
                "-H",
                "@-",
                "-H",
                "content-type: application/json",
                "-H",
            ]);
            command.arg(format!("notion-version: {NOTION_VERSION}"));
            if body.is_some() {
                command.arg("--data-binary");
                command.arg(format!("@{}", body_file.path().display()));
            }
            command.args(["-w", "\n%{http_code}"]);
            command.arg(format!("{NOTION_API}{path}"));
            command
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            let mut child = command.spawn().context("running curl")?;
            if let Some(mut stdin) = child.stdin.take() {
                writeln!(stdin, "authorization: Bearer {}", self.token)?;
            }
            let output = child.wait_with_output()?;
            std::thread::sleep(NOTION_PAUSE);
            anyhow::ensure!(
                output.status.success(),
                "couldn't reach Notion: {}",
                String::from_utf8_lossy(&output.stderr)
                    .trim()
                    .trim_start_matches("curl: ")
            );
            let (status, value) = parse_response(&String::from_utf8_lossy(&output.stdout));
            if (200..300).contains(&status) {
                return Ok(value);
            }
            if (status == 429 || status >= 500) && attempt < 6 {
                std::thread::sleep(Duration::from_secs(attempt));
                continue;
            }
            return Err(NotionError {
                status,
                message: value["message"]
                    .as_str()
                    .unwrap_or("something unexpected")
                    .to_owned(),
            }
            .into());
        }
    }

    fn create_page(
        &self,
        parent: Option<&str>,
        title: &str,
        markdown: Option<&str>,
    ) -> Result<Value> {
        let mut body = json!({
            "parent": match parent {
                Some(page) => json!({"type": "page_id", "page_id": page}),
                None => json!({"type": "workspace", "workspace": true}),
            },
            "properties": {"title": {"title": [{"text": {"content": title}}]}},
        });
        if let Some(markdown) = markdown {
            body["markdown"] = json!(markdown);
        }
        self.request("POST", "/pages", Some(&body))
    }

    fn page_is_live(&self, page: &str) -> Result<bool> {
        match self.request("GET", &format!("/pages/{page}"), None) {
            Ok(value) => Ok(!value["in_trash"].as_bool().unwrap_or(false)
                && !value["archived"].as_bool().unwrap_or(false)),
            Err(error) if is_missing(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

/// curl prints the body, then the status code on its own line (`-w`).
fn parse_response(text: &str) -> (u16, Value) {
    let (json, status) = text.rsplit_once('\n').unwrap_or(("", text));
    (
        status.trim().parse().unwrap_or(0),
        serde_json::from_str(json).unwrap_or(Value::Null),
    )
}

fn is_missing(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<NotionError>()
        .is_some_and(|error| error.status == 404)
}

/// Notion has no front matter; keep it visible as a YAML block.
fn notion_markdown(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let body = if let Some(rest) = text.strip_prefix("---\n")
        && let Some(end) = rest.find("\n---\n")
    {
        format!("```yaml\n{}\n```\n\n{}", &rest[..end], &rest[end + 5..])
    } else {
        text.to_owned()
    };
    if body.trim().is_empty() {
        " ".to_owned()
    } else {
        body
    }
}

fn note_title(note: &str) -> String {
    Path::new(note)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| note.to_owned())
}

fn folder_of(note: &str) -> &str {
    note.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// Returns the notes that could not be copied.
fn copy_to_notion(
    brain: &Path,
    notes: &[String],
    backup: &mut BrainBackup,
    notion: &Notion,
) -> Result<Vec<String>> {
    let root_live = match &backup.notion_root {
        Some(root) => notion.page_is_live(root)?,
        None => false,
    };
    if !root_live {
        backup.forget_copy();
        let name = brain
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "brain".into());
        let page = notion.create_page(None, &format!("Brainz backup · {name}"), None)?;
        backup.notion_root = Some(
            page["id"]
                .as_str()
                .ok_or_else(|| anyhow!("Notion returned a page without an id"))?
                .to_owned(),
        );
        backup.notion_url = page["url"].as_str().map(str::to_owned);
        store(brain, backup)?;
    }
    let root = backup.notion_root.clone().unwrap_or_default();
    let mut failed = Vec::new();
    for (index, note) in notes.iter().enumerate() {
        match copy_note_to_notion(brain, note, &root, backup, notion) {
            Ok(()) => {}
            Err(error) => {
                log::warn!("brainz backup: {note} to Notion: {error:#}");
                failed.push(note.clone());
            }
        }
        // Keep progress, so a quit halfway does not redo the whole brain.
        if index % 20 == 19 {
            store(brain, backup).log_err();
        }
    }
    let current: BTreeSet<&String> = notes.iter().collect();
    let gone: Vec<String> = backup
        .notes
        .keys()
        .filter(|note| !current.contains(note))
        .cloned()
        .collect();
    for note in gone {
        if let Some(page) = backup.notes.get(&note).and_then(|done| done.page.clone()) {
            match notion.request(
                "PATCH",
                &format!("/pages/{page}"),
                Some(&json!({"in_trash": true})),
            ) {
                Ok(_) => {}
                Err(error) if is_missing(&error) => {}
                Err(error) => {
                    log::warn!("brainz backup: removing {note} from Notion: {error:#}");
                    continue;
                }
            }
        }
        backup.notes.remove(&note);
    }
    Ok(failed)
}

fn copy_note_to_notion(
    brain: &Path,
    note: &str,
    root: &str,
    backup: &mut BrainBackup,
    notion: &Notion,
) -> Result<()> {
    let text = std::fs::read_to_string(brain.join(note))?;
    let hash = content_hash(text.as_bytes());
    let previous = backup.notes.get(note).cloned();
    if previous.as_ref().is_some_and(|done| done.hash == hash) {
        return Ok(());
    }
    let markdown = notion_markdown(&text);
    if let Some(page) = previous.and_then(|done| done.page) {
        let body = json!({
            "type": "replace_content",
            "replace_content": {"new_str": markdown, "allow_deleting_content": true},
        });
        match notion.request("PATCH", &format!("/pages/{page}/markdown"), Some(&body)) {
            Ok(_) => {
                backup.notes.insert(
                    note.to_owned(),
                    BackedUp {
                        hash,
                        page: Some(page),
                    },
                );
                return Ok(());
            }
            // Someone deleted the page in Notion; write it again below.
            Err(error) if is_missing(&error) => {}
            Err(error) => return Err(error),
        }
    }
    let parent = folder_page(folder_of(note), root, backup, notion)?;
    let page = notion.create_page(Some(&parent), &note_title(note), Some(&markdown))?;
    let id = page["id"]
        .as_str()
        .ok_or_else(|| anyhow!("Notion returned a page without an id"))?
        .to_owned();
    backup.notes.insert(
        note.to_owned(),
        BackedUp {
            hash,
            page: Some(id),
        },
    );
    Ok(())
}

/// The Notion page standing for a folder, made (with its parents) on first use.
fn folder_page(
    folder: &str,
    root: &str,
    backup: &mut BrainBackup,
    notion: &Notion,
) -> Result<String> {
    if folder.is_empty() {
        return Ok(root.to_owned());
    }
    if let Some(page) = backup.folders.get(folder) {
        return Ok(page.clone());
    }
    let parent = folder_page(folder_of(folder), root, backup, notion)?;
    let name = folder.rsplit('/').next().unwrap_or(folder);
    let page = notion.create_page(Some(&parent), name, None)?;
    let id = page["id"]
        .as_str()
        .ok_or_else(|| anyhow!("Notion returned a page without an id"))?
        .to_owned();
    backup.folders.insert(folder.to_owned(), id.clone());
    Ok(id)
}

/// Backs up after every sync that finishes, so the copy follows GitHub.
pub fn init(cx: &mut App) {
    let Some(sync) = github_sync::state(cx) else {
        return;
    };
    let mut was_syncing = false;
    cx.observe(&sync, move |sync, cx| {
        let state = sync.read(cx);
        let syncing = matches!(state.status(), SyncStatus::Syncing(_));
        let finished = was_syncing && *state.status() == SyncStatus::Clean;
        was_syncing = syncing;
        let Some(brain) = state.repo().cloned() else {
            return;
        };
        if !finished || settings(&brain).target == Target::Off {
            return;
        }
        cx.background_spawn(async move {
            if let Err(error) = run(&brain) {
                log::warn!("brainz backup after sync: {error:#}");
            }
        })
        .detach();
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_skip_dot_folders_and_other_files() {
        let output = b"README.md\0.claude/memory/a.md\0people/sam.md\0image.png\0notes/Plan.MARKDOWN\0people/sam.md\0";
        assert_eq!(
            notes_from_ls_files(output),
            vec!["README.md", "notes/Plan.MARKDOWN", "people/sam.md"]
        );
    }

    #[test]
    fn front_matter_becomes_a_yaml_block() {
        assert_eq!(
            notion_markdown("---\ntitle: Plan\n---\n# Plan\n"),
            "```yaml\ntitle: Plan\n```\n\n# Plan\n"
        );
        assert_eq!(notion_markdown("# Plan\n"), "# Plan\n");
        assert_eq!(notion_markdown("\n"), " ");
    }

    #[test]
    fn folders_and_titles_come_from_the_path() {
        assert_eq!(folder_of("a/b/note.md"), "a/b");
        assert_eq!(folder_of("note.md"), "");
        assert_eq!(note_title("a/b/Sam Lee.md"), "Sam Lee");
    }

    #[test]
    fn responses_split_into_status_and_body() {
        let (status, body) = parse_response(
            "{\"object\":\"error\",\"status\":401,\"message\":\"API token is invalid.\"}\n401",
        );
        assert_eq!(status, 401);
        assert_eq!(body["message"], "API token is invalid.");
        assert_eq!(parse_response("\n204"), (204, Value::Null));
        assert_eq!(parse_response("").0, 0);
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(content_hash(b""), "cbf29ce484222325");
        assert_eq!(content_hash(b"a"), "af63dc4c8601ec8c");
    }

    #[test]
    fn vault_must_sit_outside_the_brain() {
        let brain = Path::new("/Users/me/brain");
        assert!(check_vault(brain, Path::new("/Users/me/brain/backup")).is_err());
        assert!(check_vault(brain, Path::new("/Users/me")).is_err());
        assert!(check_vault(brain, Path::new("/Users/me/Obsidian/brain")).is_ok());
    }

    #[test]
    fn vault_copy_updates_changed_notes_and_keeps_edited_copies() {
        let brain = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(brain.path().join("people")).unwrap();
        std::fs::write(brain.path().join("people/sam.md"), "# Sam\n").unwrap();
        std::fs::write(brain.path().join("plan.md"), "# Plan\n").unwrap();
        std::fs::write(vault.path().join("mine.md"), "not from Brainz").unwrap();
        let mut backup = BrainBackup {
            target: Target::Obsidian,
            obsidian_vault: Some(vault.path().to_path_buf()),
            ..Default::default()
        };
        let notes = vec!["people/sam.md".to_owned(), "plan.md".to_owned()];
        assert!(
            copy_to_vault(brain.path(), &notes, &mut backup)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            std::fs::read_to_string(vault.path().join("people/sam.md")).unwrap(),
            "# Sam\n"
        );

        // Sam changes; plan is deleted from the brain, its copy untouched.
        std::fs::write(brain.path().join("people/sam.md"), "# Sam Lee\n").unwrap();
        copy_to_vault(brain.path(), &notes[..1], &mut backup).unwrap();
        assert_eq!(
            std::fs::read_to_string(vault.path().join("people/sam.md")).unwrap(),
            "# Sam Lee\n"
        );
        assert!(!vault.path().join("plan.md").exists());
        assert!(vault.path().join("mine.md").exists());

        // A copy edited in Obsidian survives its note's deletion.
        std::fs::write(vault.path().join("people/sam.md"), "edited in Obsidian").unwrap();
        copy_to_vault(brain.path(), &[], &mut backup).unwrap();
        assert!(vault.path().join("people/sam.md").exists());
        assert!(backup.notes.is_empty());
    }
}
