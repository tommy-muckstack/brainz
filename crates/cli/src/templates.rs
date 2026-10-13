use anyhow::{Context as _, Result, ensure};
use brainz_templates::{InsertMode, Registry, RenderRequest};
use clap::{Parser, Subcommand};
use serde_json::{Map, Value};
use std::{io::Read as _, path::PathBuf};

#[derive(Parser)]
#[command(
    name = "brainz templates",
    about = "List and render local Markdown templates without writing notes"
)]
struct TemplateArgs {
    #[arg(long, global = true, default_value = ".")]
    workspace: PathBuf,
    #[arg(long, global = true)]
    directory: Option<PathBuf>,
    #[command(subcommand)]
    command: TemplateCommand,
}

#[derive(Subcommand)]
enum TemplateCommand {
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        id: String,
    },
    Render {
        id: String,
        #[arg(long = "input", value_parser = parse_input)]
        inputs: Vec<(String, Value)>,
        #[arg(long, default_value = "")]
        title: String,
        #[arg(long, default_value = ".")]
        folder: String,
        /// Read a JSON object of context providers from this file, or - for stdin.
        #[arg(long)]
        context: Option<PathBuf>,
        #[arg(long, default_value = "new", value_parser = ["new", "insert", "append", "section"])]
        mode: String,
        #[arg(long)]
        strict: bool,
        #[arg(long)]
        json: bool,
    },
}

fn parse_input(argument: &str) -> Result<(String, Value), String> {
    let (key, value) = argument.split_once('=').ok_or("Use --input name=value")?;
    if key.is_empty() {
        return Err("Input name is empty".into());
    }
    Ok((
        key.into(),
        serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.into())),
    ))
}

pub fn run(arguments: impl IntoIterator<Item = std::ffi::OsString>) -> Result<()> {
    let args = TemplateArgs::parse_from(arguments);
    let registry = Registry::load_brain(&args.workspace, args.directory.as_deref())?;
    match args.command {
        TemplateCommand::List { json } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&registry)?);
            } else {
                for (id, template) in &registry.templates {
                    println!(
                        "{id}\t{}\t{}",
                        template.metadata.name, template.metadata.description
                    );
                }
                for error in &registry.errors {
                    eprintln!("{}: {}", error.path.display(), error.message);
                }
            }
        }
        TemplateCommand::Show { id } => {
            let template = registry
                .templates
                .get(&id)
                .with_context(|| format!("Template {id} was not found"))?;
            print!(
                "---\n{}---\n{}",
                serde_yaml_ng::to_string(&template.metadata)?,
                template.body
            );
        }
        TemplateCommand::Render {
            id,
            inputs,
            title,
            folder,
            context,
            mode,
            strict,
            json,
        } => {
            let template = registry
                .templates
                .get(&id)
                .with_context(|| format!("Template {id} was not found"))?;
            let values = read_context(context)?;
            let request = RenderRequest {
                title,
                folder,
                input: inputs.into_iter().collect(),
                context: values,
                mode: match mode.as_str() {
                    "insert" => InsertMode::Insert,
                    "append" => InsertMode::Append,
                    "section" => InsertMode::Section,
                    _ => InsertMode::New,
                },
                ..RenderRequest::default()
            };
            let mut rendered = brainz_templates::render(template, &request)?;
            if let Some(path) = &rendered.path {
                rendered.path = Some(brainz_templates::available_path(&args.workspace, path)?);
            }
            ensure!(
                !strict || rendered.report.missing.is_empty(),
                "Missing template values: {}",
                rendered.report.missing.join(", ")
            );
            if json {
                println!("{}", serde_json::to_string_pretty(&rendered)?);
            } else {
                print!("{}", rendered.body);
                if !rendered.report.missing.is_empty() {
                    eprintln!(
                        "Missing template values: {}",
                        rendered.report.missing.join(", ")
                    );
                }
            }
        }
    }
    Ok(())
}

fn read_context(path: Option<PathBuf>) -> Result<Map<String, Value>> {
    let Some(path) = path else {
        return Ok(Map::new());
    };
    let mut reader: Box<dyn std::io::Read> = if path.as_os_str() == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(std::fs::File::open(path)?)
    };
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 1024 * 1024, "Context is larger than 1 MB");
    serde_json::from_slice(&bytes).context("Context must be a JSON object")
}

#[derive(Parser)]
#[command(
    name = "brainz new",
    about = "Create a local Markdown note without overwriting existing notes"
)]
struct NewArgs {
    #[arg(long, default_value = ".")]
    workspace: PathBuf,
    #[arg(long)]
    directory: Option<PathBuf>,
    #[arg(long, conflicts_with = "daily")]
    template: Option<String>,
    /// Reopen today's daily-note path if it already exists.
    #[arg(long)]
    daily: bool,
    #[arg(long, default_value = "Untitled")]
    title: String,
    #[arg(long, default_value = ".")]
    folder: String,
    #[arg(long = "input", value_parser = parse_input)]
    inputs: Vec<(String, Value)>,
    #[arg(long)]
    context: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

pub fn new_note(arguments: impl IntoIterator<Item = std::ffi::OsString>) -> Result<()> {
    let args = NewArgs::parse_from(arguments);
    let note = create_from_args(&args, Some(paths::data_dir()))?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&note)?);
    } else {
        println!("{}", args.workspace.join(note.path).display());
    }
    Ok(())
}

fn create_from_args(
    args: &NewArgs,
    recents_directory: Option<&std::path::Path>,
) -> Result<brainz_templates::CreatedNote> {
    let registry = Registry::load_brain(&args.workspace, args.directory.as_deref())?;
    let config = brainz_templates::TemplatesConfig::load(&args.workspace)?;
    let blank = brainz_templates::parse(
        "---\nname: Blank note\npath: '{{folder}}/{{slug title}}.md'\n---\n{{cursor}}",
        "blank",
        brainz_templates::TemplateSource::Builtin,
    )?;
    let request = RenderRequest {
        title: args.title.clone(),
        folder: args.folder.clone(),
        input: args.inputs.iter().cloned().collect(),
        context: read_context(args.context.clone())?,
        ..Default::default()
    };
    let default = if !args.daily && args.template.is_none() {
        let proposed = brainz_templates::render(&blank, &request)?
            .path
            .context("Blank note needs a path")?;
        registry
            .default_for_path(&config, &proposed)?
            .filter(|choice| choice.auto)
    } else {
        None
    };
    let template = if args.daily {
        registry
            .templates
            .get(&config.daily)
            .with_context(|| format!("Daily template {} is unavailable", config.daily))?
    } else if let Some(id) = &args.template {
        registry
            .templates
            .get(id)
            .with_context(|| format!("Template {id} was not found"))?
    } else {
        default
            .as_ref()
            .and_then(|choice| registry.templates.get(&choice.template))
            .unwrap_or(&blank)
    };
    let rendered = brainz_templates::render(template, &request)?;
    let missing = rendered
        .report
        .missing
        .iter()
        .filter(|missing| missing.starts_with("input."))
        .cloned()
        .collect::<Vec<_>>();
    ensure!(
        missing.is_empty(),
        "Missing inputs: {}. Use --input name=value.",
        missing.join(", ")
    );
    let note = brainz_templates::create_note(
        &args.workspace,
        &rendered,
        if args.daily {
            brainz_templates::CreatePolicy::OpenExisting
        } else {
            brainz_templates::CreatePolicy::Unique
        },
    )?;
    if template.metadata.id != "blank" {
        if let Some(directory) = recents_directory {
            if let Err(error) = brainz_templates::recent_templates_path(directory, &args.workspace)
                .and_then(|path| brainz_templates::record_recent(&path, &template.metadata.id))
            {
                eprintln!("Could not update recent templates: {error}");
            }
        }
    }
    Ok(note)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_command_honors_daily_folder_defaults_and_explicit_overrides() {
        let root = tempfile::tempdir().expect("temporary brain");
        let data = tempfile::tempdir().expect("local app data");
        std::fs::create_dir_all(root.path().join(".brainz/templates")).expect("templates");
        std::fs::write(root.path().join(".brainz/templates/folder-note.md"), "---\nname: Folder note\npath: '{{folder}}/{{slug title}}.md'\n---\n# {{title}}\n\nDefault content\n{{cursor}}\n").expect("template");
        std::fs::write(root.path().join(".brainz/templates/custom-day.md"), "---\nname: Custom day\npath: 'daily/{{date}}.md'\n---\n# Today\n\nCustom daily content\n").expect("daily template");
        let config = "[templates]\ndaily = 'custom-day'\n[[templates.folder]]\npath = 'projects/*'\ntemplate = 'folder-note'\nauto = false\n";
        std::fs::write(root.path().join("brainz.toml"), config).expect("config");
        let mut args =
            NewArgs::try_parse_from(["new", "--folder", "projects/alpha", "--title", "Launch"])
                .expect("arguments");
        args.workspace = root.path().to_path_buf();
        let blank = create_from_args(&args, None).expect("suggestion stays blank");
        assert_eq!(
            std::fs::read_to_string(root.path().join(blank.path)).expect("blank"),
            ""
        );
        std::fs::write(
            root.path().join("brainz.toml"),
            config.replace("auto = false", "auto = true"),
        )
        .expect("automatic rule");
        let note = create_from_args(&args, Some(data.path())).expect("automatic default");
        assert!(
            std::fs::read_to_string(root.path().join(note.path))
                .expect("note")
                .contains("Default content")
        );
        let history = brainz_templates::recent_templates_path(data.path(), root.path())
            .expect("history path");
        assert_eq!(
            brainz_templates::read_recents(&history).expect("history"),
            ["folder-note"]
        );
        args.template = Some("decision".into());
        args.inputs
            .push(("title".into(), Value::String("Pick a date".into())));
        let note = create_from_args(&args, None).expect("explicit template wins");
        assert!(
            std::fs::read_to_string(root.path().join(note.path))
                .expect("decision")
                .contains("## Context")
        );
        args.template = None;
        args.daily = true;
        let daily = create_from_args(&args, None).expect("configured daily");
        assert!(daily.path.starts_with("daily"));
        std::fs::write(root.path().join(&daily.path), "Existing edits").expect("edit");
        create_from_args(&args, None).expect("reopen");
        assert_eq!(
            std::fs::read_to_string(root.path().join(daily.path)).expect("preserved"),
            "Existing edits"
        );
    }

    #[test]
    fn new_command_checks_inputs_and_keeps_existing_daily_notes() {
        let root = tempfile::tempdir().expect("temporary brain");
        let mut args = NewArgs::try_parse_from(["new", "--daily"]).expect("arguments");
        args.workspace = root.path().into();
        let first = create_from_args(&args, None).expect("daily note");
        assert!(first.created);
        std::fs::write(root.path().join(&first.path), "My changes").expect("edit daily");
        let second = create_from_args(&args, None).expect("existing daily");
        assert!(!second.created);
        assert_eq!(first.path, second.path);
        assert_eq!(
            std::fs::read_to_string(root.path().join(&first.path)).expect("daily"),
            "My changes"
        );
        args.daily = false;
        args.template = Some("decision".into());
        assert!(create_from_args(&args, None).is_err());
        args.folder = "projects/sample".into();
        args.inputs = vec![("title".into(), Value::String("Use Markdown".into()))];
        let decision = create_from_args(&args, None).expect("decision");
        assert!(decision.path.starts_with("projects/sample"));
        assert!(
            std::fs::read_to_string(root.path().join(decision.path))
                .expect("decision note")
                .contains("Use Markdown")
        );
    }

    #[test]
    fn input_preserves_text_and_supports_structured_values() {
        assert_eq!(
            parse_input("title=A & B"),
            Ok(("title".into(), Value::String("A & B".into())))
        );
        assert_eq!(
            parse_input("event={\"title\":\"Sync\"}").expect("input").1["title"],
            "Sync"
        );
        assert!(parse_input("title").is_err());
    }
}
