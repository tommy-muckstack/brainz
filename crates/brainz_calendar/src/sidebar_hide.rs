//! Brainz: hide items from the Notes tree on this Mac. Hiding is a view
//! preference, not part of the brain, so it lives in the app's data folder
//! and never reaches git. A hidden folder hides everything inside it.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct Store {
    /// Whether every hidden item is shown again (the "show all" toggle).
    #[serde(default)]
    show_all: bool,
    /// Hidden paths, relative to each brain's root folder.
    #[serde(default)]
    hidden: BTreeMap<String, BTreeSet<String>>,
}

fn store_path() -> PathBuf {
    paths::data_dir().join("sidebar-hidden.json")
}

fn load() -> Store {
    std::fs::read_to_string(store_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(store: &Store) {
    let path = store_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(store) {
        Ok(text) => {
            if let Err(error) = std::fs::write(&path, text) {
                log::warn!("could not save sidebar visibility: {error}");
            }
        }
        Err(error) => log::warn!("could not encode sidebar visibility: {error}"),
    }
}

fn root_key(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

/// Whether the "show all" toggle is on, which reveals hidden items.
pub fn show_all() -> bool {
    load().show_all
}

pub fn set_show_all(show_all: bool) {
    let mut store = load();
    store.show_all = show_all;
    save(&store);
}

/// The paths hidden in the brain at `root`, relative to that root.
pub fn hidden_in(root: &Path) -> BTreeSet<String> {
    load().hidden.remove(&root_key(root)).unwrap_or_default()
}

/// Hides `relative` (a file or folder) in the brain at `root`.
pub fn hide(root: &Path, relative: &str) {
    let mut store = load();
    store
        .hidden
        .entry(root_key(root))
        .or_default()
        .insert(relative.to_string());
    save(&store);
}

/// Shows an item hidden by `hide`, and everything that was hidden under it.
pub fn unhide(root: &Path, relative: &str) {
    let mut store = load();
    if let Some(set) = store.hidden.get_mut(&root_key(root)) {
        set.retain(|path| path != relative && !path.starts_with(&format!("{relative}/")));
    }
    save(&store);
}

/// Whether `relative` is one of `hidden` or sits inside a hidden folder.
pub fn is_hidden(hidden: &BTreeSet<String>, relative: &str) -> bool {
    hidden.iter().any(|path| {
        relative == path
            || relative
                .strip_prefix(path.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiding_a_folder_hides_what_is_inside_it() {
        let hidden: BTreeSet<String> = ["projects".to_string(), "START-HERE.md".to_string()].into();
        assert!(is_hidden(&hidden, "START-HERE.md"));
        assert!(is_hidden(&hidden, "projects"));
        assert!(is_hidden(&hidden, "projects/brainz/notes.md"));
        assert!(!is_hidden(&hidden, "projects-archive/notes.md"));
        assert!(!is_hidden(&hidden, "CLAUDE.md"));
    }
}
