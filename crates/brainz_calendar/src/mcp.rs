//! Brainz: an MCP connectors tab. Lists the MCP servers Claude (Brainz's own
//! config) and Codex know about, with app logos where we have them.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};



use anyhow::{Context as _, Result, anyhow};
use gpui::{
    App, Entity, EventEmitter, FocusHandle, Focusable, Global, Subscription, Task, WeakEntity,
    Window, actions,
};
use terminal_view::terminal_panel::TerminalPanel;
use ui::{ContextMenu, PopoverMenu, Tooltip, prelude::*};
use workspace::{HideStatusItem, Item, ItemHandle, StatusItemView, Workspace, dock::DockPosition};

/// Which CLI owns a connector's sign-in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum McpClient {
    Claude,
    Codex,
}

impl McpClient {
    pub fn name(self) -> &'static str {
        match self {
            McpClient::Claude => "Claude",
            McpClient::Codex => "Codex",
        }
    }

    /// The CLI the ACP adapter installed, falling back to whatever is on PATH.
    fn binary(self) -> String {
        let registry = paths::data_dir().join("external_agents/registry/npx");
        let bundled = match self {
            McpClient::Claude => registry.join(
                "claude-acp/node_modules/@anthropic-ai/claude-agent-sdk-darwin-arm64/claude",
            ),
            McpClient::Codex => registry.join(
                "codex-acp/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex",
            ),
        };
        if bundled.is_file() {
            bundled.to_string_lossy().into_owned()
        } else {
            match self {
                McpClient::Claude => "claude".to_owned(),
                McpClient::Codex => "codex".to_owned(),
            }
        }
    }

    fn env(self) -> collections::HashMap<String, String> {
        let (variable, dir) = match self {
            McpClient::Claude => ("CLAUDE_CONFIG_DIR", "claude"),
            McpClient::Codex => ("CODEX_HOME", "codex"),
        };
        let mut env = collections::HashMap::default();
        env.insert(
            variable.to_owned(),
            paths::config_dir().join(dir).to_string_lossy().into_owned(),
        );
        env
    }
}

fn claude_auth_cache_path() -> PathBuf {
    paths::config_dir().join("claude").join("mcp-needs-auth-cache.json")
}

/// Claude Code records the servers whose sign-in has lapsed in a small
/// cache file; Brainz reads it to show "sign-in needed" without probing.
pub fn needs_sign_in(client: McpClient, server: &str) -> bool {
    if client != McpClient::Claude {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(claude_auth_cache_path()) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value
        .as_object()
        .is_some_and(|map| map.keys().any(|key| key.eq_ignore_ascii_case(server)))
}

fn clear_sign_in_needed(server: &str) {
    let path = claude_auth_cache_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    if let Some(map) = value.as_object_mut() {
        map.retain(|key, _| !key.eq_ignore_ascii_case(server));
        if let Ok(text) = serde_json::to_string(&value) {
            std::fs::write(&path, text).ok();
        }
    }
}

/// The configured Claude connector names, for matching a name an agent
/// mentions ("granola") back to the real server key.
pub fn claude_server_names() -> Vec<String> {
    read_claude_servers().keys().cloned().collect()
}

/// Runs `claude mcp login <server>` (or the Codex equivalent) in the
/// terminal panel, which is shown while the browser sign-in happens and
/// swapped back for whatever panel was up when it finishes.
pub fn spawn_mcp_login(
    workspace: WeakEntity<Workspace>,
    client: McpClient,
    server: String,
    window: &mut Window,
    cx: &mut App,
) -> Task<Result<()>> {
    let Some(workspace_entity) = workspace.upgrade() else {
        return Task::ready(Err(anyhow!("workspace closed")));
    };
    let Some(terminal_panel) = workspace_entity.read(cx).panel::<TerminalPanel>(cx) else {
        return Task::ready(Err(anyhow!("Terminal panel is unavailable")));
    };
    let previous_panel = workspace_entity
        .read(cx)
        .dock_at_position(DockPosition::Bottom)
        .read(cx)
        .active_panel_index();
    let label = format!("Reconnect {server} ({})", client.name());
    let spawn = task::SpawnInTerminal {
        id: task::TaskId(format!("brainz-mcp-login-{}-{server}", client.name())),
        full_label: label.clone(),
        label: label.clone(),
        command: None,
        args: Vec::new(),
        command_label: label,
        env: client.env(),
        use_new_terminal: true,
        allow_concurrent_runs: true,
        hide: task::HideStrategy::Always,
        shell: task::Shell::WithArguments {
            program: client.binary(),
            args: vec!["mcp".to_owned(), "login".to_owned(), server.clone()],
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
        workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_panel::<TerminalPanel>(window, cx);
            })
            .ok();
        let exit = terminal
            .read_with(cx, |terminal, cx| terminal.wait_for_completed_task(cx))?
            .await;
        let result = match exit {
            Some(status) if status.success() => {
                if client == McpClient::Claude {
                    clear_sign_in_needed(&server);
                }
                Ok(())
            }
            Some(status) => Err(anyhow!(
                "`{} mcp login {server}` exited with {:?}",
                client.name().to_lowercase(),
                status.code()
            )),
            None => Err(anyhow!("the sign-in command ended without an exit status")),
        };
        workspace
            .update_in(cx, |workspace, window, cx| {
                if let Some(index) = previous_panel {
                    workspace
                        .dock_at_position(DockPosition::Bottom)
                        .update(cx, |dock, cx| dock.activate_panel(index, window, cx));
                }
            })
            .ok();
        result.with_context(|| format!("reconnecting {server}"))
    })
}

const HEALTH_INTERVAL: Duration = Duration::from_secs(60);

/// Brainz: reachability of each connector, refreshed every minute. Keyed by
/// connector name; `None` means healthy, `Some(reason)` means down.
pub struct McpHealth {
    results: BTreeMap<String, Option<String>>,
    _poll: Option<Task<()>>,
}

struct GlobalMcpHealth(Entity<McpHealth>);

impl Global for GlobalMcpHealth {}

pub fn health(cx: &App) -> Option<Entity<McpHealth>> {
    cx.try_global::<GlobalMcpHealth>().map(|g| g.0.clone())
}

impl McpHealth {
    pub fn failing(&self) -> Vec<(String, String)> {
        self.results
            .iter()
            .filter_map(|(name, result)| result.clone().map(|reason| (name.clone(), reason)))
            .collect()
    }

    fn status_of(&self, name: &str) -> Option<&Option<String>> {
        self.results.get(name)
    }

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        self._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let results = cx
                    .background_spawn(async move {
                        load_connectors()
                            .into_iter()
                            .map(|connector| {
                                let reason = probe(&connector);
                                (connector.name, reason)
                            })
                            .collect::<BTreeMap<_, _>>()
                    })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if this.results != results {
                            this.results = results;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(HEALTH_INTERVAL).await;
            }
        }));
    }
}

/// Checks one connector without touching its auth: an HTTP server counts as
/// up when it answers at all (401/403 from an OAuth server is fine); a stdio
/// server counts as up when its command can be found.
/// Blocking on purpose: probes only ever run on the background executor.
#[allow(clippy::disallowed_methods)]
fn probe(connector: &Connector) -> Option<String> {
    if connector.transport == "HTTP" || connector.transport == "SSE" {
        let url = connector.detail.trim();
        if url.is_empty() {
            return Some("no URL configured".into());
        }
        let output = std::process::Command::new("/usr/bin/curl")
            .args([
                "-sS",
                "-o",
                "/dev/null",
                "-w",
                "%{http_code}",
                "--max-time",
                "8",
                "-X",
                "POST",
                "-H",
                "content-type: application/json",
                "--data",
                "{}",
                url,
            ])
            .output();
        match output {
            Ok(output) if output.status.success() => {
                let code = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                match code.parse::<u16>() {
                    Ok(code) if code >= 500 => Some(format!("server error (HTTP {code})")),
                    Ok(0) | Err(_) => Some("no response".into()),
                    Ok(_) => None,
                }
            }
            Ok(output) => Some(
                String::from_utf8_lossy(&output.stderr)
                    .trim()
                    .trim_start_matches("curl: ")
                    .to_owned(),
            ),
            Err(error) => Some(format!("couldn't run curl: {error}")),
        }
    } else {
        let command = connector.detail.split_whitespace().next().unwrap_or_default();
        if command.is_empty() {
            return Some("no command configured".into());
        }
        let path = std::env::var("PATH").unwrap_or_default();
        let search = format!("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:{path}");
        let found = std::path::Path::new(command).is_file()
            || std::env::split_paths(&search).any(|dir| dir.join(command).is_file());
        if found {
            None
        } else {
            Some(format!("command `{command}` not found"))
        }
    }
}

actions!(
    brainz_mcp,
    [
        /// Opens the MCP connectors tab.
        OpenMcp
    ]
);

pub fn init(cx: &mut App) {
    let health = cx.new(|cx| {
        let mut health = McpHealth {
            results: BTreeMap::new(),
            _poll: None,
        };
        health.start_polling(cx);
        health
    });
    cx.set_global(GlobalMcpHealth(health));
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &OpenMcp, window, cx| {
            McpView::open(workspace, window, cx);
        });
    })
    .detach();
}

#[derive(Debug, Clone)]
struct Connector {
    name: String,
    transport: String,
    detail: String,
    logo: Option<&'static str>,
    /// The server as Claude's `.claude.json` has it, if present.
    claude: Option<serde_json::Value>,
    /// The server as Codex's `config.toml` has it, if present.
    codex: Option<toml::Value>,
}

impl Connector {
    fn key(name: &str) -> String {
        name.trim().to_lowercase().replace("-mcp", "").replace("_mcp", "")
    }
}

fn logo_for(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    if lower.contains("amplitude") {
        Some("icons/brainz/logos/amplitude.png")
    } else if lower.contains("posthog") {
        Some("icons/brainz/logos/posthog.png")
    } else if lower.contains("slack") {
        Some("icons/brainz/logos/slack.png")
    } else if lower.contains("granola") {
        Some("icons/brainz/logos/granola.png")
    } else if lower.contains("linear") {
        Some("icons/brainz/logos/linear.png")
    } else if lower.contains("figma") {
        Some("icons/brainz/logos/figma.png")
    } else if lower.contains("vapi") {
        Some("icons/brainz/logos/vapi.png")
    } else if lower.contains("atlassian") || lower.contains("jira") || lower.contains("confluence") {
        Some("icons/brainz/logos/atlassian.png")
    } else if lower.contains("glean") {
        Some("icons/brainz/logos/glean.png")
    } else {
        None
    }
}

fn claude_config_path() -> PathBuf {
    paths::config_dir().join("claude").join(".claude.json")
}

fn codex_config_path() -> PathBuf {
    paths::config_dir().join("codex").join("config.toml")
}

fn join_args(args: Option<&[String]>) -> String {
    args.map(|args| args.join(" ")).unwrap_or_default()
}

fn describe_claude(config: &serde_json::Value) -> (String, String) {
    let kind = config
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("stdio");
    match kind {
        "http" | "sse" => (
            kind.to_uppercase(),
            config
                .get("url")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned(),
        ),
        _ => {
            let command = config
                .get("command")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let args: Vec<String> = config
                .get("args")
                .and_then(|value| value.as_array())
                .map(|args| {
                    args.iter()
                        .filter_map(|arg| arg.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            (
                "STDIO".to_owned(),
                format!("{command} {}", join_args(Some(&args)))
                    .trim()
                    .to_owned(),
            )
        }
    }
}

fn describe_codex(config: &toml::Value) -> (String, String) {
    if let Some(url) = config.get("url").and_then(|value| value.as_str()) {
        return ("HTTP".to_owned(), url.to_owned());
    }
    let command = config
        .get("command")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let args: Vec<String> = config
        .get("args")
        .and_then(|value| value.as_array())
        .map(|args| {
            args.iter()
                .filter_map(|arg| arg.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    (
        "STDIO".to_owned(),
        format!("{command} {}", join_args(Some(&args)))
            .trim()
            .to_owned(),
    )
}

fn read_claude_servers() -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(claude_config_path())
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|json| json.get("mcpServers").and_then(|v| v.as_object()).cloned())
        .unwrap_or_default()
}

fn read_codex_servers() -> toml::map::Map<String, toml::Value> {
    std::fs::read_to_string(codex_config_path())
        .ok()
        .and_then(|text| text.parse::<toml::Value>().ok())
        .and_then(|value| value.get("mcp_servers").and_then(|v| v.as_table()).cloned())
        .unwrap_or_default()
}

fn load_connectors() -> Vec<Connector> {
    let mut by_key: std::collections::BTreeMap<String, Connector> = Default::default();

    for (name, config) in read_claude_servers() {
        let (transport, detail) = describe_claude(&config);
        by_key
            .entry(Connector::key(&name))
            .or_insert_with(|| Connector {
                logo: logo_for(&name),
                name: name.clone(),
                transport,
                detail,
                claude: None,
                codex: None,
            })
            .claude = Some(config);
    }
    for (name, config) in read_codex_servers() {
        let (transport, detail) = describe_codex(&config);
        let entry = by_key
            .entry(Connector::key(&name))
            .or_insert_with(|| Connector {
                logo: logo_for(&name),
                name: name.clone(),
                transport,
                detail,
                claude: None,
                codex: None,
            });
        entry.codex = Some(config);
    }
    by_key.into_values().collect()
}

/// Translates a Codex `[mcp_servers.x]` table into Claude's JSON shape.
fn codex_to_claude(config: &toml::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    if let Some(url) = config.get("url").and_then(|value| value.as_str()) {
        out.insert("type".into(), "http".into());
        out.insert("url".into(), url.into());
        if let Some(headers) = config.get("http_headers").and_then(|v| v.as_table()) {
            let headers: serde_json::Map<String, serde_json::Value> = headers
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.into())))
                .collect();
            out.insert("headers".into(), headers.into());
        }
    } else {
        out.insert("type".into(), "stdio".into());
        if let Some(command) = config.get("command").and_then(|value| value.as_str()) {
            out.insert("command".into(), command.into());
        }
        if let Some(args) = config.get("args").and_then(|value| value.as_array()) {
            let args: Vec<serde_json::Value> = args
                .iter()
                .filter_map(|arg| arg.as_str().map(|arg| arg.into()))
                .collect();
            out.insert("args".into(), args.into());
        }
        if let Some(env) = config.get("env").and_then(|value| value.as_table()) {
            let env: serde_json::Map<String, serde_json::Value> = env
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.into())))
                .collect();
            out.insert("env".into(), env.into());
        }
    }
    serde_json::Value::Object(out)
}

/// Translates a Claude `mcpServers.x` object into Codex's TOML shape.
fn claude_to_codex(config: &serde_json::Value) -> toml::Value {
    let mut out = toml::map::Map::new();
    let kind = config
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("stdio");
    if matches!(kind, "http" | "sse") {
        if let Some(url) = config.get("url").and_then(|value| value.as_str()) {
            out.insert("url".into(), toml::Value::String(url.into()));
        }
        if let Some(headers) = config.get("headers").and_then(|v| v.as_object()) {
            let headers: toml::map::Map<String, toml::Value> = headers
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), toml::Value::String(v.into()))))
                .collect();
            out.insert("http_headers".into(), toml::Value::Table(headers));
        }
    } else {
        if let Some(command) = config.get("command").and_then(|value| value.as_str()) {
            out.insert("command".into(), toml::Value::String(command.into()));
        }
        if let Some(args) = config.get("args").and_then(|value| value.as_array()) {
            let args: Vec<toml::Value> = args
                .iter()
                .filter_map(|arg| arg.as_str().map(|arg| toml::Value::String(arg.into())))
                .collect();
            out.insert("args".into(), toml::Value::Array(args));
        }
        if let Some(env) = config.get("env").and_then(|v| v.as_object()) {
            let env: toml::map::Map<String, toml::Value> = env
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), toml::Value::String(v.into()))))
                .collect();
            out.insert("env".into(), toml::Value::Table(env));
        }
    }
    toml::Value::Table(out)
}

fn connect_claude(name: &str, config: serde_json::Value) -> anyhow::Result<()> {
    let path = claude_config_path();
    let mut json = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let object = json
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} is not a JSON object", path.display()))?;
    let servers = object
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("mcpServers is not an object"))?;
    servers.insert(name.to_owned(), config);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&json)?)?;
    Ok(())
}

fn connect_codex(name: &str, config: toml::Value) -> anyhow::Result<()> {
    let path = codex_config_path();
    let mut value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.parse::<toml::Value>().ok())
        .unwrap_or_else(|| toml::Value::Table(Default::default()));
    let table = value
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("{} is not a TOML table", path.display()))?;
    let servers = table
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Table(Default::default()));
    let servers = servers
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("mcp_servers is not a table"))?;
    let key = name.to_lowercase().replace(' ', "-");
    servers.insert(key, config);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, toml::to_string_pretty(&value)?)?;
    Ok(())
}

pub struct McpView {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    connectors: Vec<Connector>,
    /// Connectors with a sign-in running right now.
    reconnecting: std::collections::HashSet<String>,
    _reconnect: Option<Task<()>>,
    loading: bool,
    error: Option<String>,
    _load: Option<Task<()>>,
    _health_subscription: Option<Subscription>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Client {
    Claude,
    Codex,
}

impl McpView {
    pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        let existing = workspace.items_of_type::<McpView>(cx).next();
        if let Some(existing) = existing {
            workspace.activate_item(&existing, true, true, window, cx);
            existing.update(cx, |view, cx| view.refresh(cx));
        } else {
            let weak = cx.entity().downgrade();
            let view = cx.new(|cx| McpView::new(weak, cx));
            workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
        }
    }

    fn new(workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        let health_subscription =
            health(cx).map(|health| cx.observe(&health, |_, _, cx| cx.notify()));
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            workspace,
            connectors: Vec::new(),
            reconnecting: std::collections::HashSet::new(),
            _reconnect: None,
            loading: true,
            error: None,
            _load: None,
            _health_subscription: health_subscription,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();
        self._load = Some(cx.spawn(async move |this, cx| {
            let connectors = cx.background_spawn(async { load_connectors() }).await;
            this.update(cx, |this, cx| {
                this.connectors = connectors;
                this.loading = false;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Adds the server to the other client's config, translated to its shape.
    fn connect(&mut self, ix: usize, client: Client, cx: &mut Context<Self>) {
        let Some(connector) = self.connectors.get(ix).cloned() else {
            return;
        };
        let result = match client {
            Client::Claude => match (&connector.claude, &connector.codex) {
                (Some(_), _) => Ok(()),
                (None, Some(codex)) => connect_claude(&connector.name, codex_to_claude(codex)),
                (None, None) => Ok(()),
            },
            Client::Codex => match (&connector.codex, &connector.claude) {
                (Some(_), _) => Ok(()),
                (None, Some(claude)) => connect_codex(&connector.name, claude_to_codex(claude)),
                (None, None) => Ok(()),
            },
        };
        match result {
            Ok(()) => {
                self.error = None;
                self.refresh(cx);
            }
            Err(error) => {
                log::error!("brainz mcp connect: {error:#}");
                self.error = Some(format!("Couldn't update the config: {error:#}"));
                cx.notify();
            }
        }
    }

    fn render_client_badge(
        &self,
        ix: usize,
        client: Client,
        connected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (icon, name) = match client {
            Client::Claude => (IconName::BrainzClaude, "Claude"),
            Client::Codex => (IconName::BrainzCodex, "Codex"),
        };
        let tooltip = if connected {
            format!("Connected to {name}")
        } else {
            format!("Connect to {name}")
        };
        h_flex()
            .id(("brainz-mcp-client", ix * 2 + (client == Client::Codex) as usize))
            .gap_1()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(if connected {
                cx.theme().colors().border_focused
            } else {
                cx.theme().colors().border
            })
            .bg(if connected {
                cx.theme().colors().element_selected
            } else {
                cx.theme().colors().element_background
            })
            .when(!connected, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(cx.theme().colors().element_hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.connect(ix, client, cx)))
            })
            .tooltip(Tooltip::text(tooltip))
            .child(
                Icon::new(icon)
                    .size(IconSize::Small)
                    .color(if connected { Color::Default } else { Color::Muted }),
            )
            .child(
                Label::new(name)
                    .size(LabelSize::XSmall)
                    .color(if connected { Color::Default } else { Color::Muted }),
            )
            .when(connected, |this| {
                this.child(
                    Icon::new(IconName::Check)
                        .size(IconSize::XSmall)
                        .color(Color::Success),
                )
            })
    }

    fn reconnect(&mut self, client: McpClient, server: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.reconnecting.contains(&server) {
            return;
        }
        self.reconnecting.insert(server.clone());
        cx.notify();
        let task = spawn_mcp_login(self.workspace.clone(), client, server.clone(), window, cx);
        self._reconnect = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.reconnecting.remove(&server);
                if let Err(error) = result {
                    this.error = Some(format!("{error:#}"));
                }
                this.refresh(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    fn render_connector_menu(&self, ix: usize, connector: &Connector, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.weak_entity();
        let name = connector.name.clone();
        let has_claude = connector.claude.is_some();
        let has_codex = connector.codex.is_some();
        PopoverMenu::new(("brainz-mcp-menu", ix))
            .trigger(
                IconButton::new(("brainz-mcp-more", ix), IconName::Ellipsis)
                    .icon_size(IconSize::Small)
                    .icon_color(Color::Muted)
                    .tooltip(Tooltip::text("Reconnect or sign in again")),
            )
            .anchor(gpui::Anchor::TopRight)
            .menu(move |window, cx| {
                let view = view.clone();
                let name = name.clone();
                Some(ContextMenu::build(window, cx, move |mut menu, _, _| {
                    menu = menu.header("Sign in again");
                    for (client, present) in [(McpClient::Claude, has_claude), (McpClient::Codex, has_codex)] {
                        if !present {
                            continue;
                        }
                        let view = view.clone();
                        let name = name.clone();
                        menu = menu.entry(
                            format!("Reconnect in {}", client.name()),
                            None,
                            move |window, cx| {
                                view.update(cx, |view, cx| view.reconnect(client, name.clone(), window, cx))
                                    .ok();
                            },
                        );
                    }
                    menu
                }))
            })
    }

    fn render_connector(
        &self,
        ix: usize,
        connector: &Connector,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let logo: gpui::AnyElement = match connector.logo {
            Some(path) => gpui::img(path.to_owned())
                .size_8()
                .object_fit(gpui::ObjectFit::Contain)
                .into_any_element(),
            None => Icon::new(IconName::BrainzMcp)
                .size(IconSize::Medium)
                .color(Color::Muted)
                .into_any_element(),
        };
        let status = health(cx).and_then(|health| health.read(cx).status_of(&connector.name).cloned());
        let sign_in_needed = connector.claude.is_some() && needs_sign_in(McpClient::Claude, &connector.name);
        let reconnecting = self.reconnecting.contains(&connector.name);
        let (dot_color, status_text): (gpui::Hsla, SharedString) = match &status {
            Some(Some(reason)) => (
                cx.theme().status().error,
                format!("Down: {reason}").into(),
            ),
            _ if sign_in_needed => (
                cx.theme().colors().text_accent,
                format!("Claude needs to sign in to {} again", connector.name).into(),
            ),
            Some(None) => (cx.theme().status().success, "Reachable and signed in".into()),
            None => (cx.theme().colors().icon_muted, "Checking…".into()),
        };
        h_flex()
            .id(("brainz-mcp-connector", ix))
            .w_full()
            .gap_3()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(if matches!(status, Some(Some(_))) {
                cx.theme().status().error.opacity(0.6)
            } else {
                cx.theme().colors().border
            })
            .bg(cx.theme().colors().surface_background)
            .child(
                div()
                    .flex_none()
                    .relative()
                    .size_10()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(cx.theme().colors().element_background)
                    .child(logo)
                    .child(
                        div()
                            .id(("brainz-mcp-status", ix))
                            .absolute()
                            .bottom_neg_0p5()
                            .right_neg_0p5()
                            .size_3()
                            .rounded_full()
                            .border_2()
                            .border_color(cx.theme().colors().surface_background)
                            .bg(dot_color)
                            .tooltip(Tooltip::text(status_text)),
                    ),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Label::new(connector.name.clone())
                                    .weight(gpui::FontWeight::SEMIBOLD),
                            )
                            .when(!connector.transport.is_empty(), |this| {
                                this.child(
                                    Label::new(connector.transport.clone())
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            }),
                    )
                    .when(!connector.detail.is_empty(), |this| {
                        this.child(
                            Label::new(connector.detail.clone())
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate(),
                        )
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_1p5()
                    .when(sign_in_needed && !reconnecting, |this| {
                        let name = connector.name.clone();
                        this.child(
                            Button::new(("brainz-mcp-reconnect", ix), "Reconnect")
                                .style(ButtonStyle::Filled)
                                .label_size(LabelSize::Small)
                                .tooltip(Tooltip::text(format!(
                                    "Sign in to {} again; a terminal opens with the link",
                                    connector.name
                                )))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.reconnect(McpClient::Claude, name.clone(), window, cx);
                                })),
                        )
                    })
                    .when(reconnecting, |this| {
                        this.child(
                            h_flex().h(px(24.)).px_2().items_center().child(ui::bouncing_dots(
                                format!("brainz-mcp-reconnecting-{ix}"),
                                cx.theme().colors().text_accent,
                            )),
                        )
                    })
                    .child(self.render_client_badge(
                        ix,
                        Client::Claude,
                        connector.claude.is_some(),
                        cx,
                    ))
                    .child(self.render_client_badge(
                        ix,
                        Client::Codex,
                        connector.codex.is_some(),
                        cx,
                    ))
                    .child(self.render_connector_menu(ix, connector, cx)),
            )
    }
}

impl EventEmitter<()> for McpView {}

impl Focusable for McpView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for McpView {
    type Event = ();

    fn to_item_events(_: &Self::Event, _: &mut dyn FnMut(workspace::item::ItemEvent)) {}

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "MCP Connectors".into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::BrainzMcp))
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        None
    }
}

impl Render for McpView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::BrainzMcp).color(Color::Accent))
                    .child(Label::new("MCP Connectors").size(LabelSize::Large)),
            )
            .child(
                IconButton::new("brainz-mcp-refresh", IconName::ArrowCircle)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text("Refresh"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );

        let body: Vec<gpui::AnyElement> = if self.loading && self.connectors.is_empty() {
            vec![
                Label::new("Reading connector configs…")
                    .color(Color::Muted)
                    .into_any_element(),
            ]
        } else if self.connectors.is_empty() {
            vec![
                v_flex()
                    .gap_2()
                    .child(Label::new("No MCP connectors yet."))
                    .child(
                        Label::new(
                            "Add servers with `claude mcp add …` in a Brainz shell. Connectors \
                             from your terminal Claude are copied in the first time Claude runs \
                             here.",
                        )
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    )
                    .into_any_element(),
            ]
        } else {
            let mut rows: Vec<gpui::AnyElement> = Vec::new();
            if let Some(error) = &self.error {
                rows.push(Label::new(error.clone()).color(Color::Error).into_any_element());
            }
            rows.push(
                Label::new("Click a Claude or Codex badge to add that connector to it. An amber dot means Claude's sign-in to that connector has lapsed; Reconnect opens the sign-in.")
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .into_any_element(),
            );
            let connectors = self.connectors.clone();
            for (ix, connector) in connectors.iter().enumerate() {
                rows.push(self.render_connector(ix, connector, cx).into_any_element());
            }
            rows
        };

        v_flex()
            .id("brainz-mcp")
            .key_context("BrainzMcp")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .bg(cx.theme().colors().editor_background)
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(760.))
                    .mx_auto()
                    .pt_6()
                    .pb_10()
                    .px_4()
                    .gap_3()
                    .child(header)
                    .children(body),
            )
    }
}

/// Status bar button that opens the MCP connectors tab.
pub struct McpButton {
    pane_item_focus_handle: Option<FocusHandle>,
    /// The MCP tab is the active item, so the button lights up.
    active: bool,
    _health_subscription: Option<Subscription>,
}

impl McpButton {
    pub fn new() -> Self {
        Self {
            pane_item_focus_handle: None,
            active: false,
            _health_subscription: None,
        }
    }
}

impl Render for McpButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.pane_item_focus_handle.clone();
        let active = self.active;
        let failing = health(cx)
            .map(|health| {
                if self._health_subscription.is_none() {
                    self._health_subscription =
                        Some(cx.observe(&health, |_, _, cx| cx.notify()));
                }
                health.read(cx).failing()
            })
            .unwrap_or_default();
        let trouble = !failing.is_empty();
        let tooltip_title: SharedString = if trouble {
            format!(
                "MCP: {} down ({})",
                failing.len(),
                failing
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .into()
        } else {
            "MCP Connectors".into()
        };
        div().child(
            IconButton::new("brainz-mcp-button", IconName::BrainzMcp)
                .icon_size(IconSize::Small)
                .toggle_state(active)
                .icon_color(if trouble {
                    Color::Error
                } else if active {
                    Color::Accent
                } else {
                    Color::Default
                })
                .tooltip(move |_window, cx| {
                    if let Some(focus_handle) = &focus_handle {
                        Tooltip::for_action_in(tooltip_title.clone(), &OpenMcp, focus_handle, cx)
                    } else {
                        Tooltip::for_action(tooltip_title.clone(), &OpenMcp, cx)
                    }
                })
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(OpenMcp), cx);
                }),
        )
    }
}

impl StatusItemView for McpButton {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_item_focus_handle = active_pane_item.map(|item| item.item_focus_handle(cx));
        self.active = active_pane_item.is_some_and(|item| item.downcast::<McpView>().is_some());
        cx.notify();
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}
