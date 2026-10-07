//! Brainz: open loops are the ⏳ (waiting on someone else) and ⏰ (owed by
//! the brain's owner) lines the signals pass finds at the start of bullets
//! in notes. They have no tab of their own: the To-Do tab and the Brief show
//! them as "Flagged in notes", where each one can be moved onto the To-Do
//! board or dismissed. Both edits tick the note's marker to ✅, so the note
//! records that the board now owns it (or that it was let go).

use std::path::Path;

use anyhow::Context as _;

use crate::themes_signals::OpenLoop;

/// The tag every item moved from a note carries on the board.
pub const FROM_NOTE_TAG: &str = "from-note";

/// Turns the loop's ⏳ or ⏰ into ✅ on its line in the note. The line is
/// found by number first, then by the nearest line with the same marker and
/// opening words, since the note may have shifted since the pass ran.
pub fn strike_marker(path: &Path, open_loop: &OpenLoop) -> anyhow::Result<()> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut lines: Vec<&str> = text.split_inclusive('\n').collect();
    let marker = open_loop.marker.as_str();
    let snippet: String = open_loop.text.chars().take(40).collect();
    let matches_line =
        |line: &str| line.contains(marker) && (snippet.is_empty() || line.contains(snippet.trim()));
    let expected = open_loop.line.saturating_sub(1) as usize;
    let row = if lines.get(expected).is_some_and(|line| matches_line(line)) {
        expected
    } else {
        let mut best: Option<usize> = None;
        for (ix, line) in lines.iter().enumerate() {
            if matches_line(line)
                && best.is_none_or(|current| ix.abs_diff(expected) < current.abs_diff(expected))
            {
                best = Some(ix);
            }
        }
        best.ok_or_else(|| anyhow::anyhow!("the line is no longer in {}", open_loop.file))?
    };
    let updated = lines[row].replacen(marker, "✅", 1);
    lines[row] = &updated;
    let joined: String = lines.concat();
    std::fs::write(path, joined).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// The board item for a loop: what the note says, who it waits on, and
/// where it came from, in the board's "what to do + why" wording.
pub fn board_line(open_loop: &OpenLoop, owner: &str) -> String {
    let text = markdown::markdown_to_plain_text(&open_loop.text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let text = text.trim_end_matches(['.', ' ']);
    let who = match (&open_loop.counterparty, open_loop.owed_by_owner()) {
        (Some(person), false) => format!("Waiting on {person}: "),
        (None, false) => "Waiting: ".to_owned(),
        (_, true) => format!("{owner} owes: "),
    };
    let source = open_loop
        .file
        .rsplit('/')
        .next()
        .unwrap_or(&open_loop.file)
        .trim_end_matches(".md");
    format!("- [ ] {who}{text} (from {source}) `{FROM_NOTE_TAG}`")
}

/// Appends the loop to the board: ⏰ lines go under the first section whose
/// name starts with "Today", ⏳ lines under the first starting with
/// "Delayed" or "Waiting". A missing section is added above "## Done" (or
/// at the end).
pub fn add_to_board(todo_path: &Path, open_loop: &OpenLoop, owner: &str) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(todo_path)
        .with_context(|| format!("reading {}", todo_path.display()))?;
    let output = insert_board_line(
        &text,
        &board_line(open_loop, owner),
        open_loop.owed_by_owner(),
    );
    std::fs::write(todo_path, output)
        .with_context(|| format!("writing {}", todo_path.display()))?;
    Ok(())
}

fn insert_board_line(text: &str, line: &str, owed_by_owner: bool) -> String {
    let wanted: &[&str] = if owed_by_owner {
        &["today"]
    } else {
        &["delayed", "waiting"]
    };
    let lines: Vec<&str> = text.lines().collect();
    let heading_at = |ix: usize| {
        lines[ix]
            .strip_prefix("## ")
            .map(|name| name.trim().to_ascii_lowercase())
    };
    let section = (0..lines.len()).find(|&ix| {
        heading_at(ix).is_some_and(|name| wanted.iter().any(|prefix| name.starts_with(prefix)))
    });
    let mut out: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    match section {
        Some(start) => {
            // The last item line of the section, or its heading when empty.
            let end = (start + 1..lines.len())
                .find(|&ix| heading_at(ix).is_some())
                .unwrap_or(lines.len());
            let mut insert_at = start + 1;
            for ix in start + 1..end {
                if lines[ix].trim_start().starts_with("- [") {
                    insert_at = ix + 1;
                }
            }
            if insert_at == start + 1 {
                // Skip a blank line and an italic "nothing here" note right after the heading.
                let mut ix = start + 1;
                while ix < end && lines[ix].trim().is_empty() {
                    ix += 1;
                }
                if ix < end && lines[ix].trim().starts_with('_') {
                    out.remove(ix);
                    insert_at = ix;
                } else {
                    insert_at = ix.min(end);
                }
            }
            out.insert(insert_at, line.to_owned());
        }
        None => {
            let heading = if owed_by_owner {
                "## Today"
            } else {
                "## Delayed / Waiting"
            };
            let done = (0..lines.len())
                .find(|&ix| heading_at(ix).is_some_and(|name| name.starts_with("done")));
            let mut block = vec![
                heading.to_owned(),
                String::new(),
                line.to_owned(),
                String::new(),
            ];
            match done {
                Some(ix) => {
                    if ix > 0 && !lines[ix - 1].trim().is_empty() {
                        block.insert(0, String::new());
                    }
                    for (offset, item) in block.into_iter().enumerate() {
                        out.insert(ix + offset, item);
                    }
                }
                None => {
                    if lines.last().is_some_and(|last| !last.trim().is_empty()) {
                        block.insert(0, String::new());
                    }
                    block.pop();
                    out.extend(block);
                }
            }
        }
    }
    let mut joined = out.join("\n");
    if text.ends_with('\n') || !joined.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiting(text: &str, who: Option<&str>) -> OpenLoop {
        OpenLoop {
            file: "career/employers/tekmetric/CLAUDE.md".into(),
            line: 3,
            text: text.into(),
            marker: "⏳".into(),
            first_seen: "2026-10-01".into(),
            folder: "career".into(),
            counterparty: who.map(str::to_owned),
        }
    }

    #[test]
    fn board_line_says_who_and_where_from() {
        let lp = waiting("Lea to come back with the **counter**.", Some("Lea"));
        assert_eq!(
            board_line(&lp, "Tommy"),
            "- [ ] Waiting on Lea: Lea to come back with the counter (from CLAUDE) `from-note`"
        );
        let mut owed = waiting("send the Armature link", None);
        owed.marker = "⏰".into();
        assert_eq!(
            board_line(&owed, "Tommy"),
            "- [ ] Tommy owes: send the Armature link (from CLAUDE) `from-note`"
        );
    }

    #[test]
    fn insert_board_line_lands_after_the_last_item_of_the_right_section() {
        let board = "# TODO\n\n## Today\n\n- [ ] jersey `a`\n\n## Delayed / Waiting\n\n- [ ] rolls `b`\n- [ ] sticker `c`\n\n## Done\n\n- [x] old `d`\n";
        let out = insert_board_line(board, "- [ ] new `n`", false);
        assert_eq!(
            out,
            "# TODO\n\n## Today\n\n- [ ] jersey `a`\n\n## Delayed / Waiting\n\n- [ ] rolls `b`\n- [ ] sticker `c`\n- [ ] new `n`\n\n## Done\n\n- [x] old `d`\n"
        );
        let out = insert_board_line(board, "- [ ] owed `o`", true);
        assert!(out.contains("## Today\n\n- [ ] jersey `a`\n- [ ] owed `o`\n\n## Delayed"));
    }

    #[test]
    fn insert_board_line_replaces_an_empty_note_and_adds_missing_sections() {
        let board = "## Today\n\n_(nothing today)_\n\n## Done\n";
        let out = insert_board_line(board, "- [ ] owed `o`", true);
        assert_eq!(out, "## Today\n\n- [ ] owed `o`\n\n## Done\n");
        let out = insert_board_line(board, "- [ ] waiting `w`", false);
        assert_eq!(
            out,
            "## Today\n\n_(nothing today)_\n\n## Delayed / Waiting\n\n- [ ] waiting `w`\n\n## Done\n"
        );
    }

    #[test]
    fn strike_marker_ticks_the_nearest_matching_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(
            &path,
            "# Note\n\n- ⏳ Lea to come back with the counter\n- ⏳ other\n",
        )
        .unwrap();
        let mut lp = waiting("Lea to come back with the counter", Some("Lea"));
        lp.line = 9;
        strike_marker(&path, &lp).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# Note\n\n- ✅ Lea to come back with the counter\n- ⏳ other\n"
        );
    }
}
