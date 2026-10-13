use std::{
    fs,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

const MAX_BYTES: u64 = 16 * 1024;
const MAX_RECENTS: usize = 12;
static WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Deserialize, Serialize)]
struct History {
    version: u32,
    ids: Vec<String>,
}

/// This path belongs in local application data, never in the brain's Git tree.
pub fn recent_templates_path(data_directory: &Path, root: &Path) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    let hash = Sha256::digest(root.as_os_str().as_encoded_bytes());
    Ok(data_directory
        .join("template-recents")
        .join(format!("{hash:x}.json")))
}

pub fn read_recents(path: &Path) -> Result<Vec<String>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut content = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut content)?;
    ensure!(
        content.len() as u64 <= MAX_BYTES,
        "Recent template history is too large"
    );
    let history: History =
        serde_json::from_slice(&content).context("Invalid recent template history")?;
    ensure!(
        history.version == 1,
        "Unsupported recent template history version"
    );
    ensure!(
        history.ids.len() <= MAX_RECENTS && history.ids.iter().all(|id| crate::valid_id(id)),
        "Invalid recent template IDs"
    );
    Ok(history.ids)
}

pub fn record_recent(path: &Path, id: &str) -> Result<()> {
    ensure!(crate::valid_id(id), "Invalid recent template ID");
    let _guard = WRITE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Recent template history lock is unavailable"))?;
    let mut ids = read_recents(path)?;
    ids.retain(|previous| previous != id);
    ids.insert(0, id.to_owned());
    ids.truncate(MAX_RECENTS);
    let parent = path
        .parent()
        .context("Recent template history needs a folder")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec(&History { version: 1, ids })?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_templates_stay_local_bounded_and_scoped_to_each_brain() {
        let directory = tempfile::tempdir().expect("local data");
        let first_root = directory.path().join("brain-one");
        let second_root = directory.path().join("brain-two");
        fs::create_dir(&first_root).expect("first brain");
        fs::create_dir(&second_root).expect("second brain");
        let first = recent_templates_path(directory.path(), &first_root).expect("first path");
        let second = recent_templates_path(directory.path(), &second_root).expect("second path");
        for index in 0..20 {
            record_recent(&first, &format!("template-{index}")).expect("record");
        }
        record_recent(&first, "template-9").expect("move to front");
        record_recent(&second, "daily").expect("second brain");
        let ids = read_recents(&first).expect("reload");
        assert_eq!(ids.len(), MAX_RECENTS);
        assert_eq!(ids.first().map(String::as_str), Some("template-9"));
        assert_eq!(ids.iter().filter(|id| *id == "template-9").count(), 1);
        assert_eq!(read_recents(&second).expect("second"), ["daily"]);
        let content = fs::read_to_string(&first).expect("history");
        assert!(!content.contains("brain-one"));
        fs::write(&first, r#"{"version":2,"ids":[]}"#).expect("future format");
        assert!(record_recent(&first, "daily").is_err());
        assert!(
            fs::read_to_string(&first)
                .expect("preserved")
                .contains("\"version\":2")
        );
    }
}
