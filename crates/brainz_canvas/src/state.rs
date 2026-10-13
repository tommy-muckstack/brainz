use crate::{atomic_json, read_json, safe_id};
use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};
use std::{path::Path, sync::Mutex};

static STATE_LOCK: Mutex<()> = Mutex::new(());

fn document(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({"format":"brainz-canvas-ui-state","version":1,"boards":{}}));
    }
    let state = read_json(path, 4 * 1024 * 1024)?;
    ensure!(
        state["format"] == "brainz-canvas-ui-state"
            && state["version"] == 1
            && state["boards"].is_object(),
        "Canvas view preferences could not be read; they have not been changed"
    );
    Ok(state)
}

pub fn validate(viewport: [f64; 3]) -> Result<()> {
    ensure!(
        viewport.iter().all(|number| number.is_finite())
            && viewport[0].abs() <= 100_000_000.
            && viewport[1].abs() <= 100_000_000.
            && (0.05..=10.).contains(&viewport[2]),
        "Invalid canvas view position"
    );
    Ok(())
}

pub fn read(path: &Path, board_id: &str) -> Result<Option<[f64; 3]>> {
    ensure!(safe_id(board_id), "Invalid canvas id");
    let _guard = STATE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Canvas view preferences unavailable"))?;
    let state = document(path)?;
    if state["boards"][board_id].is_null() {
        return Ok(None);
    }
    let viewport = serde_json::from_value(state["boards"][board_id]["viewport"].clone())?;
    validate(viewport)?;
    Ok(Some(viewport))
}

pub fn write(path: &Path, board_id: &str, viewport: [f64; 3]) -> Result<()> {
    ensure!(safe_id(board_id), "Invalid canvas id");
    validate(viewport)?;
    let _guard = STATE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Canvas view preferences unavailable"))?;
    let mut state = document(path)?;
    state["boards"]
        .as_object_mut()
        .context("Missing canvas preferences")?
        .insert(board_id.to_owned(), json!({"viewport":viewport}));
    ensure!(
        serde_json::to_vec(&state)?.len() <= 4 * 1024 * 1024,
        "Canvas view preferences are full"
    );
    atomic_json(path, &state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_machine_views_preserve_other_boards_and_refuse_corrupt_preferences() {
        let root = tempfile::tempdir().expect("local data");
        let path = root.path().join("whiteboards/state.json");
        assert_eq!(read(&path, "first").expect("no state"), None);
        write(&path, "first", [100., -200., 0.75]).expect("save view");
        write(&path, "second", [0., 0., 2.]).expect("second view");
        assert_eq!(
            read(&path, "first").expect("first preserved"),
            Some([100., -200., 0.75])
        );
        assert!(write(&path, "first", [0., 0., f64::NAN]).is_err());
        std::fs::write(&path, b"{broken").expect("corrupt file");
        assert!(write(&path, "first", [0., 0., 1.]).is_err());
        assert_eq!(std::fs::read(&path).expect("untouched"), b"{broken");
    }
}
