use anyhow::{Context as _, Result, ensure};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::{
    io::{Read as _, Write as _},
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(
    name = "brainz canvas",
    about = "Read canvases and submit reviewable local proposals"
)]
struct CanvasArgs {
    #[arg(long, global = true, default_value = ".")]
    workspace: PathBuf,
    #[command(subcommand)]
    command: CanvasCommand,
}

#[derive(Subcommand)]
enum CanvasCommand {
    List,
    Read {
        board: Option<String>,
        #[arg(long, conflicts_with = "selection")]
        view: bool,
        #[arg(long)]
        selection: bool,
    },
    Apply {
        board: String,
        /// JSON batch path, or - to read from stdin.
        batch: PathBuf,
        /// Wait up to this many milliseconds for the open canvas to acknowledge.
        #[arg(long, default_value_t = 1500, value_parser = clap::value_parser!(u64).range(0..=30_000))]
        wait: u64,
    },
    Status {
        board: String,
        batch_id: String,
    },
    Events {
        board: Option<String>,
        #[arg(long, default_value_t = 0)]
        since: u64,
    },
    Open {
        board: Option<String>,
        #[arg(long)]
        object: Option<String>,
    },
    /// Run the workspace-scoped MCP server over stdin/stdout.
    Mcp,
}

pub fn run(arguments: impl IntoIterator<Item = std::ffi::OsString>) -> Result<()> {
    let args = CanvasArgs::parse_from(arguments);
    let root = args
        .workspace
        .canonicalize()
        .context("The brain folder does not exist")?;
    let value = match args.command {
        CanvasCommand::List => json!({"boards":brainz_canvas::list_boards(&root)?}),
        CanvasCommand::Read {
            board,
            view,
            selection,
        } => {
            let board = brainz_canvas::resolve_board(&root, board.as_deref())?;
            brainz_canvas::read_context(
                &root,
                &board,
                if view {
                    "view"
                } else if selection {
                    "selection"
                } else {
                    "all"
                },
            )?
        }
        CanvasCommand::Apply { board, batch, wait } => {
            let board = brainz_canvas::resolve_board(&root, Some(&board))?;
            let batch: Value = if batch.as_os_str() == "-" {
                let mut bytes = Vec::new();
                std::io::stdin()
                    .take(brainz_canvas::MAX_BATCH_BYTES + 1)
                    .read_to_end(&mut bytes)?;
                ensure!(
                    bytes.len() as u64 <= brainz_canvas::MAX_BATCH_BYTES,
                    "Batch is too large"
                );
                serde_json::from_slice(&bytes)?
            } else {
                brainz_canvas::read_json(&batch, brainz_canvas::MAX_BATCH_BYTES)?
            };
            let mut status = brainz_canvas::queue_batch(&root, &board, &batch)?;
            let deadline = Instant::now() + Duration::from_millis(wait);
            while status["status"] == "queued" && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
                if let Some(result) = brainz_canvas::batch_status(
                    &root,
                    &board,
                    batch["batchId"].as_str().context("Missing batchId")?,
                )? {
                    status = result;
                }
            }
            status
        }
        CanvasCommand::Status { board, batch_id } => {
            let board = brainz_canvas::resolve_board(&root, Some(&board))?;
            brainz_canvas::batch_status(&root, &board, &batch_id)?
                .unwrap_or_else(|| json!({"batchId":batch_id,"status":"not_found"}))
        }
        CanvasCommand::Events { board, since } => {
            let board = brainz_canvas::resolve_board(&root, board.as_deref())?;
            json!({"events":brainz_canvas::events(&root, &board, since)?})
        }
        CanvasCommand::Open { board, object } => {
            let board = brainz_canvas::resolve_board(&root, board.as_deref())?;
            let mut url = url::Url::parse("brainz://whiteboard")?;
            url.query_pairs_mut()
                .append_pair("path", &board.path.to_string_lossy());
            if let Some(object) = object {
                url.query_pairs_mut().append_pair("object", &object);
            }
            let status = std::process::Command::new(std::env::current_exe()?)
                .args(["--add", url.as_str()])
                .status()?;
            ensure!(status.success(), "Brainz could not open the canvas");
            json!({"opened":board.path})
        }
        CanvasCommand::Mcp => {
            return brainz_canvas::mcp::serve(
                &root,
                std::io::stdin().lock(),
                std::io::stdout().lock(),
            );
        }
    };
    let mut output = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut output, &value)?;
    output.write_all(b"\n")?;
    ensure!(value["status"] != "rejected", "Canvas proposal rejected");
    Ok(())
}
