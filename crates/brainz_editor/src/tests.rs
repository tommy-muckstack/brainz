use super::*;

fn apply(source: &str, plan: EditPlan) -> String {
    let edits = plan.edits_for(source).expect("matching snapshot");
    let mut result = source.to_owned();
    for edit in edits.iter().rev() {
        result.replace_range(edit.range.clone(), &edit.replacement);
    }
    result
}

#[test]
fn synthetic_corpus_preserves_every_source_byte_and_nested_range() {
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut paths = std::fs::read_dir(corpus)
        .expect("synthetic fixtures")
        .map(|entry| entry.expect("fixture entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect::<Vec<_>>();
    paths.sort();
    assert_eq!(paths.len(), 40);
    for path in paths {
        let original = std::fs::read_to_string(&path).expect("fixture text");
        let map = BlockMap::parse(original.clone());
        assert_eq!(
            map.source_segments().collect::<String>().as_bytes(),
            original.as_bytes(),
            "{path:?}"
        );
        for (index, block) in map.blocks().iter().enumerate() {
            assert!(map.text(index).is_some(), "{path:?}: {block:?}");
            for child in &block.children {
                let child = map.blocks().get(*child).expect("child exists");
                assert_eq!(child.parent, Some(index));
                assert!(
                    child.range.start >= block.range.start,
                    "{path:?}: {child:?}"
                );
                assert!(child.range.end <= block.range.end, "{path:?}: {child:?}");
            }
        }
        let insertion = original.len();
        let edited = apply(
            &original,
            map.replace(insertion..insertion, "hello").expect("insert"),
        );
        assert_eq!(
            edited.as_bytes(),
            format!("{original}hello").as_bytes(),
            "{path:?}"
        );
    }
}

#[test]
fn prose_edits_preserve_markers_metadata_and_line_endings() {
    let source = "---\r\nstatus: draft\r\n---\r\n\r\n# Plan\r\n\r\n* Keep __this__ and [that][ref].  \r\n\r\n[ref]: /relative/path 'Title'\r\n";
    let map = BlockMap::parse(source);
    let start = source.find("Keep").expect("prose position");
    assert_eq!(
        apply(source, map.replace(start..start, "hello").expect("insert")),
        source.replacen("Keep", "helloKeep", 1)
    );
    let start = source.find("draft").expect("metadata value");
    assert_eq!(
        apply(
            source,
            map.replace(start..start + 5, "ready").expect("replace")
        ),
        source.replacen("status: draft", "status: ready", 1)
    );
}

#[test]
fn bold_adds_and_removes_only_the_selected_delimiters() {
    let source = "Keep _this_ and __that__; word and `code`.\n";
    let start = source.find("word").expect("word");
    let bold = apply(
        source,
        BlockMap::parse(source)
            .toggle_bold(start..start + 4)
            .expect("bold"),
    );
    assert_eq!(bold, "Keep _this_ and __that__; **word** and `code`.\n");
    let plain = apply(
        &bold,
        BlockMap::parse(bold.clone())
            .toggle_bold(start + 2..start + 6)
            .expect("plain"),
    );
    assert_eq!(plain, source);
}

#[test]
fn todo_title_edits_keep_the_tag_and_note_syntax() {
    let source =
        "## Later\n\n- [ ] Draft plan `writing` (next week)\n- [x] Keep done `admin` (yesterday)\n";
    let start = source.find("Draft plan").expect("title");
    let edited = apply(
        source,
        BlockMap::parse(source)
            .replace(start..start + 10, "Review plan")
            .expect("edit title"),
    );
    assert_eq!(
        edited,
        "## Later\n\n- [ ] Review plan `writing` (next week)\n- [x] Keep done `admin` (yesterday)\n"
    );
}

#[test]
fn source_changes_and_partial_unicode_ranges_are_rejected() {
    let source = "A 🧠 note\n";
    let map = BlockMap::parse(source);
    assert!(matches!(
        map.replace(3..4, "x"),
        Err(EditError::InvalidRange)
    ));
    assert!(matches!(
        map.replace(8..99, "x"),
        Err(EditError::InvalidRange)
    ));
    assert!(matches!(
        map.toggle_bold(2..2),
        Err(EditError::InvalidRange)
    ));
    let edit = map.replace(0..1, "The").expect("valid edit");
    assert_eq!(
        edit.edits_for("A changed 🧠 note\n"),
        Err(EditError::ChangedSource)
    );
    assert_eq!(
        edit.edits_for("A 🧠 note\nExternal paragraph\n"),
        Err(EditError::ChangedSource)
    );
    assert_eq!(map.source(), source);
}

#[test]
fn nested_list_items_and_structured_blocks_keep_source_ranges() {
    let source = "---\ntitle: Plan\n---\n\n# Plan\n\n- Parent\n  - Child\n    - Grandchild\n- Sibling\n\n```mermaid\nA-->B\n```\n\n| A | B |\n| - | - |\n| 1 | 2 |\n";
    let map = BlockMap::parse(source);
    let child_offset = source.find("Grandchild").expect("nested item");
    let item = map.deepest_block_at(child_offset).expect("block");
    assert_eq!(map.blocks()[item].kind, BlockKind::Item);
    assert!(map.text(item).expect("source item").contains("Grandchild"));
    assert!(
        map.blocks()
            .iter()
            .any(|block| block.kind == BlockKind::Metadata)
    );
    assert!(
        map.blocks()
            .iter()
            .any(|block| block.kind == BlockKind::Code(Some("mermaid".into())))
    );
    assert!(
        map.blocks()
            .iter()
            .any(|block| block.kind == BlockKind::Table)
    );
    assert!(
        map.blocks()
            .iter()
            .any(|block| block.kind == BlockKind::Heading(1))
    );
    assert_eq!(map.deepest_block_at(source.len()), None);
}

#[test]
fn five_thousand_lines_keep_their_original_source() {
    let source = (0..2500)
        .map(|index| format!("Paragraph {index} with **formatting**.\n\n"))
        .collect::<String>();
    let map = BlockMap::parse(source.clone());
    assert_eq!(map.roots().len(), 2500);
    assert_eq!(map.source_segments().collect::<String>(), source);
}
