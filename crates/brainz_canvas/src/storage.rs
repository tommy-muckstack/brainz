use crate::{atomic_json, read_json, safe_id, scoped_path};
use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};

pub const MAX_ASSET_BYTES: usize = 25 * 1024 * 1024;
pub const MAX_UPDATE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_STATE_BYTES: usize = 256 * 1024 * 1024;
const MAX_LOG_BYTES: usize = 64 * 1024 * 1024;
const TRASH_RETENTION_SECONDS: u64 = 7 * 24 * 60 * 60;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub mime: String,
    pub bytes: usize,
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trashed_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub format: String,
    pub version: u32,
    pub id: String,
    pub title: String,
    pub created_at: u64,
    pub schema: BTreeMap<String, String>,
    pub assets: BTreeMap<String, Asset>,
    #[serde(default)]
    pub asset_aliases: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct UpdateLog {
    pub updates: Vec<Vec<u8>>,
    pub valid_bytes: u64,
    pub torn_tail: bool,
}

impl UpdateLog {
    pub fn needs_compaction(&self) -> bool {
        self.valid_bytes >= 2 * 1024 * 1024 || self.updates.len() >= 500
    }
}

#[derive(Clone)]
pub struct BoardData {
    root: PathBuf,
    directory: PathBuf,
    id: String,
}

impl BoardData {
    pub fn new(root: &Path, id: &str) -> Result<Self> {
        ensure!(safe_id(id), "Invalid canvas id");
        let root = root.canonicalize()?;
        let directory = scoped_path(&root, &PathBuf::from(".brainz/boards").join(id), true)?;
        Ok(Self {
            root,
            directory,
            id: id.to_owned(),
        })
    }

    fn path(&self, relative: &Path) -> Result<PathBuf> {
        scoped_path(&self.root, &self.directory.join(relative), true)
    }

    pub fn metadata(&self) -> Result<Metadata> {
        let metadata: Metadata = serde_json::from_value(read_json(
            &self.path(Path::new("board.json"))?,
            4 * 1024 * 1024,
        )?)?;
        ensure!(
            metadata.format == "brainz-board" && metadata.version == 2 && metadata.id == self.id,
            "Canvas storage metadata does not match this board"
        );
        Ok(metadata)
    }

    pub fn initialize(&self, title: &str, snapshot: &[u8], now: u64) -> Result<Metadata> {
        ensure!(
            !snapshot.is_empty() && snapshot.len() <= MAX_STATE_BYTES,
            "Invalid canvas snapshot size"
        );
        let snapshot_path = self.path(Path::new("doc.ystate"))?;
        if snapshot_path.exists() {
            ensure!(
                read_bytes(&snapshot_path, MAX_STATE_BYTES)? == snapshot,
                "Canvas storage already exists with different content"
            );
            if self.path(Path::new("board.json"))?.exists() {
                return self.metadata();
            }
        } else {
            ensure!(
                !self.path(Path::new("board.json"))?.exists(),
                "The canvas snapshot is missing; existing metadata was preserved"
            );
            atomic_bytes(&snapshot_path, snapshot)?;
        }
        let metadata = Metadata {
            format: "brainz-board".into(),
            version: 2,
            id: self.id.clone(),
            title: title.to_owned(),
            created_at: now,
            schema: BTreeMap::from([("blocksuite".into(), "0.19.5".into())]),
            assets: BTreeMap::new(),
            asset_aliases: BTreeMap::new(),
        };
        self.write_metadata(&metadata)?;
        Ok(metadata)
    }

    fn write_metadata(&self, metadata: &Metadata) -> Result<()> {
        atomic_json(
            &self.path(Path::new("board.json"))?,
            &serde_json::to_value(metadata)?,
        )
    }

    pub fn lock(&self) -> Result<fs::File> {
        let path = self.path(Path::new(".lock"))?;
        fs::create_dir_all(path.parent().context("Canvas has no data folder")?)?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.path(Path::new(".gitignore"))?)
        {
            Ok(mut file) => {
                file.write_all(b"/.lock\n/.cache/\n/assets/.trash/\n")?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.try_lock()
            .context("This canvas is already open in another Brainz window")?;
        Ok(file)
    }

    pub fn rename(&self, title: &str) -> Result<()> {
        ensure!(title.len() <= 1024, "Canvas title is too long");
        let mut metadata = self.metadata()?;
        if metadata.title != title {
            metadata.title = title.to_owned();
            self.write_metadata(&metadata)?;
        }
        Ok(())
    }

    pub fn write_context(&self, context: &serde_json::Value) -> Result<()> {
        ensure!(
            serde_json::to_vec(context)?.len() as u64 <= crate::MAX_BOARD_BYTES,
            "Canvas context is too large"
        );
        atomic_json(&self.path(Path::new("canvas.json"))?, context)
    }

    pub fn import_asset(
        &self,
        source_id: &str,
        mime: &str,
        name: Option<String>,
        bytes: &[u8],
    ) -> Result<String> {
        ensure!(source_id.len() <= 512, "Invalid image source");
        let hash = format!("{:x}", Sha256::digest(bytes));
        self.put_asset(&hash, mime, name, bytes)?;
        if !source_id.is_empty() && source_id != hash {
            let mut metadata = self.metadata()?;
            metadata
                .asset_aliases
                .insert(source_id.to_owned(), hash.clone());
            self.write_metadata(&metadata)?;
        }
        Ok(hash)
    }

    pub fn image_sources(&self) -> Result<BTreeMap<String, Asset>> {
        let metadata = self.metadata()?;
        let mut sources = metadata.assets.clone();
        for (source, hash) in metadata.asset_aliases {
            if let Some(asset) = metadata.assets.get(&hash) {
                sources.insert(source, asset.clone());
            }
        }
        Ok(sources)
    }

    pub fn publish(&self, visible_path: &Path, expected: Option<&str>) -> Result<()> {
        let path = scoped_path(&self.root, visible_path, true)?;
        ensure!(
            path.extension()
                .is_some_and(|extension| extension == "brainzboard"),
            "Choose a canvas file for the storage manifest"
        );
        if path.exists() {
            let previous = read_json(&path, crate::MAX_BOARD_BYTES)?;
            ensure!(
                expected == Some(crate::document_hash(&previous)?.as_str()),
                "The canvas changed before its storage migration could finish"
            );
            if previous["version"] == 1 {
                let mut backup =
                    scoped_path(&self.root, &path.with_extension("brainzboard.v1.bak"), true)?;
                if backup.exists() && fs::read(&backup)? != fs::read(&path)? {
                    backup =
                        path.with_extension(format!("brainzboard.{}.v1.bak", uuid::Uuid::new_v4()));
                }
                let backup = scoped_path(&self.root, &backup, true)?;
                let mut temporary = tempfile::NamedTempFile::new_in(
                    path.parent().context("Canvas has no folder")?,
                )?;
                temporary.write_all(&fs::read(&path)?)?;
                temporary.as_file().sync_all()?;
                match temporary.persist_noclobber(&backup) {
                    Ok(_) => {}
                    Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.error.into()),
                }
            }
        } else {
            ensure!(
                expected.is_none(),
                "The original canvas moved or was deleted"
            );
        }
        atomic_json(&path, &self.manifest()?)
    }

    pub fn manifest(&self) -> Result<serde_json::Value> {
        let metadata = self.metadata()?;
        Ok(
            json!({"format":"brainz-whiteboard","version":2,"id":metadata.id,"title":metadata.title,"schema":metadata.schema}),
        )
    }

    pub fn snapshot(&self) -> Result<Vec<u8>> {
        read_bytes(&self.path(Path::new("doc.ystate"))?, MAX_STATE_BYTES)
    }

    pub fn updates(&self) -> Result<UpdateLog> {
        let path = self.path(Path::new("doc.yupdates"))?;
        let bytes = if path.exists() {
            read_bytes(&path, MAX_LOG_BYTES)?
        } else {
            Vec::new()
        };
        let mut offset = 0;
        let mut updates = Vec::new();
        while bytes.len() - offset >= 4 {
            let length = u32::from_le_bytes(bytes[offset..offset + 4].try_into()?) as usize;
            ensure!(
                length > 0 && length <= MAX_UPDATE_BYTES,
                "Invalid canvas update length; the log has been preserved"
            );
            if length > bytes.len() - offset - 4 {
                break;
            }
            updates.push(bytes[offset + 4..offset + 4 + length].to_vec());
            offset += 4 + length;
        }
        Ok(UpdateLog {
            updates,
            valid_bytes: offset as u64,
            torn_tail: offset != bytes.len(),
        })
    }

    pub fn append(&self, update: &[u8]) -> Result<UpdateLog> {
        ensure!(
            !update.is_empty() && update.len() <= MAX_UPDATE_BYTES,
            "Canvas update exceeds its size limit"
        );
        self.metadata()?;
        let mut log = self.updates()?;
        ensure!(
            log.valid_bytes + update.len() as u64 + 4 <= MAX_LOG_BYTES as u64,
            "Compact the canvas before appending more updates"
        );
        let path = self.path(Path::new("doc.yupdates"))?;
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        if log.torn_tail {
            file.set_len(log.valid_bytes)?;
        }
        let mut record = Vec::with_capacity(update.len() + 4);
        record.extend_from_slice(&(update.len() as u32).to_le_bytes());
        record.extend_from_slice(update);
        file.write_all(&record)?;
        file.sync_data()?;
        log.valid_bytes += record.len() as u64;
        log.torn_tail = false;
        log.updates.push(update.to_vec());
        Ok(log)
    }

    pub fn stamp(&self) -> Result<String> {
        let mut stamp = String::new();
        for file in ["doc.ystate", "doc.yupdates"] {
            let path = self.path(Path::new(file))?;
            match fs::metadata(path) {
                Ok(metadata) => {
                    let modified = metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?;
                    stamp.push_str(&format!(
                        "{}:{}:{};",
                        metadata.len(),
                        modified.as_secs(),
                        modified.subsec_nanos()
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    stamp.push_str("missing;")
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(stamp)
    }

    pub fn compact(&self, snapshot: &[u8], expected_stamp: &str) -> Result<()> {
        ensure!(
            !snapshot.is_empty() && snapshot.len() <= MAX_STATE_BYTES,
            "Invalid canvas snapshot size"
        );
        self.metadata()?;
        ensure!(
            self.stamp()? == expected_stamp,
            "The canvas changed before compaction; merge its latest updates first"
        );
        // A crash between these writes leaves old updates replayable over the
        // newer snapshot. Yjs updates are idempotent, so this cannot lose edits.
        atomic_bytes(&self.path(Path::new("doc.ystate"))?, snapshot)?;
        atomic_bytes(&self.path(Path::new("doc.yupdates"))?, &[])
    }

    pub fn put_asset(
        &self,
        hash: &str,
        mime: &str,
        name: Option<String>,
        bytes: &[u8],
    ) -> Result<()> {
        ensure!(
            bytes.len() <= MAX_ASSET_BYTES && !bytes.is_empty(),
            "Images must be 25 MB or smaller"
        );
        ensure!(
            valid_hash(hash) && format!("{:x}", Sha256::digest(bytes)) == hash,
            "The image checksum does not match its bytes"
        );
        let extension = extension(mime)?;
        let mut metadata = self.metadata()?;
        if let Some(existing) = metadata.assets.get(hash) {
            ensure!(
                existing.mime == mime,
                "The existing image has a different content type"
            );
        }
        let path = self.path(&PathBuf::from("assets").join(format!("{hash}.{extension}")))?;
        if path.exists() {
            ensure!(
                read_bytes(&path, MAX_ASSET_BYTES)? == bytes,
                "The existing image has different content"
            );
        } else {
            atomic_bytes(&path, bytes)?;
        }
        let trash =
            self.path(&PathBuf::from("assets/.trash").join(format!("{hash}.{extension}")))?;
        if trash.exists() {
            fs::remove_file(&trash)?;
        }
        metadata.assets.insert(
            hash.to_owned(),
            Asset {
                mime: mime.into(),
                bytes: bytes.len(),
                name,
                trashed_at: None,
            },
        );
        self.write_metadata(&metadata)
    }

    pub fn asset_path(&self, hash: &str) -> Result<(PathBuf, String)> {
        let metadata = self.metadata()?;
        let hash = metadata
            .asset_aliases
            .get(hash)
            .map(String::as_str)
            .unwrap_or(hash);
        ensure!(valid_hash(hash), "Invalid image hash");
        let asset = metadata
            .assets
            .get(hash)
            .context("Canvas image is missing")?;
        let extension = extension(&asset.mime)?;
        let folder = if asset.trashed_at.is_some() {
            "assets/.trash"
        } else {
            "assets"
        };
        let mut path = self.path(&PathBuf::from(folder).join(format!("{hash}.{extension}")))?;
        if !path.exists() {
            // Recover a move completed just before an interrupted metadata write.
            let alternate = if asset.trashed_at.is_some() {
                "assets"
            } else {
                "assets/.trash"
            };
            path = self.path(&PathBuf::from(alternate).join(format!("{hash}.{extension}")))?;
        }
        ensure!(path.is_file(), "Canvas image file is missing");
        Ok((path, asset.mime.clone()))
    }

    pub fn collect_assets(&self, used: &HashSet<String>, now: u64) -> Result<()> {
        let mut metadata = self.metadata()?;
        let used: HashSet<String> = used
            .iter()
            .map(|source| metadata.asset_aliases.get(source).unwrap_or(source).clone())
            .collect();
        let mut expired = Vec::new();
        for (hash, asset) in &mut metadata.assets {
            ensure!(valid_hash(hash), "Invalid stored image hash");
            let file = format!("{hash}.{}", extension(&asset.mime)?);
            let live = self.path(&PathBuf::from("assets").join(&file))?;
            let trash = self.path(&PathBuf::from("assets/.trash").join(&file))?;
            if used.contains(hash) {
                if trash.exists() {
                    fs::rename(&trash, &live)?;
                }
                asset.trashed_at = None;
            } else if let Some(at) = asset.trashed_at {
                if now.saturating_sub(at) >= TRASH_RETENTION_SECONDS {
                    if trash.exists() {
                        fs::remove_file(&trash)?;
                    }
                    expired.push(hash.clone());
                }
            } else {
                fs::create_dir_all(trash.parent().context("Missing image trash folder")?)?;
                if live.exists() {
                    fs::rename(&live, &trash)?;
                }
                asset.trashed_at = Some(now);
            }
        }
        for hash in expired {
            metadata.assets.remove(&hash);
        }
        self.write_metadata(&metadata)
    }
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn extension(mime: &str) -> Result<&'static str> {
    match mime {
        "image/png" => Ok("png"),
        "image/jpeg" => Ok("jpg"),
        "image/webp" => Ok("webp"),
        "image/gif" => Ok("gif"),
        _ => anyhow::bail!("Unsupported canvas image type"),
    }
}
fn read_bytes(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    ensure!(
        file.metadata()?.len() <= maximum as u64,
        "Canvas data exceeds its size limit"
    );
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= maximum,
        "Canvas data grew beyond its size limit"
    );
    Ok(bytes)
}
fn atomic_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("Canvas data has no folder")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_preserves_complete_records_after_a_torn_write_and_compacts_without_losing_state() {
        let root = tempfile::tempdir().expect("brain");
        let store = BoardData::new(root.path(), "diagram").expect("store");
        store
            .initialize("Diagram", b"initial state", 0)
            .expect("initialize");
        store.append(b"first edit").expect("first");
        store.append(b"second edit").expect("second");
        let path = store.path(Path::new("doc.yupdates")).expect("log path");
        let mut file = OpenOptions::new().append(true).open(&path).expect("log");
        file.write_all(&[20, 0, 0, 0, 1, 2]).expect("torn record");
        drop(file);
        let loaded = store.updates().expect("read complete updates");
        assert!(loaded.torn_tail);
        assert_eq!(
            loaded.updates,
            vec![b"first edit".to_vec(), b"second edit".to_vec()]
        );
        store.append(b"third edit").expect("repair and append");
        assert_eq!(store.updates().expect("repaired").updates.len(), 3);
        let stamp = store.stamp().expect("stamp");
        assert!(store.compact(b"stale state", "old-stamp").is_err());
        store.compact(b"merged state", &stamp).expect("compact");
        assert_eq!(store.snapshot().expect("snapshot"), b"merged state");
        assert!(store.updates().expect("empty log").updates.is_empty());
    }

    #[test]
    fn migration_keeps_the_visible_folder_and_exact_v1_backup_and_rejects_external_changes() {
        let root = tempfile::tempdir().expect("brain");
        let path = root.path().join("Projects/Plan.brainzboard");
        let old = json!({"format":"brainz-whiteboard","version":1,"data":"old"});
        atomic_json(&path, &old).expect("old board");
        let bytes = fs::read(&path).expect("old bytes");
        let hash = crate::document_hash(&old).expect("hash");
        let store = BoardData::new(root.path(), "diagram").expect("store");
        store
            .initialize("Plan", b"Yjs state", 0)
            .expect("initialize");
        assert!(store.publish(&path, Some("stale")).is_err());
        store.publish(&path, Some(&hash)).expect("publish");
        assert_eq!(
            fs::read(path.with_extension("brainzboard.v1.bak")).expect("backup"),
            bytes
        );
        assert_eq!(read_json(&path, 1024).expect("manifest")["version"], 2);
        assert!(path.is_file());
    }

    #[test]
    fn images_are_checked_deduplicated_and_recoverable_from_trash_for_seven_days() {
        let root = tempfile::tempdir().expect("brain");
        let store = BoardData::new(root.path(), "diagram").expect("store");
        store.initialize("Plan", b"state", 0).expect("initialize");
        let image = b"image bytes";
        let hash = format!("{:x}", Sha256::digest(image));
        assert!(
            store
                .put_asset(&"0".repeat(64), "image/png", None, image)
                .is_err()
        );
        store
            .put_asset(&hash, "image/png", None, image)
            .expect("image");
        store
            .put_asset(&hash, "image/png", None, image)
            .expect("deduplicate");
        assert_eq!(store.metadata().expect("metadata").assets.len(), 1);
        store.collect_assets(&HashSet::new(), 100).expect("trash");
        let (path, _) = store.asset_path(&hash).expect("undo can read trash");
        assert!(path.to_string_lossy().contains(".trash"));
        store
            .collect_assets(&HashSet::from([hash.clone()]), 200)
            .expect("restore");
        assert!(
            !store
                .asset_path(&hash)
                .expect("restored")
                .0
                .to_string_lossy()
                .contains(".trash")
        );
        store
            .collect_assets(&HashSet::new(), 300)
            .expect("trash again");
        store
            .collect_assets(&HashSet::new(), 300 + TRASH_RETENTION_SECONDS)
            .expect("expire");
        assert!(store.asset_path(&hash).is_err());
    }
}
