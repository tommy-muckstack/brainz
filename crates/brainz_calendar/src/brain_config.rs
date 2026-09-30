//! Brainz: per-brain layout, read from an optional `brainz.toml` at the
//! workspace root, so another brain (a work machine, a different folder
//! structure, no GitHub) works without touching the code. Every field
//! defaults to the tommy-brain layout.
//!
//! ```toml
//! # brainz.toml at the brain's root; every key is optional
//! todo = "ops/desk/TODO.md"
//! themes_dir = "ops/themes"
//! people_dir = "network"
//! vocabulary_folders = ["interviews/companies", "muckstack/projects"]
//! exclude_prefixes = [".claude/librarian-reports"]
//! dated_exclude_dirs = ["health"]
//! sync = true   # false hides the Sync to GitHub banner entirely
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
    /// Folders whose child folder names become vocabulary (companies,
    /// projects, clients).
    pub vocabulary_folders: Vec<String>,
    /// Path prefixes the themes pass ignores entirely.
    pub exclude_prefixes: Vec<String>,
    /// Folders where files named with a date (daily syncs) are ignored.
    pub dated_exclude_dirs: Vec<String>,
    /// Whether the Sync to GitHub banner is offered at all.
    pub sync: bool,
}

impl Default for BrainConfig {
    fn default() -> Self {
        Self {
            todo: "ops/desk/TODO.md".into(),
            themes_dir: "ops/themes".into(),
            people_dir: "network".into(),
            vocabulary_folders: vec![
                "interviews/companies".into(),
                "muckstack/projects".into(),
                "muckstack/advisory/companies".into(),
            ],
            exclude_prefixes: vec![".claude/librarian-reports".into()],
            dated_exclude_dirs: vec!["health".into()],
            sync: true,
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

    /// `network/rich-turing.md` for "Alan Turing".
    pub fn person_file(&self, name: &str) -> String {
        let stem = crate::themes_signals::normalize_key(name).replace(' ', "-");
        format!("{}/{stem}.md", self.people_dir.trim_end_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_keeps_defaults_for_missing_keys() {
        let config: BrainConfig =
            toml::from_str("todo = \"TODO.md\"\nsync = false\n").unwrap();
        assert_eq!(config.todo, "TODO.md");
        assert!(!config.sync);
        assert_eq!(config.themes_dir, "ops/themes");
        assert_eq!(config.people_dir, "network");
    }

    #[test]
    fn missing_file_means_defaults() {
        let config = BrainConfig::load(Path::new("/definitely/not/a/brain"));
        assert_eq!(config, BrainConfig::default());
    }
}
