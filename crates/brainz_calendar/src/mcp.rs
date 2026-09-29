//! Brainz: an MCP connectors tab. Lists the MCP servers Claude (Brainz's own
//! config) and Codex know about, with app logos where we have them.

use std::path::PathBuf;



use gpui::{App, EventEmitter, FocusHandle, Focusable, Task, Window, actions};
use ui::{Tooltip, prelude::*};
use workspace::{HideStatusItem, Item, ItemHandle, StatusItemView, Workspace};

actions!(
    brainz_mcp,
    [
        /// Opens the MCP connectors tab.
        OpenMcp
    ]
);

pub fn init(cx: &mut App) {
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
    connectors: Vec<Connector>,
    loading: bool,
    error: Option<String>,
    _load: Option<Task<()>>,
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
            let view = cx.new(|cx| McpView::new(cx));
            workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
        }
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            connectors: Vec::new(),
            loading: true,
            error: None,
            _load: None,
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
        h_flex()
            .id(("brainz-mcp-connector", ix))
            .w_full()
            .gap_3()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().surface_background)
            .child(
                div()
                    .flex_none()
                    .size_10()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(cx.theme().colors().element_background)
                    .child(logo),
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
                    )),
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
                Label::new("Click a Claude or Codex badge to add that connector to it.")
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
}

impl McpButton {
    pub fn new() -> Self {
        Self {
            pane_item_focus_handle: None,
        }
    }
}

impl Render for McpButton {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.pane_item_focus_handle.clone();
        div().child(
            IconButton::new("brainz-mcp-button", IconName::BrainzMcp)
                .icon_size(IconSize::Small)
                .tooltip(move |_window, cx| {
                    if let Some(focus_handle) = &focus_handle {
                        Tooltip::for_action_in("MCP Connectors", &OpenMcp, focus_handle, cx)
                    } else {
                        Tooltip::for_action("MCP Connectors", &OpenMcp, cx)
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
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}
