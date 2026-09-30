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
    Animation, AnimationExt, AnyWindowHandle, App, DismissEvent, Entity, EventEmitter,
    FocusHandle, Focusable, Global, Task, WeakEntity, Window, ease_out_quint,
};
use ui::{Tooltip, prelude::*};
use workspace::{ModalView, Workspace};

const POLL_INTERVAL: Duration = Duration::from_secs(20);
const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
const MAX_SCAN_BYTES: u64 = 2 * 1024 * 1024;

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

const BANNER_HEIGHT: f32 = 74.;
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

fn command(repo: &Path, program: &str) -> Command {
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
fn run(repo: &Path, program: &str, args: &[&str]) -> Result<String> {
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

/// Brings origin/main down. Rebases any local commits on top and stashes
/// uncommitted edits around it, so nothing local is lost.
fn pull_repo(repo: &Path, progress: &mut dyn FnMut(String)) -> Result<String> {
    progress("Checking branch".into());
    let branch = run(repo, "git", &["branch", "--show-current"])?;
    if branch != "main" {
        bail!("You're on branch `{branch}`. Switch to `main` first, then pull.");
    }
    progress("Pulling from GitHub".into());
    run(
        repo,
        "git",
        &["pull", "--rebase", "--autostash", "origin", "main"],
    )
    .context("Pull didn't finish. If there's a conflict, resolve it in a shell and run `git rebase --continue`.")?;
    let head = run(repo, "git", &["log", "--oneline", "-1"]).unwrap_or_default();
    Ok(format!("Pulled the latest from GitHub. Now at {head}"))
}

/// The review step: refuse to publish secrets or huge files.
fn review(repo: &Path) -> Result<()> {
    let porcelain = run(repo, "git", &["status", "--porcelain"])?;
    let secret_markers: &[(&str, &str)] = &[
        ("-----BEGIN ", "a private key"),
        ("sk-ant-", "an Anthropic API key"),
        ("sk-proj-", "an OpenAI API key"),
        ("ghp_", "a GitHub token"),
        ("github_pat_", "a GitHub token"),
        ("xoxb-", "a Slack bot token"),
        ("xoxp-", "a Slack user token"),
        ("AKIA", "an AWS access key"),
        ("phx_", "a PostHog personal API key"),
    ];
    let mut problems = Vec::new();
    for line in porcelain.lines() {
        if line.len() < 4 || line.starts_with(" D") || line.starts_with("D ") {
            continue;
        }
        let rel = line[3..].split(" -> ").last().unwrap_or("").trim_matches('"');
        let path = repo.join(rel);
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if name == ".env"
            || name.starts_with(".env.")
            || name.ends_with(".pem")
            || name.ends_with(".p12")
            || name == "id_rsa"
            || name == "id_ed25519"
        {
            problems.push(format!("{rel} looks like a credentials file"));
            continue;
        }
        if metadata.len() > MAX_FILE_BYTES {
            problems.push(format!(
                "{rel} is {} MB, too large for GitHub",
                metadata.len() / (1024 * 1024)
            ));
            continue;
        }
        if metadata.len() <= MAX_SCAN_BYTES
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            for (marker, what) in secret_markers {
                if text.contains(marker) {
                    problems.push(format!("{rel} appears to contain {what} ({marker}…)"));
                    break;
                }
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        bail!(
            "Review stopped the sync. Fix these and try again:\n\n• {}",
            problems.join("\n• ")
        )
    }
}

fn sync_repo(repo: &Path, progress: &mut dyn FnMut(String)) -> Result<String> {
    progress("Checking branch".into());
    let branch = run(repo, "git", &["branch", "--show-current"])?;
    if branch != "main" {
        bail!("You're on branch `{branch}`. Switch to `main` first, then sync.");
    }
    run(repo, "gh", &["auth", "status"]).context("GitHub CLI isn't signed in. Run `gh auth login` in a shell.")?;

    progress("Reviewing changes".into());
    review(repo)?;

    let status = check_status(repo, true)?;
    let SyncStatus::Pending {
        changed,
        ahead,
        behind,
    } = status
    else {
        return Ok("Already in sync with GitHub.".into());
    };
    if behind > 0 {
        // Bring GitHub's newer work down first so the PR merges cleanly.
        pull_repo(repo, progress)?;
    }
    if changed == 0 && ahead == 0 {
        return Ok("Pulled the latest from GitHub. Nothing local to push.".into());
    }

    if changed > 0 {
        progress(format!("Committing {changed} change(s)"));
        run(repo, "git", &["add", "-A"])?;
        let summary = run(repo, "git", &["diff", "--cached", "--stat"])?;
        let message = format!("Sync from Brainz\n\n{summary}");
        run(repo, "git", &["commit", "-m", &message])?;
    }

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let sync_branch = format!("brainz/sync-{stamp}");
    progress("Pushing branch".into());
    run(repo, "git", &["branch", &sync_branch])?;
    run(repo, "git", &["push", "-u", "origin", &sync_branch])?;

    progress("Opening pull request".into());
    let commits = run(
        repo,
        "git",
        &["log", "--oneline", "origin/main..HEAD"],
    )
    .unwrap_or_default();
    let body = format!(
        "Synced from Brainz.\n\nCommits:\n{}\n\nReviewed automatically: no credential files, secret markers, or files over 25 MB.",
        commits
            .lines()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let title = if ahead + usize::from(changed > 0) == 1 {
        "Sync from Brainz".to_owned()
    } else {
        format!("Sync from Brainz ({} commits)", ahead + usize::from(changed > 0))
    };
    let pr_url = run(
        repo,
        "gh",
        &[
            "pr", "create", "--base", "main", "--head", &sync_branch, "--title", &title, "--body",
            &body,
        ],
    )?;

    progress("Checking the pull request".into());
    let mut mergeable = false;
    for _ in 0..20 {
        let state = run(
            repo,
            "gh",
            &["pr", "view", &sync_branch, "--json", "mergeable", "--jq", ".mergeable"],
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
        bail!("GitHub hasn't finished checking the pull request. Open it and merge when ready:\n{pr_url}");
    }

    progress("Merging into main".into());
    run(
        repo,
        "gh",
        &["pr", "merge", &sync_branch, "--merge", "--delete-branch"],
    )?;

    progress("Updating local main".into());
    run(repo, "git", &["fetch", "--prune", "origin"])?;
    if run(repo, "git", &["merge", "--ff-only", "origin/main"]).is_err() {
        run(repo, "git", &["reset", "--hard", "origin/main"])?;
    }
    let _ = run(repo, "git", &["branch", "-D", &sync_branch]);
    Ok(format!("Merged into main: {pr_url}"))
}

impl SyncState {
    fn ensure_watching(&mut self, workspace: WeakEntity<Workspace>, repo: PathBuf, cx: &mut Context<Self>) {
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
pub struct SyncErrorModal {
    message: String,
    state: Entity<SyncState>,
    focus_handle: FocusHandle,
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
                    .child(Icon::new(IconName::Warning).color(Color::Warning))
                    .child(Label::new("Sync to GitHub didn't finish").weight(gpui::FontWeight::SEMIBOLD)),
            )
            .child(Label::new(self.message.clone()).size(LabelSize::Small))
            .child(
                h_flex().justify_end().gap_2().child(
                    Button::new("brainz-sync-error-ok", "OK")
                        .style(ButtonStyle::Filled)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |state, cx| state.dismiss_error(cx));
                            cx.emit(DismissEvent);
                        })),
                ),
            )
    }
}

/// The banner the project panel shows above the file tree.
pub fn render_banner(
    workspace: &WeakEntity<Workspace>,
    cx: &mut App,
) -> Option<gpui::AnyElement> {
    let state = state(cx)?;
    let repo = workspace
        .upgrade()?
        .read(cx)
        .root_paths(cx)
        .first()
        .map(|path| path.to_path_buf())?;
    state.update(cx, |state, cx| state.ensure_watching(workspace.clone(), repo, cx));
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
            let plural = |n: usize, word: &str| {
                format!("{n} {word}{}", if n == 1 { "" } else { "s" })
            };
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
        .w_full()
        .h(px(BANNER_HEIGHT))
        .overflow_hidden()
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
                    "Bring GitHub's newer commits down (rebases local work on top)"
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
            .w_full()
            .overflow_hidden()
            .child(banner)
            .with_animation(
                "brainz-github-sync-banner-out",
                Animation::new(Duration::from_millis(SLIDE_MS)).with_easing(ease_out_quint()),
                |wrapper, delta| {
                    let remaining = 1.0 - delta;
                    wrapper
                        .h(px(BANNER_HEIGHT * remaining))
                        .opacity(remaining)
                },
            )
            .into_any_element()
    } else {
        div()
            .w_full()
            .overflow_hidden()
            .child(banner)
            .with_animation(
                "brainz-github-sync-banner-in",
                Animation::new(Duration::from_millis(SLIDE_MS)).with_easing(ease_out_back),
                |wrapper, delta| {
                    wrapper
                        .h(px(BANNER_HEIGHT * delta))
                        .opacity(delta.min(1.0))
                },
            )
            .into_any_element()
    };

    Some(animated)
}
