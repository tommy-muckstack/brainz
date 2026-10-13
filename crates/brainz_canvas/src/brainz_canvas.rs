use anyhow::{Context as _, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead as _, BufReader, Read as _, Write as _},
    path::{Component, Path, PathBuf},
};

pub mod mcp;
pub mod requests;
pub mod state;
pub mod storage;

pub const MAX_BATCH_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_BOARD_BYTES: u64 = 24 * 1024 * 1024;

pub fn document_hash(document: &Value) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(document)?)
    ))
}

pub fn legacy_board_id(path: &Path) -> String {
    format!(
        "legacy-{:x}",
        Sha256::digest(path.as_os_str().as_encoded_bytes())
    )
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub id: String,
    pub title: String,
    pub path: PathBuf,
    pub updated_at: u64,
}

pub fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

pub fn scoped_path(root: &Path, value: &Path, allow_missing: bool) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .context("The brain folder does not exist")?;
    ensure!(
        !value
            .components()
            .any(|component| matches!(component, Component::ParentDir)),
        "Paths must stay inside this brain"
    );
    let path = if value.is_absolute() {
        value.to_path_buf()
    } else {
        root.join(value)
    };
    let mut ancestor = path.as_path();
    let mut suffix = Vec::new();
    let mut retries = 0;
    loop {
        match ancestor.canonicalize() {
            Ok(resolved) => {
                ensure!(
                    resolved.starts_with(&root),
                    "Paths must stay inside this brain"
                );
                return Ok(suffix
                    .iter()
                    .rev()
                    .fold(resolved, |path, name| path.join(name)));
            }
            Err(error) if allow_missing && error.kind() == std::io::ErrorKind::NotFound => {
                // A request writer may create this directory between the two
                // lookups. Re-resolve ordinary entries; only symlinks are unsafe
                // to treat as missing, including links with absent targets.
                match fs::symlink_metadata(ancestor) {
                    Ok(metadata) => {
                        ensure!(
                            !metadata.file_type().is_symlink(),
                            "Unresolved symbolic link"
                        );
                        ensure!(
                            retries < 8,
                            "Path changed repeatedly while resolving it; retry"
                        );
                        retries += 1;
                        continue;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                retries = 0;
                suffix.push(ancestor.file_name().context("Invalid path")?.to_os_string());
                ancestor = ancestor.parent().context("Invalid path")?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub fn read_json(path: &Path, maximum: u64) -> Result<Value> {
    let file = File::open(path)?;
    ensure!(file.metadata()?.len() <= maximum, "File is too large");
    let mut bytes = Vec::new();
    file.take(maximum + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= maximum, "File is too large");
    serde_json::from_slice(&bytes).context("Invalid JSON file")
}

pub fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    let parent = path.parent().context("File has no containing folder")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temporary, value)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub fn board(root: &Path, path: &Path) -> Result<Board> {
    let path = scoped_path(root, path, false)?;
    ensure!(
        path.extension()
            .is_some_and(|extension| extension == "brainzboard"),
        "Choose a .brainzboard file"
    );
    let document = read_json(&path, MAX_BOARD_BYTES)?;
    ensure!(
        document["format"] == "brainz-whiteboard",
        "This is not a Brainz canvas"
    );
    let id = document["id"]
        .as_str()
        .filter(|value| safe_id(value))
        .map(str::to_owned)
        .unwrap_or_else(|| legacy_board_id(&path));
    Ok(Board {
        id,
        title: document["title"]
            .as_str()
            .unwrap_or("Untitled canvas")
            .to_owned(),
        updated_at: fs::metadata(&path)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        path,
    })
}

pub fn list_boards(root: &Path) -> Result<Vec<Board>> {
    let root = root.canonicalize()?;
    let mut boards = Vec::new();
    for entry in walkdir::WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_dir()
                || !matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "node_modules" | "target" | ".cache")
                )
        })
    {
        let entry = entry?;
        if entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "brainzboard")
        {
            boards.push(
                board(&root, entry.path())
                    .with_context(|| format!("Cannot read canvas {}", entry.path().display()))?,
            );
        }
    }
    boards.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(boards)
}

pub fn resolve_board(root: &Path, selector: Option<&str>) -> Result<Board> {
    if let Some(selector) = selector {
        let path = Path::new(selector);
        if path
            .extension()
            .is_some_and(|extension| extension == "brainzboard")
        {
            return board(root, path);
        }
    }
    let matches: Vec<_> = list_boards(root)?
        .into_iter()
        .filter(|board| {
            selector.is_none_or(|selector| board.id == selector || board.title == selector)
        })
        .collect();
    ensure!(
        matches.len() == 1,
        "Specify a canvas path or id; the selection matched {} canvases",
        matches.len()
    );
    matches.into_iter().next().context("Canvas not found")
}

pub fn operations_dir(root: &Path, board: &Board) -> Result<PathBuf> {
    ensure!(safe_id(&board.id), "Invalid canvas id");
    let path = scoped_path(
        root,
        &PathBuf::from(".brainz/boards").join(&board.id).join("ops"),
        true,
    )?;
    fs::create_dir_all(&path)?;
    scoped_path(root, &path, false)
}

pub fn validate_batch(root: &Path, batch: &Value) -> Result<String> {
    ensure!(
        serde_json::to_vec(batch)?.len() as u64 <= MAX_BATCH_BYTES,
        "Batch is too large"
    );
    ensure!(
        batch["format"] == "brainz-canvas-ops" && batch["version"] == 1,
        "Expected brainz-canvas-ops version 1"
    );
    let id = batch["batchId"].as_str().context("Missing batchId")?;
    ensure!(safe_id(id), "Invalid batchId");
    let operations = batch["ops"].as_array().context("Missing ops array")?;
    ensure!(
        !operations.is_empty() && operations.len() <= 500,
        "Use 1–500 canvas operations per batch"
    );
    for operation in operations {
        for candidate in [Some(operation), operation.get("set")]
            .into_iter()
            .flatten()
        {
            if let Some(path) = candidate.get("path").and_then(Value::as_str) {
                scoped_path(root, Path::new(path), true)?;
            }
            if let Some(value) = candidate.get("url").and_then(Value::as_str) {
                let url = url::Url::parse(value).context("Invalid canvas link")?;
                match url.scheme() {
                    "file" => {
                        scoped_path(
                            root,
                            &url.to_file_path()
                                .map_err(|()| anyhow::anyhow!("Invalid file link"))?,
                            true,
                        )?;
                    }
                    "brainz" => {
                        let path = url
                            .query_pairs()
                            .find(|(key, _)| key == "path")
                            .map(|(_, value)| value.into_owned())
                            .context("Brainz links need a path inside this brain")?;
                        scoped_path(root, Path::new(&path), true)?;
                    }
                    "https" | "http" | "myman" | "mailto" => {}
                    _ => bail!("Unsupported canvas link"),
                }
            }
        }
    }
    Ok(id.to_owned())
}

pub fn queue_batch(root: &Path, board: &Board, batch: &Value) -> Result<Value> {
    let id = validate_batch(root, batch)?;
    let directory = operations_dir(root, board)?;
    let path = scoped_path(root, &directory.join(format!("{id}.json")), true)?;
    let archived = scoped_path(
        root,
        &directory.join("done").join(format!("{id}.request.json")),
        true,
    )?;
    if archived.exists() {
        ensure!(
            read_json(&archived, MAX_BATCH_BYTES)? == *batch,
            "This batchId is already used by a different proposal"
        );
        return batch_status(root, board, &id)?.context("The archived batch has no result");
    }
    if path.exists() {
        ensure!(
            read_json(&path, MAX_BATCH_BYTES)? == *batch,
            "This batchId is already used by a different proposal"
        );
    } else {
        let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
        serde_json::to_writer(&mut temporary, batch)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(&path)
            .map_err(|error| error.error)?;
    }
    Ok(batch_status(root, board, &id)?.unwrap_or_else(|| json!({"batchId":id,"status":"queued","refs":{},"ops":[],"message":"Open this canvas in Brainz with Canvas assistant enabled in Labs to review the proposal."})))
}

pub fn pending_batches(root: &Path, board: &Board) -> Result<Vec<Value>> {
    let directory = operations_dir(root, board)?;
    let mut paths = Vec::new();
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    let mut batches = Vec::new();
    for path in paths {
        if batches.len() >= 20 {
            break;
        }
        let path = scoped_path(root, &path, false)?;
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .context("Invalid batch filename")?;
        if !safe_id(id) {
            continue;
        }
        if let Some(result) = batch_status(root, board, id)?
            && result["status"]
                .as_str()
                .is_some_and(|status| matches!(status, "applied" | "discarded" | "rejected"))
        {
            continue;
        }
        let batch = read_json(&path, MAX_BATCH_BYTES).and_then(|batch| {
            ensure!(
                validate_batch(root, &batch)? == id,
                "Batch filename must match batchId"
            );
            Ok(batch)
        });
        match batch {
            Ok(batch) => batches.push(batch),
            Err(error) => save_result(
                root,
                board,
                id,
                &json!({"batchId":id,"status":"rejected","refs":{},"ops":[],"message":error.to_string()}),
            )?,
        }
    }
    Ok(batches)
}

pub fn batch_status(root: &Path, board: &Board, id: &str) -> Result<Option<Value>> {
    ensure!(safe_id(id), "Invalid batchId");
    let path = scoped_path(
        root,
        &operations_dir(root, board)?
            .join("done")
            .join(format!("{id}.result.json")),
        true,
    )?;
    if path.exists() {
        Ok(Some(read_json(&path, MAX_BATCH_BYTES)?))
    } else if scoped_path(
        root,
        &operations_dir(root, board)?.join(format!("{id}.json")),
        true,
    )?
    .is_file()
    {
        Ok(Some(
            json!({"batchId":id,"status":"queued","refs":{},"ops":[]}),
        ))
    } else {
        Ok(None)
    }
}

pub fn save_result(root: &Path, board: &Board, id: &str, result: &Value) -> Result<()> {
    ensure!(
        safe_id(id) && result["batchId"] == id,
        "Invalid batch result"
    );
    ensure!(
        serde_json::to_vec(result)?.len() as u64 <= MAX_BATCH_BYTES,
        "Batch result is too large"
    );
    let path = scoped_path(
        root,
        &operations_dir(root, board)?
            .join("done")
            .join(format!("{id}.result.json")),
        true,
    )?;
    atomic_json(&path, result)?;
    if result["status"]
        .as_str()
        .is_some_and(|status| matches!(status, "applied" | "discarded" | "rejected"))
    {
        let request = scoped_path(
            root,
            &operations_dir(root, board)?.join(format!("{id}.json")),
            true,
        )?;
        if request.exists() {
            let archived = scoped_path(
                root,
                &operations_dir(root, board)?
                    .join("done")
                    .join(format!("{id}.request.json")),
                true,
            )?;
            fs::rename(request, archived)?;
        }
    }
    Ok(())
}

pub fn read_context(root: &Path, board: &Board, scope: &str) -> Result<Value> {
    ensure!(
        matches!(scope, "all" | "view" | "selection"),
        "Use all, view, or selection scope"
    );
    let request = requests::context(root, board)?;
    let mut context = if let Some(response) =
        requests::wait_for(root, board, &request, std::time::Duration::from_millis(700))?
    {
        let mut context = response
            .get("context")
            .cloned()
            .context("Missing live canvas context")?;
        context
            .as_object_mut()
            .context("Invalid canvas context")?
            .insert("freshness".into(), json!("live"));
        context
    } else {
        let mut context = read_saved_context(root, board)?;
        context
            .as_object_mut()
            .context("Invalid canvas context")?
            .insert("freshness".into(), json!("saved"));
        context
    };
    scope_context(&mut context, scope)?;
    Ok(context)
}

fn read_saved_context(root: &Path, board: &Board) -> Result<Value> {
    let incremental = scoped_path(
        root,
        &PathBuf::from(".brainz/boards")
            .join(&board.id)
            .join("canvas.json"),
        true,
    )?;
    if incremental.is_file() && read_json(&board.path, MAX_BOARD_BYTES)?["version"] == 2 {
        return read_json(&incremental, MAX_BOARD_BYTES);
    }
    let path = scoped_path(root, &board.path.with_extension("canvas.json"), true)?;
    Ok(if path.exists() {
        read_json(&path, MAX_BOARD_BYTES)?
    } else {
        read_json(&board.path, MAX_BOARD_BYTES)?
            .get("scene")
            .cloned()
            .context("Open this canvas in Brainz to generate its readable context")?
    })
}

fn scope_context(context: &mut Value, scope: &str) -> Result<()> {
    if scope != "all" {
        ensure!(
            context["version"] == 2,
            "Open this canvas with Canvas assistant enabled to update its context"
        );
        if scope == "selection" {
            let selection = context["selection"].as_array().cloned().unwrap_or_default();
            let objects = context["objects"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|object| selection.contains(&object["id"]))
                .collect::<Vec<_>>();
            context["inView"] = json!(objects);
        }
        context
            .as_object_mut()
            .context("Invalid canvas context")?
            .remove("objects");
    }
    Ok(())
}

pub fn append_event(root: &Path, board: &Board, event: &Value) -> Result<()> {
    let path = scoped_path(
        root,
        &operations_dir(root, board)?
            .parent()
            .context("Missing board folder")?
            .join("events.jsonl"),
        true,
    )?;
    let bytes = serde_json::to_vec(event)?;
    ensure!(bytes.len() <= 64 * 1024, "Canvas event is too large");
    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(scoped_path(root, &lock_path, true)?)?;
    lock.lock()?;
    let sequence_path = path.with_extension("seq");
    let sequence_path = scoped_path(root, &sequence_path, true)?;
    let sequence = if sequence_path.exists() {
        read_json(&sequence_path, 256)?
            .as_u64()
            .context("Invalid event sequence")?
            + 1
    } else {
        1
    };
    if path.exists() && fs::metadata(&path)?.len() > 5 * 1024 * 1024 {
        fs::rename(&path, path.with_extension("previous.jsonl"))?;
    }
    let mut event = event.clone();
    event["seq"] = json!(sequence);
    let mut output = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut output, &event)?;
    output.write_all(b"\n")?;
    atomic_json(&sequence_path, &json!(sequence))?;
    Ok(())
}

pub fn events(root: &Path, board: &Board, since: u64) -> Result<Vec<Value>> {
    let parent = operations_dir(root, board)?
        .parent()
        .context("Missing board folder")?
        .to_path_buf();
    let mut events = Vec::new();
    for name in ["events.previous.jsonl", "events.jsonl"] {
        let path = scoped_path(root, &parent.join(name), true)?;
        if !path.exists() {
            continue;
        }
        for line in BufReader::new(File::open(path)?).lines() {
            let line = line?;
            let event: Value = serde_json::from_str(&line)?;
            if event["seq"]
                .as_u64()
                .is_some_and(|sequence| sequence > since)
            {
                events.push(event);
            }
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Board) {
        let root = tempfile::tempdir().expect("workspace");
        let path = root.path().join("Projects/Plan.brainzboard");
        atomic_json(&path, &json!({"format":"brainz-whiteboard","version":1,"id":"board-one","title":"Plan","data":"AAA="})).expect("fixture");
        let board = board(root.path(), &path).expect("board");
        (root, board)
    }

    #[test]
    fn operations_are_queued_idempotently_and_remain_reviewable_until_a_decision() {
        let (root, board) = fixture();
        let batch = json!({"format":"brainz-canvas-ops","version":1,"batchId":"test-one","actor":"agent:claude","ops":[{"op":"create","type":"shape","text":"Idea"}]});
        assert_eq!(
            queue_batch(root.path(), &board, &batch).expect("queue")["status"],
            "queued"
        );
        assert_eq!(
            pending_batches(root.path(), &board).expect("pending"),
            vec![batch.clone()]
        );
        let staged = json!({"batchId":"test-one","status":"staged"});
        save_result(root.path(), &board, "test-one", &staged).expect("stage");
        assert_eq!(
            queue_batch(root.path(), &board, &batch).expect("retry"),
            staged
        );
        assert_eq!(
            pending_batches(root.path(), &board).expect("restart").len(),
            1
        );
        let applied = json!({"batchId":"test-one","status":"applied"});
        save_result(root.path(), &board, "test-one", &applied).expect("apply");
        assert!(
            pending_batches(root.path(), &board)
                .expect("pending")
                .is_empty()
        );
        let mut different = batch;
        different["ops"][0]["text"] = json!("Changed");
        assert!(queue_batch(root.path(), &board, &different).is_err());
    }

    #[test]
    fn paths_and_agent_links_cannot_escape_through_traversal_or_symlinks() {
        let (root, _) = fixture();
        assert!(scoped_path(root.path(), Path::new("../private"), true).is_err());
        assert!(scoped_path(root.path(), Path::new("Projects/new.md"), true).is_ok());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().expect("outside");
            std::os::unix::fs::symlink(outside.path(), root.path().join("shortcut"))
                .expect("symlink");
            assert!(scoped_path(root.path(), Path::new("shortcut/new.md"), true).is_err());
            std::os::unix::fs::symlink(outside.path().join("absent"), root.path().join("dangling"))
                .expect("dangling symlink");
            assert!(scoped_path(root.path(), Path::new("dangling/new.md"), true).is_err());
            let url =
                url::Url::from_file_path(root.path().join("shortcut/private.md")).expect("url");
            let batch = json!({"format":"brainz-canvas-ops","version":1,"batchId":"bad-link","ops":[{"op":"create","type":"card","url":url.as_str()}]});
            assert!(validate_batch(root.path(), &batch).is_err());
        }
    }

    #[test]
    fn missing_path_resolution_tolerates_concurrent_directory_creation() {
        let root = tempfile::tempdir().expect("brain");
        let writer_root = root.path().to_path_buf();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let writer_barrier = barrier.clone();
        let writer = std::thread::spawn(move || {
            for index in 0..200 {
                writer_barrier.wait();
                fs::create_dir_all(writer_root.join(format!("board-{index}/cache/requests/done")))
                    .expect("create request folders");
            }
        });
        for index in 0..200 {
            barrier.wait();
            let relative = PathBuf::from(format!("board-{index}/cache/requests/done/read.json"));
            let resolved = scoped_path(root.path(), &relative, true).expect("concurrent path");
            assert!(resolved.starts_with(root.path().canonicalize().expect("brain root")));
        }
        writer.join().expect("writer");
    }

    #[test]
    fn context_reads_the_live_selection_and_marks_closed_board_fallbacks() {
        let (root, board) = fixture();
        let response_root = root.path().to_path_buf();
        let response_board = board.clone();
        let responder = std::thread::spawn(move || {
            for _ in 0..100 {
                let pending = requests::pending(&response_root, &response_board).expect("requests");
                if let Some(request) = pending.first() {
                    assert_eq!(request["kind"], "read");
                    requests::respond(&response_root,&response_board,request["requestId"].as_str().expect("request id"),&json!({"context":{"version":2,"selection":["chosen"],"viewport":{"zoom":0.5},"objects":[{"id":"chosen","text":"Live text"},{"id":"outside"}],"inView":[]}})).expect("live response");
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("No canvas request arrived");
        });
        let live = read_context(root.path(), &board, "selection").expect("live selection");
        responder.join().expect("responder");
        assert_eq!(live["freshness"], "live");
        assert_eq!(live["viewport"]["zoom"], 0.5);
        assert_eq!(live["inView"], json!([{"id":"chosen","text":"Live text"}]));
        assert!(live.get("objects").is_none());
        atomic_json(
            &board.path.with_extension("canvas.json"),
            &json!({"version":2,"objects":[{"id":"saved"}]}),
        )
        .expect("saved context");
        let saved = read_context(root.path(), &board, "all").expect("closed fallback");
        assert_eq!(saved["freshness"], "saved");
        assert_eq!(saved["objects"][0]["id"], "saved");
        assert!(
            requests::pending(root.path(), &board)
                .expect("expired request removed")
                .is_empty()
        );
    }

    #[test]
    fn bad_batch_files_receive_an_error_without_blocking_the_inbox() {
        let (root, board) = fixture();
        let directory = operations_dir(root.path(), &board).expect("inbox");
        fs::write(directory.join("broken.json"), b"{partial").expect("bad file");
        assert!(
            pending_batches(root.path(), &board)
                .expect("scan")
                .is_empty()
        );
        assert_eq!(
            batch_status(root.path(), &board, "broken")
                .expect("status")
                .expect("result")["status"],
            "rejected"
        );
    }

    #[test]
    fn registry_uses_existing_folders_and_events_have_monotonic_sequences() {
        let (root, board) = fixture();
        let original = fs::read(&board.path).expect("original");
        let boards = list_boards(root.path()).expect("list");
        assert_eq!(boards.len(), 1);
        assert_eq!(boards[0].path, board.path);
        append_event(
            root.path(),
            &board,
            &json!({"actor":"user","kind":"updated"}),
        )
        .expect("event");
        append_event(
            root.path(),
            &board,
            &json!({"actor":"agent:claude","kind":"batch"}),
        )
        .expect("event");
        let recent = events(root.path(), &board, 1).expect("events");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0]["seq"], 2);
        assert_eq!(fs::read(&board.path).expect("unchanged"), original);
    }
}
