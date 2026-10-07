//! Brainz: one memory for the brain. Brainz's Claude keeps its config in
//! `~/.config/brainz/claude`, so memory it writes never reaches a terminal
//! `claude` session and vice versa. On first launch after this change the
//! two memory folders are merged into the brain at `.claude/memory/` and
//! both original locations become symlinks to it. Credentials stay apart.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use gpui::{App, AppContext as _, WeakEntity};
use workspace::Workspace;

const TARGET: &str = ".claude/memory";
const INDEX: &str = "MEMORY.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Both locations already point at the brain.
    AlreadyShared,
    /// Nothing to share: neither location has a memory folder yet.
    Nothing,
    Linked {
        files: usize,
        backups: Vec<PathBuf>,
    },
}

/// Claude Code's folder name for a project: the absolute path with every
/// non-alphanumeric character replaced by `-`.
pub fn project_slug(brain: &Path) -> String {
    brain
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub fn memory_dirs(brain: &Path, home: &Path, brainz_claude_dir: &Path) -> [PathBuf; 2] {
    let slug = project_slug(brain);
    [
        home.join(".claude/projects").join(&slug).join("memory"),
        brainz_claude_dir
            .join("projects")
            .join(&slug)
            .join("memory"),
    ]
}

fn points_to(link: &Path, target: &Path) -> bool {
    match std::fs::read_link(link) {
        Ok(found) => {
            let resolved = if found.is_absolute() {
                found
            } else {
                link.parent().map(|p| p.join(&found)).unwrap_or(found)
            };
            resolved
                .canonicalize()
                .ok()
                .zip(target.canonicalize().ok())
                .is_some_and(|(a, b)| a == b)
        }
        Err(_) => false,
    }
}

fn memory_files(dir: &Path) -> Result<Vec<(String, PathBuf, std::time::SystemTime)>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let modified = entry.metadata()?.modified()?;
        files.push((name.to_owned(), path, modified));
    }
    Ok(files)
}

/// Union of two `MEMORY.md` indexes: every line of the first, then the
/// lines of the second it does not already have.
pub fn merge_index(first: &str, second: &str) -> String {
    let mut lines: Vec<&str> = first.lines().collect();
    for line in second.lines() {
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// Merges the memory folders into `<brain>/.claude/memory` and replaces
/// both with symlinks. Returns an error, having changed nothing, whenever
/// the layout is not one of the expected shapes.
pub fn ensure_shared(brain: &Path, home: &Path, brainz_claude_dir: &Path) -> Result<Outcome> {
    let target = brain.join(TARGET);
    let sources = memory_dirs(brain, home, brainz_claude_dir);
    let linked = sources.iter().filter(|s| points_to(s, &target)).count();
    if linked == 2 {
        return Ok(Outcome::AlreadyShared);
    }
    let mut real_dirs = Vec::new();
    for source in &sources {
        match std::fs::symlink_metadata(source) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if !points_to(source, &target) {
                    bail!(
                        "{} is a symlink that does not point at {}; leaving it alone",
                        source.display(),
                        target.display()
                    );
                }
            }
            Ok(metadata) if metadata.is_dir() => real_dirs.push(source.clone()),
            Ok(_) => bail!("{} is a file, not a folder", source.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("checking {}", source.display()));
            }
        }
    }
    let target_has_files = std::fs::symlink_metadata(&target)
        .map(|m| {
            m.is_dir()
                && memory_files(&target)
                    .map(|f| !f.is_empty())
                    .unwrap_or(false)
        })
        .unwrap_or(false);
    if real_dirs.is_empty() {
        if linked == 1 || target_has_files {
            // One side already shared, or the brain arrived with memory of
            // its own (a clone on another Mac): link whatever is missing.
            for source in &sources {
                if !points_to(source, &target) {
                    link(source, &target)?;
                }
            }
            return Ok(Outcome::Linked {
                files: memory_files(&target).map(|f| f.len()).unwrap_or(0),
                backups: Vec::new(),
            });
        }
        return Ok(Outcome::Nothing);
    }

    // Newest file wins on a name collision; the index is unioned. Memory
    // already in the brain takes part like any other folder, so a brain
    // that came with memory and a Mac that has some of its own merge too.
    let mut chosen: BTreeMap<String, (PathBuf, std::time::SystemTime)> = BTreeMap::new();
    let mut indexes: Vec<String> = Vec::new();
    let mut ordered = real_dirs.clone();
    if target_has_files {
        ordered.insert(0, target.clone());
    }
    for dir in &ordered {
        for (name, path, modified) in memory_files(dir)? {
            if name == INDEX {
                indexes.push(std::fs::read_to_string(&path)?);
                continue;
            }
            match chosen.get(&name) {
                Some((_, existing)) if *existing >= modified => {}
                _ => {
                    chosen.insert(name, (path, modified));
                }
            }
        }
    }
    let staging = brain.join(".claude").join("memory.merging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    for (name, (path, _)) in &chosen {
        std::fs::copy(path, staging.join(name))
            .with_context(|| format!("copying {}", path.display()))?;
    }
    let index = indexes
        .iter()
        .fold(String::new(), |merged, next| merge_index(&merged, next));
    let has_index = !index.trim().is_empty();
    if has_index {
        std::fs::write(staging.join(INDEX), index)?;
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    if target.is_dir() {
        let backup = brain
            .join(".claude")
            .join(format!("memory.pre-share-{stamp}"));
        std::fs::rename(&target, &backup)?;
    }
    std::fs::rename(&staging, &target)?;
    let mut backups = Vec::new();
    for source in &real_dirs {
        let backup = source.with_file_name(format!("memory.pre-share-{stamp}"));
        std::fs::rename(source, &backup)
            .with_context(|| format!("moving {} aside", source.display()))?;
        backups.push(backup);
        link(source, &target)?;
    }
    for source in &sources {
        if !points_to(source, &target) && std::fs::symlink_metadata(source).is_err() {
            link(source, &target)?;
        }
    }
    Ok(Outcome::Linked {
        files: chosen.len() + usize::from(has_index),
        backups,
    })
}

fn link(source: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = source.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::os::unix::fs::symlink(target, source)
        .with_context(|| format!("linking {} to {}", source.display(), target.display()))
}

static RAN: AtomicBool = AtomicBool::new(false);

/// Runs the merge once per launch for the open brain and tells the user
/// what happened, if anything did.
pub fn ensure_once(brain: PathBuf, workspace: WeakEntity<Workspace>, cx: &mut App) {
    if RAN.swap(true, Ordering::SeqCst) {
        return;
    }
    cx.spawn(async move |cx| {
        let result = cx
            .background_spawn(async move {
                let home = util::paths::home_dir().to_path_buf();
                let brainz_claude_dir = paths::config_dir().join("claude");
                ensure_shared(&brain, &home, &brainz_claude_dir)
            })
            .await;
        let message = match result {
            Ok(Outcome::AlreadyShared) => {
                log::info!("brainz memory share: already shared");
                return;
            }
            Ok(Outcome::Nothing) => {
                log::info!("brainz memory share: no memory folders to share yet");
                return;
            }
            Ok(Outcome::Linked { files, .. }) => format!(
                "Claude memory is now shared: {files} file(s) live in the brain at {TARGET}, and both the terminal and Brainz Claude read them."
            ),
            Err(error) => {
                log::error!("brainz memory share: {error:#}");
                format!("Could not share Claude memory into the brain: {error:#}")
            }
        };
        workspace
            .update(cx, |workspace, cx| {
                let id = workspace::notifications::NotificationId::unique::<Outcome>();
                workspace.show_notification(id.clone(), cx, |cx| {
                    cx.new(|cx| {
                        workspace::notifications::simple_message_notification::MessageNotification::new(
                            message,
                            cx,
                        )
                    })
                });
                cx.spawn(async move |workspace, cx| {
                    cx.background_executor().timer(Duration::from_secs(12)).await;
                    workspace
                        .update(cx, |workspace, cx| workspace.dismiss_notification(&id, cx))
                        .ok();
                })
                .detach();
            })
            .ok();
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_links_both_locations_and_keeps_the_newest_copy() {
        let root = tempfile::tempdir().unwrap();
        let brain = root.path().join("brain");
        let home = root.path().join("home");
        let brainz = root.path().join("brainz-config/claude");
        std::fs::create_dir_all(&brain).unwrap();
        let [cli, inside] = memory_dirs(&brain, &home, &brainz);
        std::fs::create_dir_all(&cli).unwrap();
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(cli.join("a.md"), "old a").unwrap();
        std::fs::write(cli.join("only-cli.md"), "cli").unwrap();
        std::fs::write(
            cli.join(INDEX),
            "# Memory\n- [A](a.md)\n- [Only](only-cli.md)\n",
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(inside.join("a.md"), "new a").unwrap();
        std::fs::write(inside.join("only-brainz.md"), "brainz").unwrap();
        std::fs::write(
            inside.join(INDEX),
            "# Memory\n- [A](a.md)\n- [B](only-brainz.md)\n",
        )
        .unwrap();
        let newer = std::time::SystemTime::now();
        filetime_touch(&inside.join("a.md"), newer);

        let outcome = ensure_shared(&brain, &home, &brainz).unwrap();
        let Outcome::Linked { files, backups } = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(files, 4);
        assert_eq!(backups.len(), 2);
        let target = brain.join(TARGET);
        assert_eq!(
            std::fs::read_to_string(target.join("a.md")).unwrap(),
            "new a"
        );
        assert!(target.join("only-cli.md").exists());
        assert!(target.join("only-brainz.md").exists());
        assert_eq!(
            std::fs::read_to_string(target.join(INDEX)).unwrap(),
            "# Memory\n- [A](a.md)\n- [Only](only-cli.md)\n- [B](only-brainz.md)\n"
        );
        assert!(
            std::fs::symlink_metadata(&cli)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::symlink_metadata(&inside)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(cli.join("only-brainz.md")).unwrap(),
            "brainz"
        );
        assert_eq!(
            ensure_shared(&brain, &home, &brainz).unwrap(),
            Outcome::AlreadyShared
        );
    }

    #[test]
    fn nothing_to_do_without_memory_and_a_foreign_symlink_stops() {
        let root = tempfile::tempdir().unwrap();
        let brain = root.path().join("brain");
        let home = root.path().join("home");
        let brainz = root.path().join("brainz-config/claude");
        std::fs::create_dir_all(&brain).unwrap();
        assert_eq!(
            ensure_shared(&brain, &home, &brainz).unwrap(),
            Outcome::Nothing
        );
        let [cli, _] = memory_dirs(&brain, &home, &brainz);
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(root.path().join("elsewhere"), &cli).unwrap();
        let error = ensure_shared(&brain, &home, &brainz).unwrap_err();
        assert!(error.to_string().contains("does not point at"), "{error}");
    }

    #[test]
    fn a_brain_that_arrives_with_memory_is_linked_or_merged() {
        // Second Mac: the brain was cloned with memory, nothing local yet.
        let root = tempfile::tempdir().unwrap();
        let brain = root.path().join("brain");
        let home = root.path().join("home");
        let brainz = root.path().join("brainz-config/claude");
        std::fs::create_dir_all(brain.join(TARGET)).unwrap();
        std::fs::write(brain.join(TARGET).join("z.md"), "z").unwrap();
        let outcome = ensure_shared(&brain, &home, &brainz).unwrap();
        assert!(
            matches!(outcome, Outcome::Linked { files: 1, .. }),
            "{outcome:?}"
        );
        let [cli, inside] = memory_dirs(&brain, &home, &brainz);
        assert!(points_to(&cli, &brain.join(TARGET)));
        assert!(points_to(&inside, &brain.join(TARGET)));
        assert_eq!(
            ensure_shared(&brain, &home, &brainz).unwrap(),
            Outcome::AlreadyShared
        );

        // A Mac with memory of its own meets a brain that has some too.
        let root = tempfile::tempdir().unwrap();
        let brain = root.path().join("brain");
        let home = root.path().join("home");
        let brainz = root.path().join("brainz-config/claude");
        std::fs::create_dir_all(brain.join(TARGET)).unwrap();
        std::fs::write(brain.join(TARGET).join("z.md"), "z").unwrap();
        std::fs::write(brain.join(TARGET).join(INDEX), "- z\n").unwrap();
        let [cli, inside] = memory_dirs(&brain, &home, &brainz);
        std::fs::create_dir_all(&cli).unwrap();
        std::fs::write(cli.join("a.md"), "a").unwrap();
        std::fs::write(cli.join(INDEX), "- a\n").unwrap();
        let outcome = ensure_shared(&brain, &home, &brainz).unwrap();
        assert!(
            matches!(outcome, Outcome::Linked { files: 3, .. }),
            "{outcome:?}"
        );
        let target = brain.join(TARGET);
        assert_eq!(std::fs::read_to_string(target.join("a.md")).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(target.join("z.md")).unwrap(), "z");
        assert_eq!(
            std::fs::read_to_string(target.join(INDEX)).unwrap(),
            "- z\n- a\n"
        );
        assert!(points_to(&cli, &target));
        assert!(points_to(&inside, &target));
        assert!(
            std::fs::read_dir(brain.join(".claude"))
                .unwrap()
                .filter_map(Result::ok)
                .any(|e| e
                    .file_name()
                    .to_string_lossy()
                    .starts_with("memory.pre-share-")),
            "the brain's earlier memory is kept as a backup"
        );
    }

    #[test]
    fn slug_matches_claude_code_naming() {
        assert_eq!(
            project_slug(Path::new("/Users/ada/brain")),
            "-Users-ada-brain"
        );
        assert_eq!(merge_index("a\nb\n", "b\nc\n"), "a\nb\nc\n");
    }

    fn filetime_touch(path: &Path, time: std::time::SystemTime) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(time).unwrap();
    }
}
