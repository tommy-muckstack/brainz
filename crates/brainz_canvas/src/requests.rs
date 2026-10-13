use crate::{Board, atomic_json, operations_dir, read_json, safe_id, scoped_path};
use anyhow::{Context as _, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn directory(root: &Path, board: &Board) -> Result<PathBuf> {
    let path = operations_dir(root, board)?
        .parent()
        .context("Missing board folder")?
        .join(".cache/requests");
    let path = scoped_path(root, &path, true)?;
    fs::create_dir_all(&path)?;
    let ignore = scoped_path(root, &path.join(".gitignore"), true)?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(ignore)
    {
        Ok(mut file) => file.write_all(b"*\n")?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    Ok(path)
}

pub fn screenshot(root: &Path, board: &Board, arguments: &Value) -> Result<String> {
    if let Some(bound) = arguments.get("bound") {
        let bound = bound
            .as_array()
            .context("bound needs x, y, width, height")?;
        ensure!(
            bound.len() == 4
                && bound.iter().all(|value| value
                    .as_f64()
                    .is_some_and(|value| value.is_finite() && value.abs() <= 1_000_000.)),
            "Invalid screenshot bounds"
        );
        ensure!(
            bound
                .get(2)
                .and_then(Value::as_f64)
                .is_some_and(|width| width > 0.)
                && bound
                    .get(3)
                    .and_then(Value::as_f64)
                    .is_some_and(|height| height > 0.),
            "Screenshot size must be positive"
        );
    }
    if let Some(ids) = arguments.get("objectIds") {
        let ids = ids.as_array().context("objectIds must be an array")?;
        ensure!(
            !ids.is_empty()
                && ids.len() <= 500
                && ids.iter().all(|id| id
                    .as_str()
                    .is_some_and(|id| !id.is_empty() && id.len() <= 256)),
            "Invalid screenshot object ids"
        );
    }
    ensure!(
        !(arguments.get("bound").is_some() && arguments.get("objectIds").is_some()),
        "Choose bounds or object ids"
    );
    let id = uuid::Uuid::now_v7().to_string();
    atomic_json(
        &directory(root, board)?.join(format!("{id}.json")),
        &json!({"requestId":id,"kind":"screenshot","bound":arguments.get("bound"),"objectIds":arguments.get("objectIds")}),
    )?;
    Ok(id)
}

pub fn context(root: &Path, board: &Board) -> Result<String> {
    let id = uuid::Uuid::now_v7().to_string();
    atomic_json(
        &directory(root, board)?.join(format!("{id}.json")),
        &json!({"requestId":id,"kind":"read"}),
    )?;
    Ok(id)
}

pub fn pending(root: &Path, board: &Board) -> Result<Vec<Value>> {
    let mut requests = Vec::new();
    for entry in fs::read_dir(directory(root, board)?)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file()
            || path.extension().is_none_or(|extension| extension != "json")
        {
            continue;
        }
        let path = scoped_path(root, &path, false)?;
        let Some(id) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|id| safe_id(id))
        else {
            continue;
        };
        let request = match read_json(&path, 16 * 1024) {
            Ok(request)
                if request["requestId"] == id
                    && matches!(request["kind"].as_str(), Some("screenshot" | "read")) =>
            {
                request
            }
            _ => {
                respond(root, board, id, &json!({"error":"Invalid canvas request"}))?;
                continue;
            }
        };
        requests.push(request);
        if requests.len() == 20 {
            break;
        }
    }
    Ok(requests)
}

pub fn respond(root: &Path, board: &Board, id: &str, result: &Value) -> Result<()> {
    ensure!(safe_id(id), "Invalid canvas request id");
    let directory = directory(root, board)?;
    let request = scoped_path(root, &directory.join(format!("{id}.json")), true)?;
    if !request.exists() {
        return Ok(());
    }
    let response = scoped_path(
        root,
        &directory.join("done").join(format!("{id}.json")),
        true,
    )?;
    ensure!(
        serde_json::to_vec(result)?.len() as u64 <= crate::MAX_BOARD_BYTES,
        "Canvas screenshot is too large"
    );
    atomic_json(&response, result)?;
    match fs::remove_file(request) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshots_round_trip_without_poisoning_the_queue_or_leaving_consumed_images() {
        let root = tempfile::tempdir().expect("brain");
        let board = Board {
            id: "board".into(),
            title: "Diagram".into(),
            path: root.path().join("Diagram.brainzboard"),
            updated_at: 0,
        };
        let id = screenshot(root.path(), &board, &json!({"objectIds":["box"]})).expect("request");
        let folder = directory(root.path(), &board).expect("cache");
        fs::write(folder.join("broken.json"), b"{partial").expect("broken request");
        let requests = pending(root.path(), &board).expect("queue survives invalid request");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["requestId"], id);
        let response = json!({"png":"image","bounds":[0,0,300,200]});
        respond(root.path(), &board, &id, &response).expect("capture");
        assert_eq!(wait(root.path(), &board, &id).expect("response"), response);
        assert!(!folder.join("done").join(format!("{id}.json")).exists());
        respond(root.path(), &board, &id, &response).expect("late response ignored");
        assert!(!folder.join("done").join(format!("{id}.json")).exists());
        assert!(
            wait(root.path(), &board, "broken")
                .expect_err("invalid")
                .to_string()
                .contains("Invalid canvas request")
        );
        assert_eq!(
            fs::read_to_string(folder.join(".gitignore")).expect("cache ignored"),
            "*\n"
        );
        assert!(screenshot(root.path(), &board, &json!({"bound":[0,0,-2,10]})).is_err());
        assert!(
            screenshot(
                root.path(),
                &board,
                &json!({"bound":[0,0,2,10],"objectIds":["box"]})
            )
            .is_err()
        );
    }
}

pub fn wait(root: &Path, board: &Board, id: &str) -> Result<Value> {
    wait_for(root,board,id,Duration::from_secs(5))?.context("Open this canvas in Brainz with Canvas assistant enabled in Labs, then request its screenshot again")
}

pub fn wait_for(root: &Path, board: &Board, id: &str, timeout: Duration) -> Result<Option<Value>> {
    ensure!(safe_id(id), "Invalid canvas request id");
    let path = scoped_path(
        root,
        &directory(root, board)?
            .join("done")
            .join(format!("{id}.json")),
        true,
    )?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            let response = read_json(&path, crate::MAX_BOARD_BYTES)?;
            fs::remove_file(&path)?;
            if let Some(error) = response["error"].as_str() {
                bail!("{error}");
            }
            return Ok(Some(response));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let request = scoped_path(
        root,
        &directory(root, board)?.join(format!("{id}.json")),
        true,
    )?;
    match fs::remove_file(request) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(None)
}
