//! Brainz: per-brain layout, read from an optional `brainz.toml` at the
//! workspace root, so any notes repo works without touching the code.
//!
//! ```toml
//! # brainz.toml at the brain's root; every key is optional
//! todo = "TODO.md"
//! themes_dir = "themes"
//! people_dir = "people"
//! places_dir = "places"
//! vocabulary_folders = ["projects", "companies"]
//! exclude_prefixes = ["reports"]
//! dated_exclude_dirs = ["health"]
//! stop_words = ["yourname"]   # extra terms the themes pass never counts
//! sync = true                 # false hides the Sync to GitHub banner
//!
//! [calendar]
//! prep_lead_minutes = 10      # the prep banner appears this long before an event
//! match_dirs = ["companies", "projects"]   # folders whose children are matched
//!
//! [themes]
//! noise_dirs = ["itinerary", "roster", "logistics", "prompt"]
//! window_weeks = 12
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const CONFIG_FILE: &str = "brainz.toml";

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct BrainConfig {
    /// The check-off board the To-Do tab edits.
    pub todo: String,
    /// Where the themes pass writes `signals.json`, `themes.md`, `pins.md`,
    /// and `vocabulary.md`.
    pub themes_dir: String,
    /// One Markdown file per person; the stems become the people vocabulary
    /// and the People chips in the Themes tab open them.
    pub people_dir: String,
    pub places_dir: String,
    /// Folders whose child folder names become vocabulary (companies,
    /// projects, clients).
    pub vocabulary_folders: Vec<String>,
    /// Path prefixes the themes pass ignores entirely.
    pub exclude_prefixes: Vec<String>,
    /// Folders where files named with a date (daily syncs) are ignored.
    pub dated_exclude_dirs: Vec<String>,
    /// Extra words the themes pass never counts (your own name, say), on
    /// top of the built-in stoplist.
    pub stop_words: Vec<String>,
    /// Whether the Sync to GitHub banner is offered at all.
    pub sync: bool,
    /// The calendar-aware prep and capture banner.
    pub calendar: CalendarConfig,
    /// Knobs for the themes pass.
    pub themes: ThemesConfig,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct CalendarConfig {
    /// Minutes before an event starts that the prep banner appears.
    pub prep_lead_minutes: u32,
    /// Folders whose child folders are matched against event titles,
    /// attendee names, and email domains. Empty means `vocabulary_folders`.
    pub match_dirs: Vec<String>,
}

impl Default for CalendarConfig {
    fn default() -> Self {
        Self {
            prep_lead_minutes: 10,
            match_dirs: vec![],
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct ThemesConfig {
    /// A term whose only source files sit under one of these folder names
    /// (or in a file of that name) is not a theme.
    pub noise_dirs: Vec<String>,
    /// How many ISO weeks the trend line and momentum cover.
    pub window_weeks: usize,
}

impl Default for ThemesConfig {
    fn default() -> Self {
        Self {
            noise_dirs: ["itinerary", "roster", "logistics", "prompt"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            window_weeks: 12,
        }
    }
}

impl Default for BrainConfig {
    fn default() -> Self {
        Self {
            todo: "TODO.md".into(),
            themes_dir: "themes".into(),
            people_dir: "people".into(),
            places_dir: "places".into(),
            vocabulary_folders: vec!["projects".into(), "companies".into()],
            exclude_prefixes: vec![],
            dated_exclude_dirs: vec![],
            stop_words: vec![],
            sync: true,
            calendar: CalendarConfig::default(),
            themes: ThemesConfig::default(),
        }
    }
}

impl BrainConfig {
    pub fn load(repo: &Path) -> Self {
        let path = repo.join(CONFIG_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str::<Self>(&text) {
            Ok(config) => config,
            Err(error) => {
                log::error!("ignoring {}: {error}", path.display());
                Self::default()
            }
        }
    }

    pub fn themes_file(&self, name: &str) -> String {
        format!("{}/{name}", self.themes_dir.trim_end_matches('/'))
    }

    pub fn themes_path(&self, repo: &Path, name: &str) -> PathBuf {
        repo.join(self.themes_dir.trim_end_matches('/')).join(name)
    }

    /// `network/ada-lovelace.md` for "Ada Lovelace".
    pub fn person_file(&self, name: &str) -> String {
        let stem = crate::themes_signals::normalize_key(name).replace(' ', "-");
        format!("{}/{stem}.md", self.people_dir.trim_end_matches('/'))
    }

    /// The folders whose children are companies, clients, and projects for
    /// calendar and screenshot matching.
    pub fn match_dirs(&self) -> Vec<String> {
        let dirs = if self.calendar.match_dirs.is_empty() {
            &self.vocabulary_folders
        } else {
            &self.calendar.match_dirs
        };
        dirs.iter()
            .map(|dir| dir.trim_matches('/').to_owned())
            .filter(|dir| !dir.is_empty())
            .collect()
    }

    /// The window the trend lines cover, never shorter than the eight weeks
    /// momentum needs.
    pub fn window_weeks(&self) -> usize {
        self.themes.window_weeks.max(8)
    }

    /// Who "owes" the ⏰ loops in summaries: the first configured stop word
    /// (usually the brain owner's first name), or "you".
    pub fn owner_label(&self) -> String {
        self.stop_words
            .first()
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => "you".to_owned(),
                }
            })
            .unwrap_or_else(|| "you".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_keeps_defaults_for_missing_keys() {
        let config: BrainConfig = toml::from_str("todo = \"TODO.md\"\nsync = false\n").unwrap();
        assert_eq!(config.todo, "TODO.md");
        assert!(!config.sync);
        assert_eq!(config.themes_dir, "themes");
        assert_eq!(config.people_dir, "people");
        assert_eq!(config.calendar.prep_lead_minutes, 10);
        assert_eq!(config.themes.window_weeks, 12);
        assert_eq!(config.match_dirs(), config.vocabulary_folders);
    }

    #[test]
    fn nested_tables_parse() {
        let config: BrainConfig = toml::from_str(
            "stop_words = [\"ada\"]\n[calendar]\nprep_lead_minutes = 15\nmatch_dirs = [\"clients/\"]\n[themes]\nwindow_weeks = 4\nnoise_dirs = [\"drafts\"]\n",
        )
        .unwrap();
        assert_eq!(config.calendar.prep_lead_minutes, 15);
        assert_eq!(config.match_dirs(), vec!["clients".to_owned()]);
        assert_eq!(config.themes.noise_dirs, vec!["drafts".to_owned()]);
        assert_eq!(config.window_weeks(), 8);
        assert_eq!(config.owner_label(), "Ada");
        assert_eq!(BrainConfig::default().owner_label(), "you");
    }

    #[test]
    fn missing_file_means_defaults() {
        let config = BrainConfig::load(Path::new("/definitely/not/a/brain"));
        assert_eq!(config, BrainConfig::default());
    }
}
