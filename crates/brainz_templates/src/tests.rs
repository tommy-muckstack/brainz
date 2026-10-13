use super::*;

fn request() -> RenderRequest {
    RenderRequest {
        now: DateTime::parse_from_rfc3339("2026-10-12T09:30:00-04:00").expect("fixed date"),
        run_id: Uuid::nil(),
        title: "Planning & review".into(),
        folder: "projects/demo".into(),
        input: serde_json::from_value(json!({"title":"A safer launch","goal":"Make the workflow clear","person":{"name":"Alex"},"start":"2026-10-15","end":"2026-10-20","destination":"Montréal","app":"Brainz"})).expect("inputs"),
        context: serde_json::from_value(json!({
            "owner":"Alex", "selection":"A & B < C", "event":{"title":"Product sync","start":"2026-10-12T14:00:00-04:00","end":"2026-10-12T15:00:00-04:00","calendar":"Work","attendee_names":["Alex","Sam"]},
            "match":{"folder":"projects/demo","folder_name":"Demo","who":"Sam","prep_file":"projects/demo/prep.md"},
            "people":[{"name":"Sam","file":"people/sam.md"}], "calendar":{"today":[],"next_week":[]}, "stale":[], "related":[], "last_meeting":null,"myman":{"last_transcript":null}, "todo":{"open":[]}, "git":{"changes":[]}
        })).expect("context"),
        ..RenderRequest::default()
    }
}

fn template(body: &str) -> Template {
    parse(
        &format!("---\nname: Example\nmodes: [new, insert, append, section]\n---\n{body}"),
        "example",
        TemplateSource::Brain,
    )
    .expect("template")
}

#[test]
fn creation_is_complete_unique_and_daily_notes_are_never_overwritten() {
    let root = tempfile::tempdir().expect("brain");
    let mut rendered =
        render(&template("# Test\n\n{{cursor}}Content"), &request()).expect("render");
    rendered.path = Some("existing/folder/note.md".into());
    let first = create_note(root.path(), &rendered, CreatePolicy::Unique).expect("create");
    assert!(first.created);
    assert_eq!(
        fs::read_to_string(root.path().join(&first.path)).expect("note"),
        rendered.body
    );
    fs::write(root.path().join(&first.path), "My edited note").expect("edit");
    let daily = create_note(root.path(), &rendered, CreatePolicy::OpenExisting).expect("daily");
    assert!(!daily.created);
    assert_eq!(daily.cursor, None);
    assert_eq!(
        fs::read_to_string(root.path().join(&daily.path)).expect("note"),
        "My edited note"
    );
    let paths = std::thread::scope(|scope| {
        let tasks = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    create_note(root.path(), &rendered, CreatePolicy::Unique)
                        .expect("concurrent create")
                })
            })
            .collect::<Vec<_>>();
        tasks
            .into_iter()
            .map(|task| task.join().expect("thread").path)
            .collect::<std::collections::BTreeSet<_>>()
    });
    assert_eq!(paths.len(), 8);
    for path in paths {
        assert_ne!(path, first.path);
        assert_eq!(
            fs::read_to_string(root.path().join(path)).expect("complete note"),
            rendered.body
        );
    }
    rendered.path = Some("../outside.md".into());
    assert!(create_note(root.path(), &rendered, CreatePolicy::Unique).is_err());
    rendered.path = Some("commands.sh".into());
    assert!(create_note(root.path(), &rendered, CreatePolicy::Unique).is_err());
}

#[test]
fn builtins_render_offline_with_fixed_context() {
    let registry = Registry::builtin().expect("built-in templates");
    assert_eq!(registry.templates.len(), 10);
    for (id, template) in &registry.templates {
        let rendered =
            render(template, &request()).unwrap_or_else(|error| panic!("{id}: {error:#}"));
        assert!(rendered.path.is_some(), "{id}");
        assert!(!rendered.body.contains("{{"), "{id}");
        assert!(
            rendered
                .cursor
                .is_some_and(|cursor| rendered.body.is_char_boundary(cursor)),
            "{id}"
        );
        assert_eq!(rendered.content_guard, CONTENT_GUARD);
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rendered")
            .join(format!("{id}.md"));
        if std::env::var_os("BRAINZ_UPDATE_TEMPLATE_FIXTURES").is_some() {
            fs::create_dir_all(fixture.parent().expect("fixture folder")).expect("fixture folder");
            fs::write(&fixture, &rendered.body).expect("write reviewed fixture");
        }
        assert_eq!(
            rendered.body,
            fs::read_to_string(fixture).expect("reviewed render fixture"),
            "{id}"
        );
        for block in rendered.ai_blocks {
            assert!(rendered.body.contains(&format!("id={}", block.id)));
        }
    }
}

#[test]
fn calendar_inputs_override_context_events() {
    let registry = Registry::builtin().expect("templates");
    let mut request = request();
    request.context.insert("calendar".into(), json!({"current_or_next":{"title":"Chosen call","start":"2026-10-12T16:00:00-04:00","end":"2026-10-12T17:00:00-04:00","attendee_names":[]}}));
    let rendered = render(&registry.templates["meeting"], &request).expect("render");
    assert!(rendered.body.starts_with("# Chosen call .. 2026-10-12\n"));
    assert!(rendered.body.contains("4:00 PM to 5:00 PM"));
}

#[test]
fn body_helpers_are_markdown_and_cursor_is_a_byte_offset() {
    let rendered = render(&template("# {{title}}\n\n## Context\n{{selection}}\nRésumé {{cursor}}\n{{slug title}} | {{upper owner}} | {{date \"+14d\"}}\n"), &request()).expect("render");
    assert_eq!(
        rendered.body,
        "# Planning & review\n\n## Context\nA & B < C\nRésumé \nplanning-review | ALEX | 2026-10-26\n"
    );
    assert_eq!(rendered.cursor, rendered.body.find("\nplanning-review"));
    assert!(rendered.report.missing.is_empty());
}

#[test]
fn missing_values_are_collected_and_optional_values_can_be_defaulted() {
    let rendered = render(&template("{{missing.name}} {{missing.name}} {{upper absent}} {{default optional \"Fallback\"}} {{#if absent}}hidden{{/if}}"), &request()).expect("render");
    assert_eq!(rendered.report.missing, ["absent", "missing.name"]);
    assert_eq!(rendered.body, "   Fallback ");
}

#[test]
fn required_provider_defaults_do_not_become_literal_text() {
    let source = "---\nname: Event\ninputs:\n  - {id: event, type: calendar.event, required: true, default: calendar.current_or_next}\n---\n{{input.event.title}}";
    let rendered = render(
        &parse(source, "event", TemplateSource::Brain).expect("template"),
        &RenderRequest::default(),
    )
    .expect("render");
    assert!(rendered.report.missing.contains(&"input.event".into()));
    assert!(!rendered.body.contains("calendar.current_or_next"));
}

#[test]
fn ai_sections_are_inert_and_keep_their_instructions_separate() {
    let rendered = render(&template("{{#ai \"agenda\" context=\"event,related\" words=80 format=\"bullets\"}}Summarize {{owner}}'s goals.{{/ai}}"), &request()).expect("render");
    assert_eq!(rendered.ai_blocks.len(), 1);
    assert_eq!(rendered.ai_blocks[0].instruction, "Summarize Alex's goals.");
    assert_eq!(rendered.ai_blocks[0].context, ["event", "related"]);
    assert_eq!(
        rendered.body,
        "<!-- brainz:ai id=agenda run=00000000-0000-0000-0000-000000000000 -->\n✦ AI draft: agenda · Set up a model\n<!-- /brainz:ai -->"
    );
    assert!(
        render(
            &template("{{#ai \"a\"}}one{{/ai}}{{#ai \"a\"}}two{{/ai}}"),
            &request()
        )
        .is_err()
    );
    assert!(render(&template("{{#ai \"bad-->\"}}x{{/ai}}"), &request()).is_err());
}

#[test]
fn insertion_omits_only_the_leading_title_and_frontmatter() {
    let source = "---\nname: Note\nmodes: [new, insert, append]\nnote_frontmatter: {title: '{{title}}'}\n---\n# {{title}}\n\n## Body\n{{cursor}}\n```md\n# Keep this example\n```\n";
    let template = parse(source, "note", TemplateSource::Brain).expect("template");
    let mut request = request();
    let created = render(&template, &request).expect("new");
    assert!(
        created
            .body
            .starts_with("---\ntitle: Planning & review\n---\n# Planning")
    );
    for mode in [InsertMode::Insert, InsertMode::Append] {
        request.mode = mode;
        let rendered = render(&template, &request).expect("insert");
        assert_eq!(
            rendered.body,
            "## Body\n\n```md\n# Keep this example\n```\n"
        );
        assert_eq!(rendered.cursor, Some("## Body\n".len()));
    }
    request.mode = InsertMode::Section;
    assert!(render(&template, &request).is_err());
}

#[test]
fn links_are_relative_to_the_new_note_and_encode_delimiters() {
    let source = "---\nname: Links\npath: projects/demo/notes/new.md\n---\n{{link \"projects/demo/Some file (draft).md\"}}\n{{link \"people/sam.md\"}}\n{{wikilink \"people/sam.md\"}}";
    let rendered = render(
        &parse(source, "links", TemplateSource::Brain).expect("template"),
        &request(),
    )
    .expect("render");
    assert_eq!(
        rendered.body,
        "../Some%20file%20%28draft%29.md\n../../../people/sam.md\n[[people/sam]]"
    );
}

#[test]
fn paths_reject_escape_and_pick_a_non_overwriting_name() {
    let brain = tempfile::tempdir().expect("brain");
    for path in [
        "../../etc/x.md",
        "/etc/x.md",
        "notes/../x.md",
        "C:\\x.md",
        "",
        "a\nb.md",
    ] {
        assert!(relative_path(path).is_err(), "{path}");
    }
    fs::write(brain.path().join("note.md"), "original").expect("original");
    fs::write(brain.path().join("note-2.md"), "second").expect("second");
    assert_eq!(
        available_path(brain.path(), Path::new("note.md")).expect("path"),
        PathBuf::from("note-3.md")
    );
    assert_eq!(
        fs::read_to_string(brain.path().join("note.md")).expect("original"),
        "original"
    );
}

#[cfg(unix)]
#[test]
fn symlinks_cannot_load_templates_or_write_notes_outside_the_brain() {
    let brain = tempfile::tempdir().expect("brain");
    let outside = tempfile::tempdir().expect("outside");
    std::os::unix::fs::symlink(outside.path(), brain.path().join("escape")).expect("symlink");
    assert!(scoped_path(brain.path(), Path::new("escape/notes/new.md")).is_err());
    assert!(Registry::load_brain(brain.path(), Some(Path::new("escape"))).is_err());
    fs::create_dir_all(brain.path().join(".brainz/templates")).expect("directory");
    fs::write(
        outside.path().join("private.md"),
        "---\nname: Private\n---\nsecret",
    )
    .expect("fixture");
    std::os::unix::fs::symlink(
        outside.path().join("private.md"),
        brain.path().join(".brainz/templates/private.md"),
    )
    .expect("symlink");
    let registry = Registry::load_brain(brain.path(), None).expect("registry");
    assert_eq!(registry.errors.len(), 1);
    assert!(!registry.templates.contains_key("private"));
}

#[test]
fn brain_overrides_reload_and_bad_files_do_not_hide_the_builtins() {
    let brain = tempfile::tempdir().expect("brain");
    let directory = brain.path().join(".brainz/templates");
    fs::create_dir_all(&directory).expect("directory");
    fs::write(
        directory.join("daily.md"),
        "---\nname: My daily\n---\nFirst",
    )
    .expect("override");
    fs::write(
        directory.join("broken.md"),
        "---\nname: [broken\n---\nBroken",
    )
    .expect("broken");
    let registry = Registry::load_brain(brain.path(), None).expect("registry");
    assert_eq!(registry.templates.len(), 10);
    assert_eq!(registry.errors.len(), 1);
    assert_eq!(
        registry.templates["daily"].shadows,
        Some(TemplateSource::Builtin)
    );
    fs::write(
        directory.join("daily.md"),
        "---\nname: My daily\n---\nUpdated",
    )
    .expect("update");
    assert_eq!(
        Registry::load_brain(brain.path(), None)
            .expect("reload")
            .templates["daily"]
            .body,
        "Updated"
    );
}

#[test]
fn metadata_reports_unknown_keys_and_handles_crlf() {
    let template = parse(
        "\u{feff}---\r\nname: Test\r\nfuture: true\r\n---\r\nBody",
        "test",
        TemplateSource::Brain,
    )
    .expect("template");
    assert_eq!(template.body, "Body");
    assert_eq!(template.warnings, ["Unknown metadata key: future"]);
    assert!(parse("---\nname: Test", "test", TemplateSource::Brain).is_err());
    assert!(
        parse(
            "---\nname: Test\nversion: 99\n---\nBody",
            "test",
            TemplateSource::Brain
        )
        .is_err()
    );
}

#[test]
fn configured_template_folder_stays_scoped_and_duplicate_ids_are_reported() {
    let brain = tempfile::tempdir().expect("brain");
    fs::create_dir(brain.path().join("Templates")).expect("directory");
    fs::write(
        brain.path().join("brainz.toml"),
        "[templates]\ndir = 'Templates'\n",
    )
    .expect("config");
    fs::write(
        brain.path().join("Templates/first.md"),
        "---\nid: custom\nname: First\n---\nFirst",
    )
    .expect("first");
    fs::write(
        brain.path().join("Templates/second.md"),
        "---\nid: custom\nname: Second\n---\nSecond",
    )
    .expect("second");
    let registry = Registry::load_brain(brain.path(), None).expect("registry");
    assert_eq!(registry.templates["custom"].body, "First");
    assert_eq!(registry.errors.len(), 1);
    fs::write(
        brain.path().join("brainz.toml"),
        "[templates]\ndir = '../outside'\n",
    )
    .expect("config");
    assert!(Registry::load_brain(brain.path(), None).is_err());
}

#[test]
fn relative_dates_and_invalid_formats_are_predictable() {
    let rendered = render(&template("{{date}} | {{date \"%Y\"}} | {{date \"monday\"}} | {{date \"next friday\"}} | {{date \"-1d\"}}"), &request()).expect("dates");
    assert_eq!(
        rendered.body,
        "2026-10-12 | 2026 | 2026-10-12 | 2026-10-16 | 2026-10-11"
    );
    assert!(render(&template("{{date \"%Q\"}}"), &request()).is_err());
    assert!(render(&template("{{date \"+99999999999999999d\"}}"), &request()).is_err());
}

#[test]
fn repeated_cursor_and_runaway_output_are_rejected() {
    assert!(render(&template("{{cursor}} {{cursor}}"), &request()).is_err());
    let mut request = request();
    request.context.insert("items".into(), json!(vec!["x"; 50]));
    let template = template(&format!(
        "{{{{#each items}}}}{}{{{{/each}}}}",
        "x".repeat(100_000)
    ));
    assert!(render(&template, &request).is_err());
}
