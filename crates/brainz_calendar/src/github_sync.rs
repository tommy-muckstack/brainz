//! Brainz: "Sync to GitHub". Watches the open repo for work that isn't on
//! GitHub yet and offers one button that reviews the changes, commits, pushes
//! a branch, opens a PR, checks it, and merges it into main.

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use gpui::{
    Animation, AnimationExt, AnyWindowHandle, App, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, Global, Task, WeakEntity, Window, ease_out_quint,
};
use ui::{Tooltip, prelude::*};
use workspace::{ModalView, Workspace};

const POLL_INTERVAL: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq)]
pub enum SyncStatus {
    Unknown,
    Clean,
    Pending {
        changed: usize,
        ahead: usize,
        /// Commits on origin/main that aren't here yet.
        behind: usize,
    },
    Syncing(String),
    Error(String),
}

pub struct SyncState {
    repo: Option<PathBuf>,
    workspace: Option<WeakEntity<Workspace>>,
    status: SyncStatus,
    /// The banner is playing its slide-up before disappearing.
    leaving: bool,
    _poll: Option<Task<()>>,
    _sync: Option<Task<()>>,
    _leave: Option<Task<()>>,
}

// The tallest the banner can be while it slides in or out. It is a ceiling
// for the slide only; the card itself takes the height its text and button
// need, so no font size or state can clip the bottom off.
const BANNER_MAX_HEIGHT: f32 = 160.;
const SLIDE_MS: u64 = 320;

fn wants_banner(status: &SyncStatus) -> bool {
    matches!(
        status,
        SyncStatus::Pending { .. } | SyncStatus::Syncing(_) | SyncStatus::Error(_)
    )
}

/// Ease-out with a small overshoot, so the banner settles like a drawer.
fn ease_out_back(t: f32) -> f32 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

struct GlobalSyncState(Entity<SyncState>);

impl Global for GlobalSyncState {}

pub fn init(cx: &mut App) {
    let state = cx.new(|_| SyncState {
        repo: None,
        workspace: None,
        status: SyncStatus::Unknown,
        leaving: false,
        _poll: None,
        _sync: None,
        _leave: None,
    });
    cx.set_global(GlobalSyncState(state));
}

pub fn state(cx: &App) -> Option<Entity<SyncState>> {
    cx.try_global::<GlobalSyncState>().map(|g| g.0.clone())
}

pub(crate) fn command(repo: &Path, program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(repo);
    let path = std::env::var("PATH").unwrap_or_default();
    cmd.env(
        "PATH",
        format!("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:{path}"),
    );
    if std::env::var_os("DEVELOPER_DIR").is_none()
        && Path::new("/Library/Developer/CommandLineTools").is_dir()
    {
        cmd.env("DEVELOPER_DIR", "/Library/Developer/CommandLineTools");
    }
    cmd
}

/// Blocking on purpose: only ever called from the background executor.
#[allow(clippy::disallowed_methods)]
pub(crate) fn run(repo: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = command(repo, program)
        .args(args)
        .output()
        .with_context(|| format!("running {program} {}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        bail!(
            "`{program} {}` failed ({}): {}",
            args.join(" "),
            output.status,
            if stderr.is_empty() { stdout } else { stderr }
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn check_status(repo: &Path, fetch: bool) -> Result<SyncStatus> {
    // A local-only brain (no origin, or `sync = false` in brainz.toml) never
    // gets the banner; there is nothing to sync to.
    if !crate::brain_config::BrainConfig::load(repo).sync
        || run(repo, "git", &["remote", "get-url", "origin"]).is_err()
    {
        return Ok(SyncStatus::Clean);
    }
    if fetch {
        // Quiet and best-effort: offline just means "behind" stays stale.
        run(repo, "git", &["fetch", "--quiet", "origin", "main"]).ok();
    }
    let porcelain = run(repo, "git", &["status", "--porcelain"])?;
    let changed = porcelain.lines().filter(|line| !line.is_empty()).count();
    let count = |range: &str| {
        run(repo, "git", &["rev-list", range, "--count"])
            .ok()
            .and_then(|count| count.trim().parse::<usize>().ok())
            .unwrap_or(0)
    };
    let ahead = count("@{u}..HEAD");
    let behind = count("HEAD..origin/main");
    Ok(if changed == 0 && ahead == 0 && behind == 0 {
        SyncStatus::Clean
    } else {
        SyncStatus::Pending {
            changed,
            ahead,
            behind,
        }
    })
}

fn ensure_no_git_operation(repo: &Path) -> Result<()> {
    for operation in [
        "rebase-merge",
        "rebase-apply",
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
    ] {
        let path = run(repo, "git", &["rev-parse", "--git-path", operation])?;
        if repo.join(path).exists() {
            bail!(
                "Another Git operation is in progress. Finish or cancel it before updating. Brainz left your files in place."
            );
        }
    }
    if !run(repo, "git", &["ls-files", "--unmerged"])?.is_empty() {
        bail!(
            "Unresolved Git changes need attention before syncing. Local files were left in place."
        );
    }
    Ok(())
}

fn fast_forward_main(repo: &Path) -> Result<()> {
    let branch = run(repo, "git", &["branch", "--show-current"])?;
    if branch != "main" {
        bail!(
            "The active branch changed to `{branch}`. Local files were left in place; switch to main before updating."
        );
    }
    ensure_no_git_operation(repo)?;
    if !run(
        repo,
        "git",
        &["status", "--porcelain", "-z", "--untracked-files=all"],
    )?
    .is_empty()
    {
        bail!(
            "Local edits are waiting to be committed. Sync them before pulling; Brainz left every file in place."
        );
    }
    // Explicitly disable a user's merge.autostash setting as well: a successful
    // fast-forward followed by a failed stash application can write conflict markers.
    run(repo, "git", &["merge", "--ff-only", "--no-autostash", "origin/main"])
        .context("GitHub and this brain could not be fast-forwarded safely. Local commits and files were left in place. Use Sync to GitHub to merge committed work through a pull request.")?;
    Ok(())
}

fn pull_repo(repo: &Path, progress: &mut dyn FnMut(String)) -> Result<String> {
    progress("Fetching from GitHub".into());
    run(repo, "git", &["fetch", "origin", "main"])?;
    progress("Updating local main".into());
    fast_forward_main(repo)?;
    let head = run(repo, "git", &["log", "--oneline", "-1"])?;
    Ok(format!("Pulled the latest from GitHub. Now at {head}"))
}

/// Merges `origin/main` into the committed local `main`. On an overlapping
/// edit the merge is abandoned, every file is left as it was, and the error
/// names the notes that need a decision.
fn merge_origin_main(repo: &Path) -> Result<()> {
    ensure_no_git_operation(repo)?;
    if run(repo, "git", &["branch", "--show-current"])? != "main" {
        bail!("The active branch changed. Local work was kept; return to main before syncing.");
    }
    if !run(
        repo,
        "git",
        &["status", "--porcelain", "-z", "--untracked-files=all"],
    )?
    .is_empty()
    {
        bail!("Local edits are waiting to be committed. Sync again so they are reviewed first.");
    }
    let head = run(repo, "git", &["rev-parse", "HEAD"])?;
    let merge = run(
        repo,
        "git",
        &[
            "merge",
            "--no-autostash",
            "--no-edit",
            "-m",
            "notes: merge the latest from GitHub",
            "origin/main",
        ],
    );
    if merge.is_ok() {
        return Ok(());
    }
    let conflicted = run(repo, "git", &["diff", "--name-only", "--diff-filter=U"])
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    // Put every file back exactly as it was before the merge began; nothing
    // is published and no conflict markers reach a note.
    run(repo, "git", &["merge", "--abort"])
        .or_else(|_| run(repo, "git", &["reset", "--hard", &head]))
        .with_context(|| "Could not undo the merge attempt; check the brain folder in Terminal.")?;
    if conflicted.is_empty() {
        return Err(merge.unwrap_err());
    }
    let list = conflicted
        .iter()
        .map(|path| format!("  {path}"))
        .collect::<Vec<_>>()
        .join("\n");
    bail!(
        "This Mac and GitHub both changed the same notes, so Brainz can't merge them on its own:\n{list}\n\nYour files were left exactly as they are. In Terminal, run `git merge origin/main` in the brain folder, keep the lines you want from each side, commit, then sync again."
    )
}

/// Earlier sync attempts can leave pull requests open on GitHub when they
/// stopped before merging. Once their commits are part of `publish_head`
/// they are superseded, so close them instead of adding a duplicate.
fn close_superseded_sync_requests(repo: &Path, publish_head: &str) {
    let Ok(listing) = run(
        repo,
        "gh",
        &[
            "pr",
            "list",
            "--state",
            "open",
            "--json",
            "number,headRefName,headRefOid",
            "--jq",
            r#".[] | select(.headRefName | startswith("brainz/sync-")) | "\(.number) \(.headRefOid)""#,
        ],
    ) else {
        return;
    };
    for line in listing.lines() {
        let Some((number, head)) = line.split_once(' ') else {
            continue;
        };
        let superseded = run(
            repo,
            "git",
            &["merge-base", "--is-ancestor", head, publish_head],
        )
        .is_ok();
        if !superseded {
            continue;
        }
        if let Err(error) = run(
            repo,
            "gh",
            &[
                "pr",
                "close",
                number,
                "--delete-branch",
                "--comment",
                "Superseded by a newer Brainz sync that includes these commits.",
            ],
        ) {
            log::warn!("brainz sync: could not close superseded pull request #{number}: {error}");
        }
    }
}

fn finish_sync(repo: &Path, pr_url: &str, progress: &mut dyn FnMut(String)) -> Result<()> {
    run(repo, "git", &["fetch", "--prune", "origin", "main"])?;
    progress("Updating local main".into());
    fast_forward_main(repo).with_context(|| format!(
        "Your pull request was merged on GitHub: {pr_url}\nNew local work still needs attention before this checkout can update."
    ))
}

fn commit_subject(paths: &[&str]) -> String {
    let area_for = |path: &str| {
        let parts = path.split('/').collect::<Vec<_>>();
        let area = parts
            .iter()
            .take(parts.len().saturating_sub(1).min(2))
            .map(|part| {
                part.to_lowercase()
                    .chars()
                    .map(|character| {
                        if character.is_ascii_alphanumeric() {
                            character
                        } else {
                            '-'
                        }
                    })
                    .collect::<String>()
                    .split('-')
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("-")
            })
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        if area.is_empty() {
            "notes".to_owned()
        } else {
            area
        }
    };
    let Some(first) = paths.first() else {
        return "notes: update notes".into();
    };
    let first_area = area_for(first);
    let area = if paths.iter().all(|path| area_for(path) == first_area) {
        first_area.as_str()
    } else {
        "notes"
    };
    if paths.len() == 1 {
        let stem = Path::new(first)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("note");
        let stem = stem.split_whitespace().collect::<Vec<_>>().join(" ");
        let stem = stem
            .chars()
            .filter(|character| !character.is_control())
            .take(100)
            .collect::<String>();
        format!("{area}: edit {stem}")
    } else {
        format!("{area}: update {} files", paths.len())
    }
}

fn commit_local_changes(repo: &Path) -> Result<()> {
    ensure_no_git_operation(repo)?;
    if run(repo, "git", &["branch", "--show-current"])? != "main" {
        bail!("The active branch changed. Local work was kept; return to main before syncing.");
    }
    run(repo, "git", &["add", "-A"])?;
    let reviewed = crate::sync_review::staged_tree(repo)?;
    if reviewed.branch != "refs/heads/main" {
        bail!("The active branch changed during review. Local work was kept.");
    }
    let summary = run(
        repo,
        "git",
        &["diff", "--stat", &reviewed.head, &reviewed.tree],
    )?;
    let paths = reviewed
        .paths
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let subject = commit_subject(&paths);
    let message = format!(
        "{subject}\n\n{summary}\n\nBrainz-Session: {}",
        uuid::Uuid::new_v4()
    );
    crate::sync_review::commit(repo, &reviewed, &message)?;
    Ok(())
}

fn sync_repo(repo: &Path, progress: &mut dyn FnMut(String)) -> Result<String> {
    progress("Checking branch".into());
    ensure_no_git_operation(repo)?;
    let branch = run(repo, "git", &["branch", "--show-current"])?;
    if branch != "main" {
        bail!("You're on branch `{branch}`. Switch to `main` first, then sync.");
    }
    if run(repo, "gh", &["auth", "status"]).is_err() {
        bail!("{GITHUB_SIGN_IN_NEEDED}");
    }

    let status = check_status(repo, true)?;
    let SyncStatus::Pending {
        changed,
        ahead,
        behind,
    } = status
    else {
        return Ok("Already in sync with GitHub.".into());
    };
    if changed == 0 && ahead == 0 {
        if behind > 0 {
            pull_repo(repo, progress)?;
        }
        return Ok("Pulled the latest from GitHub. Nothing local to push.".into());
    }
    // A PR can merge diverged histories on GitHub without rebasing or stashing
    // the live checkout. Keep local edits in place throughout that review.

    if changed > 0 {
        progress(format!("Committing {changed} change(s)"));
        commit_local_changes(repo)?;
    }
    if behind > 0 {
        // Bring GitHub's commits in here first, so the pull request never
        // conflicts on GitHub, and an overlapping edit is named while the
        // files are still in front of the person instead of in a browser.
        progress("Merging the latest from GitHub".into());
        merge_origin_main(repo)?;
    }

    progress("Reviewing commits".into());
    let publish_head = run(repo, "git", &["rev-parse", "HEAD"])?;
    let commit_count = crate::sync_review::outgoing_commits(repo, "origin/main", &publish_head)?;
    close_superseded_sync_requests(repo, &publish_head);
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let sync_branch = format!("brainz/sync-{stamp}");
    progress("Pushing branch".into());
    run(repo, "git", &["branch", &sync_branch, &publish_head])?;
    run(repo, "git", &["push", "-u", "origin", &sync_branch])?;

    progress("Opening pull request".into());
    let revision_range = format!("origin/main..{publish_head}");
    let commits = run(repo, "git", &["log", "--oneline", &revision_range])?;
    let body = format!(
        "Synced from Brainz.\n\nCommits:\n{}\n\nReviewed automatically: no credential files, secret markers, or files over 25 MB.",
        commits
            .lines()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let title = if commit_count == 1 {
        run(repo, "git", &["log", "-1", "--format=%s", &publish_head])?
    } else {
        format!("notes: sync {commit_count} commits")
    };
    let pr_url = run(
        repo,
        "gh",
        &[
            "pr",
            "create",
            "--base",
            "main",
            "--head",
            &sync_branch,
            "--title",
            &title,
            "--body",
            &body,
        ],
    )?;

    progress("Checking the pull request".into());
    let mut mergeable = false;
    for _ in 0..20 {
        let state = run(
            repo,
            "gh",
            &[
                "pr",
                "view",
                &sync_branch,
                "--json",
                "mergeable",
                "--jq",
                ".mergeable",
            ],
        )?;
        match state.as_str() {
            "MERGEABLE" => {
                mergeable = true;
                break;
            }
            "CONFLICTING" => bail!(
                "The pull request has conflicts with main. Open it to resolve them:\n{pr_url}"
            ),
            _ => std::thread::sleep(Duration::from_secs(3)),
        }
    }
    if !mergeable {
        bail!(
            "GitHub hasn't finished checking the pull request. Open it and merge when ready:\n{pr_url}"
        );
    }

    progress("Merging into main".into());
    run(repo, "gh", &["pr", "merge", &sync_branch, "--merge"])?;

    finish_sync(repo, &pr_url, progress)?;
    if let Err(error) = run(repo, "git", &["branch", "-d", &sync_branch]) {
        log::warn!("Synced successfully but kept the local sync branch: {error}");
    }
    Ok(format!("Merged into main: {pr_url}"))
}

impl SyncState {
    fn ensure_watching(
        &mut self,
        workspace: WeakEntity<Workspace>,
        repo: PathBuf,
        cx: &mut Context<Self>,
    ) {
        if self.repo.as_deref() == Some(repo.as_path()) {
            return;
        }
        self.repo = Some(repo.clone());
        self.workspace = Some(workspace);
        self.status = SyncStatus::Unknown;
        self._poll = Some(cx.spawn(async move |this, cx| {
            let mut last_fetch: Option<std::time::Instant> = None;
            loop {
                let repo = repo.clone();
                // Fetch every couple of minutes; status alone every poll.
                let fetch = last_fetch.is_none_or(|at| at.elapsed() > Duration::from_secs(120));
                if fetch {
                    last_fetch = Some(std::time::Instant::now());
                }
                let status = cx
                    .background_spawn(async move { check_status(&repo, fetch) })
                    .await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        if !matches!(this.status, SyncStatus::Syncing(_) | SyncStatus::Error(_)) {
                            let new_status = status.unwrap_or(SyncStatus::Unknown);
                            if new_status != this.status {
                                this.set_status(new_status, cx);
                            }
                        }
                    })
                    .is_ok();
                if !keep_going {
                    break;
                }
                cx.background_executor().timer(POLL_INTERVAL).await;
            }
        }));
    }

    pub fn status(&self) -> &SyncStatus {
        &self.status
    }

    /// The brain checkout being watched, once the project panel reported it.
    pub fn repo(&self) -> Option<&PathBuf> {
        self.repo.as_ref()
    }

    /// Changes status and, when the banner is going away, keeps it around
    /// long enough to slide back up.
    fn set_status(&mut self, status: SyncStatus, cx: &mut Context<Self>) {
        let was_visible = wants_banner(&self.status);
        let now_visible = wants_banner(&status);
        self.status = status;
        if was_visible && !now_visible {
            self.leaving = true;
            self._leave = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(SLIDE_MS + 40))
                    .await;
                this.update(cx, |this, cx| {
                    this.leaving = false;
                    cx.notify();
                })
                .ok();
            }));
        } else if now_visible {
            self.leaving = false;
            self._leave = None;
        }
        cx.notify();
    }

    fn dismiss_error(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, SyncStatus::Error(_)) {
            self.set_status(SyncStatus::Unknown, cx);
        }
    }

    fn sync(&mut self, window_handle: AnyWindowHandle, pull_only: bool, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if matches!(self.status, SyncStatus::Syncing(_)) {
            return;
        }
        self.status = SyncStatus::Syncing("Starting".into());
        cx.notify();
        let (tx, rx) = smol::channel::unbounded::<String>();
        let progress_task = cx.spawn(async move |this, cx| {
            while let Ok(step) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.status = SyncStatus::Syncing(step);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self._sync = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut report = |step: String| {
                        tx.send_blocking(step).ok();
                    };
                    if pull_only {
                        pull_repo(&repo, &mut report)
                    } else {
                        sync_repo(&repo, &mut report)
                    }
                })
                .await;
            drop(progress_task);
            this.update(cx, |this, cx| {
                match result {
                    Ok(message) => {
                        this.set_status(SyncStatus::Clean, cx);
                        if let Some(workspace) = this.workspace.clone() {
                            workspace
                                .update(cx, |workspace, cx| {
                                    let id = workspace::notifications::NotificationId::unique::<SyncState>();
                                    workspace.show_notification(
                                        id.clone(),
                                        cx,
                                        |cx| {
                                            cx.new(|cx| {
                                                workspace::notifications::simple_message_notification::MessageNotification::new(
                                                    format!("Synced to GitHub. {message}"),
                                                    cx,
                                                )
                                            })
                                        },
                                    );
                                    // The toast is confirmation, not a decision:
                                    // let it go away on its own.
                                    cx.spawn(async move |workspace, cx| {
                                        cx.background_executor()
                                            .timer(Duration::from_secs(6))
                                            .await;
                                        workspace
                                            .update(cx, |workspace, cx| {
                                                workspace.dismiss_notification(&id, cx);
                                            })
                                            .ok();
                                    })
                                    .detach();
                                })
                                .ok();
                        }
                    }
                    Err(error) => {
                        let message = format!("{error:#}");
                        this.set_status(SyncStatus::Error(message.clone()), cx);
                        if let Some(workspace) = this.workspace.clone() {
                            let state = cx.entity();
                            window_handle
                                .update(cx, |_, window, cx| {
                                    workspace
                                        .update(cx, |workspace, cx| {
                                            workspace.toggle_modal(window, cx, |_, cx| {
                                                SyncErrorModal::new(
                                                    message.clone(),
                                                    state.clone(),
                                                    cx,
                                                )
                                            });
                                        })
                                        .ok();
                                })
                                .ok();
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }
}

/// Popup shown when a sync fails.
/// The one sync failure with a button instead of an explanation.
pub const GITHUB_SIGN_IN_NEEDED: &str = "GitHub isn't signed in on this Mac yet.";

pub struct SyncErrorModal {
    message: String,
    state: Entity<SyncState>,
    focus_handle: FocusHandle,
}

/// Runs `gh auth login` in the terminal panel (the browser opens for the
/// code), then syncs again once it succeeds, so the person never sees a
/// shell command.
pub fn spawn_github_login(
    workspace: WeakEntity<Workspace>,
    state: Entity<SyncState>,
    window: &mut Window,
    cx: &mut App,
) -> Task<Result<()>> {
    use terminal_view::terminal_panel::TerminalPanel;
    use workspace::dock::DockPosition;
    let Some(workspace_entity) = workspace.upgrade() else {
        return Task::ready(Err(anyhow::anyhow!("workspace closed")));
    };
    let Some(terminal_panel) = workspace_entity.read(cx).panel::<TerminalPanel>(cx) else {
        return Task::ready(Err(anyhow::anyhow!("Terminal panel is unavailable")));
    };
    let previous_panel = workspace_entity
        .read(cx)
        .dock_at_position(DockPosition::Bottom)
        .read(cx)
        .active_panel_index();
    let label = "Sign in to GitHub".to_owned();
    let spawn = task::SpawnInTerminal {
        id: task::TaskId("brainz-github-login".to_owned()),
        full_label: label.clone(),
        label: label.clone(),
        command: None,
        args: Vec::new(),
        command_label: label,
        use_new_terminal: true,
        allow_concurrent_runs: false,
        hide: task::HideStrategy::Always,
        shell: task::Shell::WithArguments {
            program: "gh".to_owned(),
            args: ["auth", "login", "--web", "--git-protocol", "https"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            title_override: None,
        },
        ..Default::default()
    };
    window.spawn(cx, async move |cx| {
        let terminal = terminal_panel
            .update_in(cx, |terminal_panel, window, cx| {
                terminal_panel.spawn_task(&spawn, window, cx)
            })?
            .await?;
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_panel::<TerminalPanel>(window, cx);
        })?;
        let exit = terminal
            .read_with(cx, |terminal, cx| terminal.wait_for_completed_task(cx))?
            .await;
        let signed_in = matches!(exit, Some(status) if status.success());
        workspace.update_in(cx, |workspace, window, cx| {
            if let Some(index) = previous_panel {
                workspace
                    .dock_at_position(DockPosition::Bottom)
                    .update(cx, |dock, cx| dock.activate_panel(index, window, cx));
            }
            if signed_in {
                let handle = window.window_handle();
                state.update(cx, |state, cx| {
                    state.dismiss_error(cx);
                    state.sync(handle, false, cx);
                });
            }
        })?;
        if signed_in {
            Ok(())
        } else {
            Err(anyhow::anyhow!("GitHub sign-in did not finish"))
        }
    })
}

impl SyncErrorModal {
    fn new(message: String, state: Entity<SyncState>, cx: &mut Context<Self>) -> Self {
        Self {
            message,
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl ModalView for SyncErrorModal {}
impl EventEmitter<DismissEvent> for SyncErrorModal {}

impl Focusable for SyncErrorModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SyncErrorModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let needs_sign_in = self.message == GITHUB_SIGN_IN_NEEDED;
        let (title, body) = if needs_sign_in {
            (
                "Sign in to GitHub",
                "Sync keeps your brain on GitHub through the GitHub CLI, which isn't signed in on this Mac yet. Sign in once: a browser window opens for the code, and Sync runs again as soon as you're done."
                    .to_owned(),
            )
        } else {
            ("Sync to GitHub didn't finish", self.message.clone())
        };
        v_flex()
            .key_context("BrainzSyncError")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .w(px(520.))
            .p_4()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().elevated_surface_background)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Icon::new(if needs_sign_in {
                            IconName::Github
                        } else {
                            IconName::Warning
                        })
                        .color(if needs_sign_in {
                            Color::Default
                        } else {
                            Color::Warning
                        }),
                    )
                    .child(Label::new(title).weight(gpui::FontWeight::SEMIBOLD)),
            )
            .child(Label::new(body).size(LabelSize::Small))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new(
                            "brainz-sync-error-ok",
                            if needs_sign_in { "Not now" } else { "OK" },
                        )
                        .style(if needs_sign_in {
                            ButtonStyle::Subtle
                        } else {
                            ButtonStyle::Filled
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |state, cx| state.dismiss_error(cx));
                            cx.emit(DismissEvent);
                        })),
                    )
                    .when(needs_sign_in, |this| {
                        this.child(
                            Button::new("brainz-sync-github-login", "Sign in to GitHub")
                                .style(ButtonStyle::Filled)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let (Some(workspace), state) =
                                        (this.state.read(cx).workspace.clone(), this.state.clone())
                                    else {
                                        return;
                                    };
                                    cx.emit(DismissEvent);
                                    spawn_github_login(workspace, state, window, cx)
                                        .detach_and_log_err(cx);
                                })),
                        )
                    }),
            )
    }
}

/// The banner the project panel shows above the file tree.
pub fn render_banner(workspace: &WeakEntity<Workspace>, cx: &mut App) -> Option<gpui::AnyElement> {
    let state = state(cx)?;
    let repo = workspace
        .upgrade()?
        .read(cx)
        .root_paths(cx)
        .first()
        .map(|path| path.to_path_buf())?;
    state.update(cx, |state, cx| {
        state.ensure_watching(workspace.clone(), repo, cx)
    });
    let (status, leaving) = {
        let state = state.read(cx);
        (state.status().clone(), state.leaving)
    };

    let pull_only = matches!(
        status,
        SyncStatus::Pending {
            changed: 0,
            ahead: 0,
            behind
        } if behind > 0
    );
    let (text, accent) = match &status {
        SyncStatus::Pending {
            changed,
            ahead,
            behind,
        } => {
            let plural =
                |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
            let mut local = Vec::new();
            if *changed > 0 {
                local.push(plural(*changed, "change"));
            }
            if *ahead > 0 {
                local.push(plural(*ahead, "commit"));
            }
            let mut parts = Vec::new();
            if !local.is_empty() {
                parts.push(format!("{} not on GitHub", local.join(", ")));
            }
            if *behind > 0 {
                parts.push(format!("{} new on GitHub", plural(*behind, "commit")));
            }
            (parts.join(" · "), false)
        }
        SyncStatus::Syncing(step) => (format!("{step}…"), true),
        SyncStatus::Error(_) => ("Sync didn't finish".to_owned(), false),
        SyncStatus::Clean | SyncStatus::Unknown if leaving => ("All synced".to_owned(), false),
        SyncStatus::Clean | SyncStatus::Unknown => return None,
    };
    let syncing = matches!(status, SyncStatus::Syncing(_));
    let errored = matches!(status, SyncStatus::Error(_));
    let colors = cx.theme().colors();
    // Brainz: the banner is a solid amber strip with dark text, so it reads
    // as a call to action rather than a panel.
    let amber = colors.text_accent;
    let ink = gpui::hsla(0., 0., 0.08, 1.);
    let banner_bg = if errored {
        cx.theme().status().warning
    } else {
        amber
    };

    let banner = v_flex()
        .id("brainz-github-sync-banner")
        .flex_none()
        .w_full()
        .px_2()
        .py_2()
        .child(
            // The amber card itself; the outer box keeps the panel's edge so
            // the rounded corners read.
            v_flex()
                .size_full()
                .px_2()
                .py_1p5()
                .gap_1p5()
                .rounded_lg()
                .bg(banner_bg)
        .child(
            h_flex()
                .gap_1p5()
                .child(
                    Icon::new(if errored {
                        IconName::Warning
                    } else {
                        IconName::GitBranch
                    })
                    .size(IconSize::Small)
                    .color(Color::Custom(ink)),
                )
                .child(
                    Label::new(text)
                        .size(LabelSize::Small)
                        .color(Color::Custom(ink))
                        .truncate(),
                ),
        )
        .when(!leaving && syncing, |this| {
            let dots = h_flex()
                .h(px(18.))
                .w_full()
                .items_center()
                .justify_center()
                .child(ui::bouncing_dots("brainz-sync", amber));
            this.child(
                ui::ButtonLike::new("brainz-syncing")
                    .style(ButtonStyle::Filled)
                    .full_width()
                    .tooltip(Tooltip::text("Syncing to GitHub"))
                    .child(dots),
            )
        })
        .when(!leaving && !syncing, |this| this.child(
            Button::new(
                "brainz-sync-to-github",
                if pull_only { "Pull from GitHub" } else { "Sync to GitHub" },
            )
                .full_width()
                .style(ButtonStyle::Filled)
                .tooltip(Tooltip::text(if pull_only {
                    "Bring GitHub's newer commits down when local work is committed"
                } else {
                    "Pull anything new, then review, commit, push, open a pull request, and merge it into main"
                }))
                .on_click({
                    let state = state.clone();
                    move |_, window, cx| {
                        let handle = window.window_handle();
                        state.update(cx, |state, cx| state.sync(handle, pull_only, cx));
                    }
                }),
        ))
        .when(leaving, |this| {
            this.child(
                h_flex().gap_1p5().child(
                    Icon::new(IconName::Check)
                        .size(IconSize::Small)
                        .color(Color::Custom(ink)),
                ),
            )
        }),
        );
    let _ = accent;

    // The wrapper animates height and opacity so the file tree glides with
    // the banner instead of jumping.
    let animated = if leaving {
        div()
            .flex_none()
            .w_full()
            .overflow_hidden()
            .child(banner)
            .with_animation(
                "brainz-github-sync-banner-out",
                Animation::new(Duration::from_millis(SLIDE_MS)).with_easing(ease_out_quint()),
                |wrapper, delta| {
                    let remaining = 1.0 - delta;
                    wrapper
                        .max_h(px(BANNER_MAX_HEIGHT * remaining))
                        .opacity(remaining)
                },
            )
            .into_any_element()
    } else {
        div()
            .flex_none()
            .w_full()
            .overflow_hidden()
            .child(banner)
            .with_animation(
                "brainz-github-sync-banner-in",
                Animation::new(Duration::from_millis(SLIDE_MS)).with_easing(ease_out_back),
                |wrapper, delta| {
                    wrapper
                        .max_h(px(BANNER_MAX_HEIGHT * delta))
                        .opacity(delta.min(1.0))
                },
            )
            .into_any_element()
    };

    Some(animated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_subjects_explain_the_changed_area_without_a_model() {
        assert_eq!(
            commit_subject(&["projects/site-redesign/plan.md"]),
            "projects/site-redesign: edit plan"
        );
        assert_eq!(
            commit_subject(&["Projects/Launch Plan.md"]),
            "projects: edit Launch Plan"
        );
        assert_eq!(commit_subject(&["NOW.md"]), "notes: edit NOW");
        assert_eq!(
            commit_subject(&["health/log.md", "health/training.md"]),
            "health: update 2 files"
        );
        assert_eq!(
            commit_subject(&["health/log.md", "work/plan.md"]),
            "notes: update 2 files"
        );
        assert_eq!(
            commit_subject(&["My Projects/Big_plan/a\nInjected.md"]),
            "my-projects/big-plan: edit a Injected"
        );
        assert_eq!(commit_subject(&[]), "notes: update notes");
    }

    struct Repositories {
        _directory: tempfile::TempDir,
        local: PathBuf,
        remote_writer: PathBuf,
    }

    impl Repositories {
        fn new() -> Self {
            let directory = tempfile::tempdir().expect("temporary repositories");
            let origin = directory.path().join("origin.git");
            run(
                directory.path(),
                "git",
                &[
                    "init",
                    "--bare",
                    "--initial-branch=main",
                    origin.to_str().expect("path"),
                ],
            )
            .expect("bare origin");
            let local = directory.path().join("local");
            run(
                directory.path(),
                "git",
                &[
                    "clone",
                    origin.to_str().expect("path"),
                    local.to_str().expect("path"),
                ],
            )
            .expect("local clone");
            Self::configure(&local);
            std::fs::write(local.join("note.md"), "Original paragraph\n").expect("note");
            std::fs::write(local.join("other.md"), "Another note\n").expect("other note");
            run(&local, "git", &["add", "note.md", "other.md"]).expect("stage fixture");
            run(&local, "git", &["commit", "-m", "notes: initial fixture"]).expect("commit");
            run(&local, "git", &["push", "-u", "origin", "main"])
                .expect("push to local bare origin");
            let remote_writer = directory.path().join("remote-writer");
            run(
                directory.path(),
                "git",
                &[
                    "clone",
                    origin.to_str().expect("path"),
                    remote_writer.to_str().expect("path"),
                ],
            )
            .expect("second clone");
            Self::configure(&remote_writer);
            Self {
                _directory: directory,
                local,
                remote_writer,
            }
        }

        fn configure(repo: &Path) {
            for (key, value) in [
                ("user.name", "Brainz Fixture"),
                ("user.email", "fixture@example.invalid"),
                ("commit.gpgsign", "false"),
                ("core.hooksPath", "/dev/null"),
                ("merge.autostash", "true"),
                ("rebase.autostash", "true"),
            ] {
                run(repo, "git", &["config", key, value]).expect("isolated configuration");
            }
        }

        fn remote_edit(&self, path: &str, body: &str) {
            std::fs::write(self.remote_writer.join(path), body).expect("remote edit");
            run(&self.remote_writer, "git", &["add", "--", path]).expect("stage remote");
            run(
                &self.remote_writer,
                "git",
                &["commit", "-m", "notes: remote edit"],
            )
            .expect("commit remote");
            run(&self.remote_writer, "git", &["push", "origin", "main"]).expect("push fixture");
        }

        fn assert_no_integration_state(&self) {
            for operation in ["rebase-merge", "rebase-apply", "MERGE_HEAD"] {
                let path = run(&self.local, "git", &["rev-parse", "--git-path", operation])
                    .expect("git path");
                assert!(!self.local.join(path).exists(), "no {operation}");
            }
            assert!(
                run(&self.local, "git", &["stash", "list"])
                    .expect("stashes")
                    .is_empty()
            );
        }
    }

    #[test]
    fn local_commit_preserves_configured_author_and_records_a_session() {
        let repositories = Repositories::new();
        std::fs::write(repositories.local.join("note.md"), "Edited note\n").expect("edit");
        commit_local_changes(&repositories.local).expect("commit");
        let body = run(&repositories.local, "git", &["log", "-1", "--format=%B"]).expect("message");
        assert!(body.starts_with("notes: edit note\n"));
        let session = body
            .lines()
            .find_map(|line| line.strip_prefix("Brainz-Session: "))
            .expect("session trailer");
        assert!(uuid::Uuid::parse_str(session).is_ok());
        assert_eq!(
            run(
                &repositories.local,
                "git",
                &["log", "-1", "--format=%an <%ae>"]
            )
            .expect("author"),
            "Brainz Fixture <fixture@example.invalid>"
        );
        assert!(
            run(&repositories.local, "git", &["status", "--porcelain"])
                .expect("status")
                .is_empty()
        );
    }

    #[test]
    fn pulls_clean_notes_without_creating_a_merge_or_stash() {
        let repositories = Repositories::new();
        repositories.remote_edit("note.md", "Remote paragraph\n");
        pull_repo(&repositories.local, &mut |_| {}).expect("clean pull");
        assert_eq!(
            std::fs::read_to_string(repositories.local.join("note.md")).expect("note"),
            "Remote paragraph\n"
        );
        assert_eq!(
            run(&repositories.local, "git", &["rev-parse", "HEAD"]).expect("local head"),
            run(&repositories.remote_writer, "git", &["rev-parse", "HEAD"]).expect("remote head")
        );
        repositories.assert_no_integration_state();
    }

    #[test]
    fn dirty_pull_preserves_index_worktree_untracked_files_and_crlf() {
        let repositories = Repositories::new();
        repositories.remote_edit("note.md", "Remote paragraph\n");
        std::fs::write(
            repositories.local.join("note.md"),
            b"My staged paragraph\r\n",
        )
        .expect("staged draft");
        run(&repositories.local, "git", &["add", "note.md"]).expect("stage draft");
        std::fs::write(
            repositories.local.join("note.md"),
            b"My newest paragraph\r\n",
        )
        .expect("unstaged draft");
        std::fs::write(repositories.local.join("new note.md"), "Unsaved to Git\n")
            .expect("untracked note");
        let head = run(&repositories.local, "git", &["rev-parse", "HEAD"]).expect("head");
        let index = run(&repositories.local, "git", &["write-tree"]).expect("index");
        assert!(pull_repo(&repositories.local, &mut |_| {}).is_err());
        assert_eq!(
            std::fs::read(repositories.local.join("note.md")).expect("draft"),
            b"My newest paragraph\r\n"
        );
        assert_eq!(
            std::fs::read_to_string(repositories.local.join("new note.md")).expect("untracked"),
            "Unsaved to Git\n"
        );
        assert_eq!(
            run(&repositories.local, "git", &["write-tree"]).expect("index"),
            index
        );
        assert_eq!(
            run(&repositories.local, "git", &["rev-parse", "HEAD"]).expect("head"),
            head
        );
        repositories.assert_no_integration_state();
    }

    #[test]
    fn diverged_pull_leaves_commits_and_prose_without_conflict_markers() {
        let repositories = Repositories::new();
        repositories.remote_edit("note.md", "Remote paragraph\n");
        std::fs::write(
            repositories.local.join("note.md"),
            "My committed paragraph\n",
        )
        .expect("local edit");
        run(&repositories.local, "git", &["add", "note.md"]).expect("stage");
        run(
            &repositories.local,
            "git",
            &["commit", "-m", "notes: my edit"],
        )
        .expect("commit");
        let head = run(&repositories.local, "git", &["rev-parse", "HEAD"]).expect("head");
        assert!(pull_repo(&repositories.local, &mut |_| {}).is_err());
        assert_eq!(
            run(&repositories.local, "git", &["rev-parse", "HEAD"]).expect("head"),
            head
        );
        assert_eq!(
            std::fs::read_to_string(repositories.local.join("note.md")).expect("note"),
            "My committed paragraph\n"
        );
        repositories.assert_no_integration_state();
    }

    #[test]
    fn diverged_sync_merges_github_before_publishing() {
        let repositories = Repositories::new();
        repositories.remote_edit("other.md", "Remote paragraph\n");
        std::fs::write(repositories.local.join("note.md"), "My paragraph\n").expect("local edit");
        run(&repositories.local, "git", &["add", "note.md"]).expect("stage");
        run(
            &repositories.local,
            "git",
            &["commit", "-m", "notes: my edit"],
        )
        .expect("commit");
        run(&repositories.local, "git", &["fetch", "origin", "main"]).expect("fetch");

        merge_origin_main(&repositories.local).expect("clean merge");

        let parents =
            run(&repositories.local, "git", &["log", "-1", "--format=%P"]).expect("parents");
        assert_eq!(
            parents.split_whitespace().count(),
            2,
            "a merge commit joins both sides"
        );
        assert_eq!(
            std::fs::read_to_string(repositories.local.join("note.md")).expect("note"),
            "My paragraph\n"
        );
        assert_eq!(
            std::fs::read_to_string(repositories.local.join("other.md")).expect("other"),
            "Remote paragraph\n"
        );
        assert!(
            run(&repositories.local, "git", &["status", "--porcelain"])
                .expect("status")
                .is_empty()
        );
        let head = run(&repositories.local, "git", &["rev-parse", "HEAD"]).expect("head");
        assert_eq!(
            crate::sync_review::outgoing_commits(&repositories.local, "origin/main", &head)
                .expect("the merge commit passes review"),
            2
        );
        repositories.assert_no_integration_state();
    }

    #[test]
    fn overlapping_sync_names_the_note_and_leaves_files_untouched() {
        let repositories = Repositories::new();
        repositories.remote_edit("note.md", "Remote paragraph\n");
        std::fs::write(repositories.local.join("note.md"), "My paragraph\n").expect("local edit");
        run(&repositories.local, "git", &["add", "note.md"]).expect("stage");
        run(
            &repositories.local,
            "git",
            &["commit", "-m", "notes: my edit"],
        )
        .expect("commit");
        run(&repositories.local, "git", &["fetch", "origin", "main"]).expect("fetch");
        let head = run(&repositories.local, "git", &["rev-parse", "HEAD"]).expect("head");

        let error = merge_origin_main(&repositories.local).expect_err("overlap is reported");
        let message = format!("{error:#}");
        assert!(message.contains("note.md"), "names the note: {message}");
        assert!(message.contains("sync again"), "says what to do: {message}");

        assert_eq!(
            run(&repositories.local, "git", &["rev-parse", "HEAD"]).expect("head"),
            head
        );
        assert_eq!(
            std::fs::read_to_string(repositories.local.join("note.md")).expect("note"),
            "My paragraph\n"
        );
        assert!(
            run(&repositories.local, "git", &["status", "--porcelain"])
                .expect("status")
                .is_empty()
        );
        repositories.assert_no_integration_state();
    }

    #[test]
    fn edits_arriving_during_sync_completion_are_not_discarded() {
        let repositories = Repositories::new();
        repositories.remote_edit("note.md", "Remote paragraph\n");
        let before = run(&repositories.local, "git", &["rev-parse", "HEAD"]).expect("head");
        let error = finish_sync(
            &repositories.local,
            "https://example.invalid/pull/1",
            &mut |step| {
                if step == "Updating local main" {
                    std::fs::write(
                        repositories.local.join("note.md"),
                        "Typed while GitHub checked the PR\n",
                    )
                    .expect("concurrent edit");
                    std::fs::write(repositories.local.join("new.md"), "Concurrent new file\n")
                        .expect("concurrent file");
                }
            },
        )
        .expect_err("stop before touching newer local work");
        assert!(error.to_string().contains("was merged on GitHub"));
        assert_eq!(
            std::fs::read_to_string(repositories.local.join("note.md")).expect("note"),
            "Typed while GitHub checked the PR\n"
        );
        assert_eq!(
            std::fs::read_to_string(repositories.local.join("new.md")).expect("new"),
            "Concurrent new file\n"
        );
        assert_eq!(
            run(&repositories.local, "git", &["rev-parse", "HEAD"]).expect("head"),
            before
        );
        repositories.assert_no_integration_state();
    }

    #[test]
    fn changing_branches_during_sync_does_not_update_the_new_branch() {
        let repositories = Repositories::new();
        repositories.remote_edit("note.md", "Remote paragraph\n");
        let before = run(&repositories.local, "git", &["rev-parse", "HEAD"]).expect("head");
        finish_sync(
            &repositories.local,
            "https://example.invalid/pull/1",
            &mut |_| {
                run(&repositories.local, "git", &["switch", "-c", "other-work"])
                    .expect("user switches branch");
            },
        )
        .expect_err("branch changed");
        assert_eq!(
            run(&repositories.local, "git", &["rev-parse", "HEAD"]).expect("head"),
            before
        );
        assert_eq!(
            run(&repositories.local, "git", &["branch", "--show-current"]).expect("branch"),
            "other-work"
        );
        repositories.assert_no_integration_state();
    }
}
