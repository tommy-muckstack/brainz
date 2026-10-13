use super::*;
use brainz_canvas::storage::{BoardData, MAX_STATE_BYTES, MAX_UPDATE_BYTES};
use std::collections::BTreeMap;

pub(super) struct IncrementalStorage {
    pub id: String,
    pub data: BoardData,
    pub observed_stamp: String,
    unmerged_stamp: Option<String>,
    forwarded_stamp: Option<String>,
    _lock: std::fs::File,
    migration: Option<Value>,
    updates: BTreeMap<u64, Vec<u8>>,
    title: String,
    compact: Option<(Vec<u8>, String, Vec<String>)>,
    needs_compaction: bool,
    asset_bytes: u64,
}

pub(super) enum StorageChange {
    Migrate {
        revision: u64,
        document: Value,
        base_hash: Option<String>,
    },
    Append {
        revision: u64,
        update: Option<String>,
        title: String,
    },
    Compact {
        snapshot: String,
        stamp: String,
        used_assets: Vec<String>,
    },
}

fn decode(value: &str, maximum: usize) -> Result<Vec<u8>> {
    ensure!(
        value.len() <= maximum.div_ceil(3) * 4,
        "Canvas data exceeds its size limit"
    );
    let bytes = base64::engine::general_purpose::STANDARD.decode(value)?;
    ensure!(bytes.len() <= maximum, "Canvas data exceeds its size limit");
    Ok(bytes)
}

impl IncrementalStorage {
    pub fn open(pending: &PendingDocument, document: &Value) -> Result<Self> {
        let root = pending
            .storage_root
            .as_ref()
            .context("Canvas workspace is missing")?;
        let id = document["id"].as_str().context("Canvas id is missing")?;
        let data = BoardData::new(root, id)?;
        let lock = data.lock()?;
        let observed_stamp = data.stamp()?;
        let asset_bytes = if document["version"] == 2 {
            data.metadata()?
                .assets
                .values()
                .map(|asset| asset.bytes as u64)
                .sum()
        } else {
            0
        };
        Ok(Self {
            id: id.to_owned(),
            data,
            observed_stamp,
            unmerged_stamp: None,
            forwarded_stamp: None,
            _lock: lock,
            migration: None,
            updates: BTreeMap::new(),
            title: document["title"]
                .as_str()
                .unwrap_or("Untitled canvas")
                .into(),
            compact: None,
            needs_compaction: false,
            asset_bytes,
        })
    }

    pub fn wire_document(&self, manifest: &Value) -> Result<Value> {
        let mut wire = manifest.clone();
        wire["data"] =
            json!(base64::engine::general_purpose::STANDARD.encode(self.data.snapshot()?));
        wire["updates"] = json!(
            self.data
                .updates()?
                .updates
                .iter()
                .map(|update| base64::engine::general_purpose::STANDARD.encode(update))
                .collect::<Vec<_>>()
        );
        wire["assets"] = serde_json::to_value(self.data.image_sources()?)?;
        wire["storageStamp"] = json!(self.data.stamp()?);
        Ok(wire)
    }

    pub fn changed_on_disk(&self) -> Result<bool> {
        let stamp = self.data.stamp()?;
        Ok(stamp != self.observed_stamp
            || (self.unmerged_stamp.is_some() && self.forwarded_stamp.as_ref() != Some(&stamp)))
    }

    pub fn forward_external(&mut self, stamp: String) {
        self.observed_stamp = stamp.clone();
        self.unmerged_stamp = Some(stamp.clone());
        self.forwarded_stamp = Some(stamp);
    }
}

impl BoardStorage {
    pub(super) fn write_incremental_recovery(
        pending: &PendingDocument,
        document: &Value,
        path: &Path,
    ) -> Result<()> {
        let storage = pending
            .incremental
            .as_ref()
            .context("Canvas storage is unavailable")?;
        let root = pending
            .storage_root
            .as_ref()
            .context("Canvas workspace is missing")?;
        let id = brainz_canvas::legacy_board_id(path);
        let recovery = BoardData::new(root, &id)?;
        let snapshot = decode(
            document["data"]
                .as_str()
                .context("Recovery snapshot is missing")?,
            MAX_STATE_BYTES,
        )?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let title = format!(
            "{} (recovered)",
            document["title"].as_str().unwrap_or("Canvas")
        );
        if recovery.metadata().is_ok() {
            recovery.compact(&snapshot, &recovery.stamp()?)?;
        } else {
            recovery.initialize(&title, &snapshot, now)?;
        }
        for (source, asset) in storage.data.image_sources()? {
            let (path, _) = storage.data.asset_path(&source)?;
            recovery.import_asset(&source, &asset.mime, asset.name, &std::fs::read(path)?)?;
        }
        if let Some(scene) = document.get("scene") {
            recovery.write_context(scene)?;
        }
        write_json_atomically(path, &recovery.manifest()?)
    }

    pub(super) fn queue_incremental(&self, change: StorageChange) -> Result<()> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow!("Canvas storage is unavailable"))?;
        match change {
            StorageChange::Migrate {
                revision,
                document,
                base_hash,
            } => {
                validate_document(&document)?;
                ensure!(
                    document["version"] == 1,
                    "Expected the original canvas for migration"
                );
                ensure!(
                    serde_json::to_vec(&document)?.len() <= MAX_DOCUMENT_BYTES,
                    "The original canvas is too large"
                );
                if pending.incremental.is_none() {
                    let mut storage = IncrementalStorage::open(&pending, &document)?;
                    storage.migration = Some(document.clone());
                    pending.board_id = Some(storage.id.clone());
                    pending.document = Some(document);
                    pending.incremental = Some(storage);
                    pending.queued_base = base_hash.or_else(|| pending.disk_hash.clone());
                }
                pending.revision = pending.revision.max(revision);
            }
            StorageChange::Append {
                revision,
                update,
                title,
            } => {
                ensure!(title.len() <= 1024, "Canvas title is too long");
                let update = update
                    .as_deref()
                    .map(|update| decode(update, MAX_UPDATE_BYTES))
                    .transpose()?;
                if pending
                    .saved_revision
                    .is_some_and(|saved| revision <= saved)
                {
                    return Ok(());
                }
                let storage = pending
                    .incremental
                    .as_mut()
                    .context("Canvas migration has not started")?;
                if let Some(update) = update {
                    ensure!(!update.is_empty(), "Empty canvas update");
                    storage.updates.entry(revision).or_insert(update);
                }
                storage.title = title;
                pending.revision = pending.revision.max(revision);
            }
            StorageChange::Compact {
                snapshot,
                stamp,
                used_assets,
            } => {
                let snapshot = decode(&snapshot, MAX_STATE_BYTES)?;
                ensure!(
                    used_assets.len() <= 100_000
                        && used_assets.iter().all(|source| source.len() <= 512),
                    "Too many canvas images"
                );
                pending
                    .incremental
                    .as_mut()
                    .context("Canvas storage is unavailable")?
                    .compact = Some((snapshot, stamp, used_assets));
            }
        }
        Ok(())
    }

    pub(super) fn save_incremental(pending: &mut PendingDocument) -> Result<u64> {
        let storage = pending
            .incremental
            .as_mut()
            .context("Canvas storage is unavailable")?;
        if let Some(document) = &storage.migration {
            let snapshot = decode(
                document["data"]
                    .as_str()
                    .context("Canvas snapshot is missing")?,
                MAX_STATE_BYTES,
            )?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            storage.data.initialize(&storage.title, &snapshot, now)?;
            if let Some(images) = document["images"].as_object() {
                for (id, asset) in images {
                    let bytes = decode(
                        asset["data"]
                            .as_str()
                            .context("Canvas image bytes are missing")?,
                        brainz_canvas::storage::MAX_ASSET_BYTES,
                    )?;
                    storage.data.import_asset(
                        id,
                        asset["mime"]
                            .as_str()
                            .context("Canvas image type is missing")?,
                        None,
                        &bytes,
                    )?;
                }
            }
            storage
                .data
                .publish(&pending.path, pending.queued_base.as_deref())?;
            if let Some(scene) = document.get("scene") {
                storage.data.write_context(scene)?;
            }
            storage.migration = None;
            storage.asset_bytes = storage
                .data
                .metadata()?
                .assets
                .values()
                .map(|asset| asset.bytes as u64)
                .sum();
            pending.persisted = true;
            let manifest = read_document(&pending.path, false)?;
            pending.disk_hash = Some(brainz_canvas::document_hash(&manifest)?);
            pending.document = Some(manifest);
            storage.observed_stamp = storage.data.stamp()?;
        }
        ensure!(
            pending.path.is_file(),
            "Canvas moved or deleted; waiting for its new location"
        );
        let manifest = read_document(&pending.path, false)?;
        ensure!(
            manifest["version"] == 2 && manifest["id"].as_str() == Some(storage.id.as_str()),
            "The canvas file was replaced. Your pending edits have been preserved."
        );
        let before_append = storage.data.stamp()?;
        if before_append != storage.observed_stamp {
            storage.unmerged_stamp = Some(before_append);
        }
        while let Some((&revision, update)) = storage.updates.first_key_value() {
            let log = storage.data.append(update)?;
            storage.needs_compaction |= log.needs_compaction();
            storage.updates.remove(&revision);
        }
        if manifest["title"].as_str() != Some(storage.title.as_str()) {
            storage.data.rename(&storage.title)?;
            storage.data.publish(
                &pending.path,
                Some(&brainz_canvas::document_hash(&manifest)?),
            )?;
        }
        if let Some((snapshot, stamp, used_assets)) = storage.compact.take() {
            // A newer append or an external edit makes this checkpoint stale.
            // Keep the complete log and request another checkpoint instead.
            if storage.unmerged_stamp.is_none() && stamp == storage.data.stamp()? {
                storage.data.compact(&snapshot, &stamp)?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs();
                storage
                    .data
                    .collect_assets(&used_assets.into_iter().collect(), now)?;
                storage.needs_compaction = false;
            } else {
                storage.needs_compaction = true;
            }
        }
        storage.observed_stamp = storage.data.stamp()?;
        pending.saved_revision = Some(pending.revision);
        let manifest = read_document(&pending.path, false)?;
        pending.disk_hash = Some(brainz_canvas::document_hash(&manifest)?);
        pending.queued_base = pending.disk_hash.clone();
        pending.document = Some(manifest);
        let metadata = std::fs::metadata(&pending.path)?;
        pending.observed_stamp = Some((metadata.len(), metadata.modified()?));
        Ok(pending.revision)
    }

    pub(super) fn storage_status(&self) -> Result<Value> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| anyhow!("Canvas storage is unavailable"))?;
        Ok(match &pending.incremental {
            Some(storage) if storage.migration.is_none() => {
                json!({"version":2,"stamp":storage.unmerged_stamp.is_none().then_some(&storage.observed_stamp),"needsCompaction":storage.needs_compaction && storage.unmerged_stamp.is_none(),"assetBytes":storage.asset_bytes})
            }
            _ => Value::Null,
        })
    }

    pub(super) fn put_image(
        &self,
        mime: &str,
        name: Option<String>,
        encoded: &str,
    ) -> Result<String> {
        let bytes = decode(encoded, brainz_canvas::storage::MAX_ASSET_BYTES)?;
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow!("Canvas storage is unavailable"))?;
        Self::save_pending(&mut pending)?;
        let storage = pending
            .incremental
            .as_mut()
            .context("Save this canvas before adding an image")?;
        // The hash is returned only after the image and its metadata are durable.
        let hash = storage.data.import_asset("", mime, name, &bytes)?;
        storage.asset_bytes = storage
            .data
            .metadata()?
            .assets
            .values()
            .map(|asset| asset.bytes as u64)
            .sum();
        Ok(hash)
    }

    pub(super) fn external_merged(&self, stamp: &str) -> Result<()> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow!("Canvas storage is unavailable"))?;
        let storage = pending
            .incremental
            .as_mut()
            .context("Canvas storage is unavailable")?;
        if storage.unmerged_stamp.as_deref() == Some(stamp)
            && storage.data.stamp()? == storage.observed_stamp
        {
            storage.unmerged_stamp = None;
            storage.forwarded_stamp = None;
        }
        Ok(())
    }

    pub(super) fn image(&self, source: &str) -> Result<(Vec<u8>, String)> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| anyhow!("Canvas storage is unavailable"))?;
        let storage = pending
            .incremental
            .as_ref()
            .context("Canvas image storage is unavailable")?;
        let (path, mime) = storage.data.asset_path(source)?;
        ensure!(
            std::fs::metadata(&path)?.len() <= brainz_canvas::storage::MAX_ASSET_BYTES as u64,
            "Canvas image is too large"
        );
        Ok((std::fs::read(path)?, mime))
    }

    pub(super) fn save_context(&self, context: Value) -> Result<()> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| anyhow!("Canvas storage is unavailable"))?;
        let storage = pending
            .incremental
            .as_ref()
            .context("Canvas storage is unavailable")?;
        storage.data.write_context(&context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage(root: &Path, path: PathBuf) -> BoardStorage {
        let mut storage = BoardStorage::new(path);
        storage.pending.get_mut().expect("pending").storage_root = Some(root.to_path_buf());
        storage
    }

    fn fixture() -> Value {
        json!({"format":"brainz-whiteboard","version":1,"id":"migration-board","title":"Plan","data":"AQID","images":{"legacy-image":{"mime":"image/png","data":"aW1hZ2U="}},"scene":{"objects":[{"id":"shape-one","text":"Keep me"}]}})
    }

    #[test]
    fn thirty_megabytes_of_images_do_not_expand_small_edits_and_unused_images_remain_recoverable() {
        let temp = tempfile::tempdir().expect("brain");
        let path = temp.path().join("Large.brainzboard");
        let storage = storage(temp.path(), path.clone());
        storage.load(true).expect("open");
        storage
            .queue_incremental(StorageChange::Migrate {
                revision: 1,
                document: fixture(),
                base_hash: None,
            })
            .expect("migration");
        storage.save().expect("save");
        let mut image_ids = Vec::new();
        for index in 0..30 {
            let mut bytes = vec![0u8; 1024 * 1024];
            bytes[0] = index;
            image_ids.push(
                storage
                    .put_image(
                        "image/png",
                        None,
                        &base64::engine::general_purpose::STANDARD.encode(bytes),
                    )
                    .expect("image"),
            );
        }
        let data = BoardData::new(temp.path(), "migration-board").expect("data");
        let before_snapshot = data.snapshot().expect("snapshot");
        let before_manifest = std::fs::read(&path).expect("manifest");
        storage
            .queue_incremental(StorageChange::Append {
                revision: 2,
                update: Some("BAU=".into()),
                title: "Plan".into(),
            })
            .expect("small edit");
        storage.save().expect("save edit");
        assert!(data.updates().expect("log").valid_bytes < 64 * 1024);
        assert_eq!(data.snapshot().expect("snapshot"), before_snapshot);
        assert_eq!(std::fs::read(&path).expect("manifest"), before_manifest);
        let used = image_ids[1..].to_vec();
        storage
            .queue_incremental(StorageChange::Compact {
                snapshot: "AQIDBAU=".into(),
                stamp: data.stamp().expect("stamp"),
                used_assets: used,
            })
            .expect("compact");
        storage.save().expect("compacted");
        assert!(
            data.metadata().expect("metadata").assets[&image_ids[0]]
                .trashed_at
                .is_some()
        );
        assert_eq!(
            storage
                .image(&image_ids[0])
                .expect("undo can still read image")
                .0
                .len(),
            1024 * 1024
        );
        assert!(data.updates().expect("log").updates.is_empty());
    }

    #[test]
    fn migrates_in_place_then_replays_appends_and_reopens_with_external_images() {
        let temp = tempfile::tempdir().expect("brain");
        let folder = temp.path().join("Projects");
        std::fs::create_dir(&folder).expect("folder");
        let path = folder.join("Plan.brainzboard");
        let original = fixture();
        write_json_atomically(&path, &original).expect("original");
        let original_bytes = std::fs::read(&path).expect("bytes");
        let storage = storage(temp.path(), path.clone());
        storage.load(false).expect("load");
        storage
            .queue_incremental(StorageChange::Migrate {
                revision: 1,
                document: original.clone(),
                base_hash: Some(brainz_canvas::document_hash(&original).expect("hash")),
            })
            .expect("migration");
        storage
            .queue_incremental(StorageChange::Append {
                revision: 2,
                update: Some("BAU=".into()),
                title: "Plan updated".into(),
            })
            .expect("edit during migration");
        assert_eq!(storage.save().expect("save"), 2);
        assert_eq!(
            std::fs::read(path.with_extension("brainzboard.v1.bak")).expect("backup"),
            original_bytes
        );
        let manifest = read_document(&path, false).expect("manifest");
        assert_eq!(manifest["version"], 2);
        assert!(manifest.get("data").is_none());
        assert!(manifest.get("images").is_none());
        assert_eq!(storage.image("legacy-image").expect("image").0, b"image");
        assert!(storage.image("../../outside").is_err());
        let other = self::storage(temp.path(), folder.join("Copy.brainzboard"));
        write_json_atomically(&other.path().expect("path"), &manifest).expect("copy");
        assert!(
            other.load(false).is_err(),
            "a second manifest must not bypass the board id lock"
        );
        drop(storage);
        let reopened = self::storage(temp.path(), path);
        let wire = reopened.load(false).expect("reopen");
        assert_eq!(wire["data"], "AQID");
        assert_eq!(wire["updates"], json!(["BAU="]));
        assert_eq!(wire["assets"]["legacy-image"]["mime"], "image/png");
        assert_eq!(wire["title"], "Plan updated");
    }

    #[test]
    fn external_updates_are_observed_and_stale_compaction_cannot_drop_them() {
        let temp = tempfile::tempdir().expect("brain");
        let path = temp.path().join("Plan.brainzboard");
        let storage = storage(temp.path(), path);
        storage.load(true).expect("open");
        storage
            .queue_incremental(StorageChange::Migrate {
                revision: 1,
                document: fixture(),
                base_hash: None,
            })
            .expect("migrate");
        storage.save().expect("save");
        let stale = storage.storage_status().expect("status")["stamp"]
            .as_str()
            .expect("stamp")
            .to_owned();
        let external = BoardData::new(temp.path(), "migration-board").expect("data");
        external.append(&[7, 8]).expect("external");
        let (wire, _) = storage.external_document().expect("watch").expect("change");
        assert_eq!(wire["updates"], json!(["Bwg="]));
        storage
            .queue_incremental(StorageChange::Append {
                revision: 2,
                update: Some("CQo=".into()),
                title: "Plan".into(),
            })
            .expect("local edit");
        storage.save().expect("append");
        assert!(
            storage.storage_status().expect("status")["stamp"].is_null(),
            "an acknowledgement must not advertise a checkpoint containing unseen external edits"
        );
        let unsafe_stamp = external.stamp().expect("current stamp");
        storage
            .queue_incremental(StorageChange::Compact {
                snapshot: "AQID".into(),
                stamp: unsafe_stamp,
                used_assets: vec![],
            })
            .expect("queue");
        storage.save().expect("skip unseen");
        assert_eq!(
            external.updates().expect("log").updates,
            vec![vec![7, 8], vec![9, 10]]
        );
        storage
            .queue_incremental(StorageChange::Compact {
                snapshot: "AQID".into(),
                stamp: stale,
                used_assets: vec!["legacy-image".into()],
            })
            .expect("queue");
        storage.save().expect("skip stale");
        assert_eq!(
            external.updates().expect("log").updates,
            vec![vec![7, 8], vec![9, 10]]
        );
        let (wire, _) = storage
            .external_document()
            .expect("watch")
            .expect("latest merge");
        storage
            .external_merged(wire["storageStamp"].as_str().expect("stamp"))
            .expect("merged");
        assert!(storage.storage_status().expect("status")["stamp"].is_string());
    }

    #[test]
    fn future_version_and_external_replacement_are_preserved_and_recovery_keeps_images() {
        let temp = tempfile::tempdir().expect("brain");
        let path = temp.path().join("Plan.brainzboard");
        let storage = storage(temp.path(), path.clone());
        storage.load(true).expect("open");
        storage
            .queue_incremental(StorageChange::Migrate {
                revision: 1,
                document: fixture(),
                base_hash: None,
            })
            .expect("migrate");
        storage.save().expect("save");
        let future =
            json!({"format":"brainz-whiteboard","version":3,"id":"future","title":"Future"});
        write_json_atomically(&path, &future).expect("future");
        storage
            .queue_incremental(StorageChange::Append {
                revision: 2,
                update: Some("BAU=".into()),
                title: "Local".into(),
            })
            .expect("edit");
        assert!(storage.save().is_err());
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&path).expect("bytes")).expect("json"),
            future
        );
        let recovery_path = storage.recover(fixture()).expect("recovery");
        let recovery = self::storage(temp.path(), recovery_path);
        let wire = recovery.load(false).expect("reopen recovery");
        assert_ne!(wire["id"], "migration-board");
        assert_eq!(recovery.image("legacy-image").expect("image").0, b"image");
        assert!(
            read_document(&path, false)
                .expect_err("future version")
                .to_string()
                .contains("Update Brainz")
        );
    }
}
