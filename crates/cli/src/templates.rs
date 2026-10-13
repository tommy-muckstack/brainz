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
            let mut values: Map<String, Value> = Map::new();
            if let Some(path) = context {
                let mut bytes = Vec::new();
                let mut reader: Box<dyn std::io::Read> = if path.as_os_str() == "-" {
                    Box::new(std::io::stdin())
                } else {
                    Box::new(std::fs::File::open(path)?)
                };
                reader
                    .by_ref()
                    .take(1024 * 1024 + 1)
                    .read_to_end(&mut bytes)?;
                ensure!(bytes.len() <= 1024 * 1024, "Context is larger than 1 MB");
                values = serde_json::from_slice(&bytes).context("Context must be a JSON object")?;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

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
