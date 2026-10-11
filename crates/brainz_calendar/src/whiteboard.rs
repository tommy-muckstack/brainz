use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context as _, Result, anyhow, ensure};
use base64::Engine as _;
use gpui::{
    App, EventEmitter, FocusHandle, Focusable, Subscription, Task, WeakEntity, Window, actions,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ui::prelude::*;
use util::ResultExt;
use workspace::{Item, Workspace};

const MAX_DOCUMENT_BYTES: usize = 24 * 1024 * 1024;

actions!(brainz_whiteboard, [OpenWhiteboard]);

#[derive(Clone, PartialEq, Debug, Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = brainz_whiteboard)]
pub struct AttachCanvasToChat {
    pub text: String,
    pub png: Option<String>,
}

#[derive(Clone, PartialEq, Debug, Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = brainz_whiteboard)]
pub struct ReferenceCanvas {
    pub path: PathBuf,
}

pub fn canvas_title(path: &Path) -> Result<String> {
    let document = read_document(path, false)?;
    Ok(document
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("Untitled canvas")
        .to_owned())
}

fn canvas_file_references(paths: Vec<PathBuf>) -> Vec<Value> {
    let mut image_bytes = 0;
    let mut files = Vec::new();
    for path in paths {
        if !path.is_file() {
            continue;
        }
        let Ok(file_url) = url::Url::from_file_path(&path) else {
            continue;
        };
        let Some(title) = path.file_name() else {
            continue;
        };
        let mut reference = json!({"title": title.to_string_lossy(), "url": file_url.as_str()});
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mime = match extension.as_str() {
            "png" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "webp" => Some("image/webp"),
            "gif" => Some("image/gif"),
            _ => None,
        };
        if let Some(mime) = mime {
            let image = std::fs::metadata(&path).and_then(|metadata| {
                if metadata.len() > 8 * 1024 * 1024
                    || image_bytes + metadata.len() > 16 * 1024 * 1024
                {
                    return Err(std::io::Error::other(
                        "Image is too large to embed; keeping a file link",
                    ));
                }
                std::fs::read(&path)
            });
            match image {
                Ok(bytes) => {
                    image_bytes += bytes.len() as u64;
                    reference["image"] = json!({"mime": mime, "data": base64::engine::general_purpose::STANDARD.encode(bytes)});
                }
                Err(error) => log::warn!("Canvas image stays a file link: {error}"),
            }
        }
        files.push(reference);
    }
    files
}

fn write_json_atomically(path: &Path, value: &Value) -> Result<()> {
    let parent = path.parent().context("Canvas has no containing folder")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temporary, value)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BoardTarget {
    pub path: PathBuf,
    pub object: Option<String>,
    pub viewport: Option<[f64; 3]>,
}

pub fn parse_link(value: &str) -> Result<BoardTarget> {
    let url = url::Url::parse(value)?;
    ensure!(
        url.scheme() == "brainz" && url.host_str() == Some("whiteboard"),
        "Not a whiteboard link"
    );
    let parameters: std::collections::HashMap<_, _> = url.query_pairs().collect();
    let path = PathBuf::from(
        parameters
            .get("path")
            .context("Whiteboard link has no file")?
            .as_ref(),
    );
    ensure!(
        path.is_absolute()
            && path
                .extension()
                .is_some_and(|extension| extension == "brainzboard"),
        "Invalid whiteboard file"
    );
    let object = parameters.get("object").map(|object| object.to_string());
    ensure!(
        object
            .as_ref()
            .is_none_or(|object| !object.is_empty() && object.len() <= 256),
        "Invalid object link"
    );
    let viewport = if parameters.contains_key("x")
        || parameters.contains_key("y")
        || parameters.contains_key("zoom")
    {
        let read = |key: &str| -> Result<f64> {
            let number = parameters
                .get(key)
                .context("Incomplete canvas position")?
                .parse::<f64>()?;
            ensure!(number.is_finite(), "Invalid canvas position");
            Ok(number)
        };
        let viewport = [read("x")?, read("y")?, read("zoom")?];
        ensure!((0.05..=10.).contains(&viewport[2]), "Invalid canvas zoom");
        Some(viewport)
    } else {
        None
    };
    Ok(BoardTarget {
        path,
        object,
        viewport,
    })
}

fn read_document(path: &Path, allow_create: bool) -> Result<Value> {
    match std::fs::metadata(path) {
        Ok(metadata) => ensure!(
            metadata.len() <= MAX_DOCUMENT_BYTES as u64,
            "This board is too large to open"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_create => {
            return Ok(Value::Null);
        }
        Err(error) => return Err(error.into()),
    }
    let bytes = std::fs::read(path)?;
    let document: Value = serde_json::from_slice(&bytes)
        .context("This whiteboard file could not be read. Its contents have been preserved.")?;
    validate_document(&document)?;
    Ok(document)
}

fn validate_document(document: &Value) -> Result<()> {
    ensure!(
        document.get("format").and_then(Value::as_str) == Some("brainz-whiteboard"),
        "This is not a Brainz whiteboard"
    );
    ensure!(
        document.get("version").and_then(Value::as_u64) == Some(1),
        "This board requires a different version of Brainz"
    );
    ensure!(
        document.get("data").and_then(Value::as_str).is_some(),
        "The whiteboard document is missing"
    );
    Ok(())
}

#[derive(Default)]
struct PendingDocument {
    revision: u64,
    document: Option<Value>,
    saved_revision: Option<u64>,
}

struct BoardStorage {
    path: PathBuf,
    pending: Mutex<PendingDocument>,
    file_lock: Mutex<Option<std::fs::File>>,
}

impl BoardStorage {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            pending: Mutex::new(PendingDocument::default()),
            file_lock: Mutex::new(None),
        }
    }

    fn load(&self, allow_create: bool) -> Result<Value> {
        if allow_create {
            std::fs::create_dir_all(
                self.path
                    .parent()
                    .context("Canvas has no containing folder")?,
            )?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.path.with_extension("brainzboard.lock"))?;
        file.try_lock().context("This canvas is already open in another Brainz window. Close that tab before opening another copy.")?;
        *self
            .file_lock
            .lock()
            .map_err(|_| anyhow!("Canvas storage is unavailable"))? = Some(file);
        read_document(&self.path, allow_create)
    }

    fn queue(&self, revision: u64, document: Value) -> Result<()> {
        validate_document(&document)?;
        ensure!(
            serde_json::to_vec(&document)?.len() <= MAX_DOCUMENT_BYTES,
            "The whiteboard has reached its size limit"
        );
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow!("Whiteboard storage is unavailable"))?;
        if revision > pending.revision || pending.document.is_none() {
            pending.revision = revision;
            pending.document = Some(document);
        }
        Ok(())
    }

    fn save(&self) -> Result<u64> {
        // A closing tab and app shutdown can both flush. The lock also prevents
        // an older background write from replacing a more recent document.
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow!("Whiteboard storage is unavailable"))?;
        if pending.saved_revision == Some(pending.revision) || pending.document.is_none() {
            return Ok(pending.revision);
        }
        let parent = self
            .path
            .parent()
            .context("Whiteboard has no containing folder")?;
        std::fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(&mut temporary, &pending.document)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        if let Some(scene) = pending
            .document
            .as_ref()
            .and_then(|document| document.get("scene"))
        {
            write_json_atomically(&self.path.with_extension("canvas.json"), scene)?;
        }
        pending.saved_revision = Some(pending.revision);
        Ok(pending.revision)
    }
}

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &OpenWhiteboard, window, cx| {
            let path = workspace
                .root_paths(cx)
                .first()
                .map(|root| root.join(".brainz/whiteboard.brainzboard"))
                .unwrap_or_else(|| paths::data_dir().join("whiteboards/Personal.brainzboard"));
            WhiteboardView::open(
                workspace,
                BoardTarget {
                    path,
                    object: None,
                    viewport: None,
                },
                true,
                None,
                window,
                cx,
            );
        });
        workspace.register_action(|workspace, action: &ReferenceCanvas, window, cx| {
            if !action.path.is_absolute()
                || action
                    .path
                    .extension()
                    .is_none_or(|extension| extension != "brainzboard")
            {
                return;
            }
            WhiteboardView::open(
                workspace,
                BoardTarget {
                    path: action.path.clone(),
                    object: None,
                    viewport: None,
                },
                false,
                Some(window.window_handle()),
                window,
                cx,
            );
        });
    })
    .detach();
}

pub struct WhiteboardView {
    this: WeakEntity<Self>,
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    target: BoardTarget,
    storage: Arc<BoardStorage>,
    document: Option<Value>,
    error: Option<SharedString>,
    chat_destination: Option<gpui::AnyWindowHandle>,
    _load: Task<()>,
    _subscriptions: Vec<Subscription>,
    bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    #[cfg(target_os = "macos")]
    webview: Option<std::rc::Rc<wry::WebView>>,
    #[cfg(target_os = "macos")]
    messages: Option<Task<()>>,
}

impl WhiteboardView {
    pub fn open_target(
        workspace: &mut Workspace,
        target: BoardTarget,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        Self::open(workspace, target, false, None, window, cx);
    }

    fn open(
        workspace: &mut Workspace,
        mut target: BoardTarget,
        allow_create: bool,
        chat_destination: Option<gpui::AnyWindowHandle>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        if let Ok(path) = target.path.canonicalize() {
            target.path = path;
        }
        let existing = workspace
            .items_of_type::<Self>(cx)
            .find(|view| view.read(cx).target.path == target.path);
        if let Some(existing) = existing {
            workspace.activate_item(&existing, true, true, window, cx);
            existing.update(cx, |view, cx| {
                view.target = target;
                view.send(json!({"type": "focus", "target": view.target}));
                if let Some(destination) = chat_destination {
                    view.chat_destination = Some(destination);
                    view.send(json!({"type": "attach"}));
                }
                cx.notify();
            });
            return;
        }
        let current_workspace = cx.entity_id();
        let other = workspace::AppState::try_global(cx).and_then(|state| {
            state
                .workspace_store
                .read(cx)
                .workspaces_with_windows()
                .find_map(|(handle, other)| {
                    let other = other.upgrade()?;
                    if other.entity_id() == current_workspace {
                        return None;
                    }
                    let view = other
                        .read(cx)
                        .items_of_type::<Self>(cx)
                        .find(|view| view.read(cx).target.path == target.path)?;
                    Some((handle, other, view))
                })
        });
        if let Some((handle, other, view)) = other {
            // Defer until the current workspace's action handler releases its lease.
            cx.defer(move |cx| {
                handle
                    .update(cx, |_, window, cx| {
                        let multi_workspace = other
                            .read(cx)
                            .multi_workspace()
                            .and_then(WeakEntity::upgrade);
                        if let Some(multi_workspace) = multi_workspace {
                            multi_workspace.update(cx, |multi_workspace, cx| {
                                multi_workspace.activate(other.clone(), None, window, cx)
                            });
                        }
                        other.update(cx, |workspace, cx| {
                            workspace.activate_item(&view, true, true, window, cx)
                        });
                        view.update(cx, |view, cx| {
                            view.target = target;
                            view.send(json!({"type":"focus", "target":view.target}));
                            if let Some(destination) = chat_destination {
                                view.chat_destination = Some(destination);
                                view.send(json!({"type":"attach"}));
                            }
                            cx.notify();
                        });
                        window.activate_window();
                    })
                    .log_err();
            });
            return;
        }
        let workspace_handle = cx.entity().downgrade();
        let view = cx.new(|cx| {
            let mut view = Self::new(workspace_handle, target, allow_create, cx);
            view.chat_destination = chat_destination;
            view
        });
        workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
    }

    fn new(
        workspace: WeakEntity<Workspace>,
        target: BoardTarget,
        allow_create: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let storage = Arc::new(BoardStorage::new(target.path.clone()));
        let loading_storage = storage.clone();
        let load = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { loading_storage.load(allow_create) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(document) => this.document = Some(document),
                    Err(error) => {
                        this.error = Some(format!("Unable to open this whiteboard: {error}").into())
                    }
                }
                cx.notify();
            })
            .log_err();
        });
        let quit = cx.on_app_quit({
            let storage = storage.clone();
            move |_, cx| {
                let storage = storage.clone();
                cx.background_spawn(async move {
                    storage.save().log_err();
                })
            }
        });
        let release = cx.on_release({
            let storage = storage.clone();
            move |_, cx| {
                let storage = storage.clone();
                cx.background_spawn(async move {
                    storage.save().log_err();
                })
                .detach();
            }
        });
        let mut subscriptions = vec![quit, release];
        if let Some(workspace) = workspace.upgrade() {
            subscriptions.push(cx.observe(&workspace, |_, _, cx| cx.notify()));
        }
        Self {
            this: cx.weak_entity(),
            workspace,
            focus_handle: cx.focus_handle(),
            target,
            storage,
            document: None,
            error: None,
            chat_destination: None,
            _load: load,
            _subscriptions: subscriptions,
            bounds: Default::default(),
            #[cfg(target_os = "macos")]
            webview: None,
            #[cfg(target_os = "macos")]
            messages: None,
        }
    }

    fn send(&self, message: Value) {
        #[cfg(target_os = "macos")]
        if let Some(webview) = &self.webview {
            webview
                .evaluate_script(&format!(
                    "window.dispatchEvent(new CustomEvent('brainz', {{detail: {message}}}));"
                ))
                .log_err();
        }
        #[cfg(not(target_os = "macos"))]
        drop(message);
    }

    fn hide(&self) {
        #[cfg(target_os = "macos")]
        if let Some(webview) = &self.webview {
            webview.set_visible(false).log_err();
        }
    }

    fn send_files(&self, paths: Vec<PathBuf>, position: Option<[f64; 2]>, cx: &mut App) {
        let task = cx.background_spawn(async move { canvas_file_references(paths) });
        let this = self.this.clone();
        cx.spawn(async move |cx| {
            let files = task.await;
            this.update(cx, |this, _| {
                this.send(json!({"type": "files", "files": files, "position": position}))
            })
            .log_err();
        })
        .detach();
    }
}

impl EventEmitter<()> for WhiteboardView {}

impl Focusable for WhiteboardView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for WhiteboardView {
    type Event = ();
    fn to_item_events(_: &(), _: &mut dyn FnMut(workspace::item::ItemEvent)) {}
    fn tab_content_text(&self, _: usize, _: &App) -> SharedString {
        "Canvas".into()
    }
    fn tab_icon(&self, _: &Window, _: &App) -> Option<Icon> {
        Some(Icon::new(IconName::BrainzWhiteboard))
    }
    fn show_toolbar(&self) -> bool {
        false
    }
    fn deactivated(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.hide();
    }
    fn workspace_deactivated(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.hide();
    }
    fn on_removed(&self, _: &mut Context<Self>) {
        self.hide();
    }

    fn handle_drop(
        &self,
        _: &workspace::Pane,
        dropped: &dyn std::any::Any,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let paths =
            if let Some(selection) = dropped.downcast_ref::<workspace::pane::DraggedSelection>() {
                let Some(workspace) = self.workspace.upgrade() else {
                    return false;
                };
                let project = workspace.read(cx).project().read(cx);
                selection
                    .items()
                    .filter_map(|selection| {
                        let worktree = project.worktree_for_id(selection.worktree_id, cx)?.read(cx);
                        if !worktree.is_local() {
                            return None;
                        }
                        let entry = worktree.entry_for_id(selection.entry_id)?;
                        (!entry.is_dir()).then(|| worktree.absolutize(&entry.path))
                    })
                    .collect()
            } else if let Some(paths) = dropped.downcast_ref::<gpui::ExternalPaths>() {
                paths.paths().to_vec()
            } else {
                return false;
            };
        let position = window.mouse_position() - self.bounds.get().origin;
        self.send_files(
            paths,
            Some([f64::from(position.x), f64::from(position.y)]),
            cx,
        );
        true
    }
}

impl Render for WhiteboardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        if self.document.is_some() && self.webview.is_none() && self.error.is_none() {
            if let Err(error) = self.create_webview(window, cx) {
                self.error = Some(format!("Whiteboard could not start: {error}").into());
            }
        }
        let mut body = div()
            .id("brainz-whiteboard")
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .bg(cx.theme().colors().editor_background);
        if let Some(error) = &self.error {
            self.hide();
            return body
                .child(
                    v_flex()
                        .p_8()
                        .gap_3()
                        .child(Label::new("Whiteboard").size(LabelSize::Large))
                        .child(Label::new(error.clone())),
                )
                .into_any_element();
        }
        #[cfg(target_os = "macos")]
        if let Some(webview) = self.webview.clone() {
            let modal_open = self
                .workspace
                .upgrade()
                .is_some_and(|workspace| workspace.read(cx).has_active_modal(window, cx));
            // AppKit's child view cannot receive GPUI's in-window file drags. Let the
            // pane route the drop to Item::handle_drop while a GPUI drag is active.
            if modal_open || cx.has_active_drag() {
                self.hide();
                if !modal_open {
                    body = body.child(
                        v_flex()
                            .size_full()
                            .items_center()
                            .justify_center()
                            .gap_3()
                            .child(Icon::new(IconName::BrainzWhiteboard))
                            .child(Label::new("Drop files to add them to your canvas")),
                    );
                }
            } else {
                let view_bounds = self.bounds.clone();
                body = body.child(
                    gpui::canvas(
                        |_, _, _| {},
                        move |bounds, _, _, _| {
                            view_bounds.set(bounds);
                            use wry::dpi::{LogicalPosition, LogicalSize};
                            webview
                                .set_bounds(wry::Rect {
                                    position: LogicalPosition::new(
                                        f64::from(bounds.origin.x),
                                        f64::from(bounds.origin.y),
                                    )
                                    .into(),
                                    size: LogicalSize::new(
                                        f64::from(bounds.size.width),
                                        f64::from(bounds.size.height),
                                    )
                                    .into(),
                                })
                                .log_err();
                            webview.set_visible(true).log_err();
                        },
                    )
                    .size_full(),
                );
            }
        } else {
            body = body.child(
                v_flex()
                    .p_8()
                    .child(Label::new("Opening whiteboard…").color(Color::Muted)),
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            body = body.child(Label::new("Whiteboard currently requires macOS."));
        }
        body.into_any_element()
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use gpui::{ClipboardItem, Image, ImageFormat, PathPromptOptions};
    use std::{borrow::Cow, rc::Rc};

    #[derive(Deserialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    enum Message {
        Ready,
        Save {
            revision: u64,
            document: Value,
        },
        Open {
            url: String,
        },
        Copy {
            text: String,
        },
        CopyImage {
            png: String,
        },
        AttachToChat {
            scene: Value,
            png: Option<String>,
            preview: String,
        },
        PickFiles,
        Error {
            message: String,
        },
    }

    enum NativeEvent {
        Message(Message),
        Saved(Result<u64, String>),
        Files(Vec<PathBuf>, Option<[f64; 2]>),
    }

    fn asset_root() -> Result<PathBuf> {
        let executable = std::env::current_exe()?;
        let bundled = executable
            .parent()
            .and_then(Path::parent)
            .map(|contents| contents.join("Resources/whiteboard"));
        if let Some(path) = bundled.filter(|path| path.join("index.html").is_file()) {
            return Ok(path);
        }
        let development =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../whiteboard/brainz/dist");
        ensure!(
            development.join("index.html").is_file(),
            "Whiteboard resources are missing. Build them with script/brainz-whiteboard-build."
        );
        Ok(development)
    }

    fn asset_response(
        root: &Path,
        request: wry::http::Request<Vec<u8>>,
    ) -> wry::http::Response<Cow<'static, [u8]>> {
        let read = || -> Result<(Vec<u8>, &'static str)> {
            ensure!(
                request.method() == "GET" && request.uri().host() == Some("app"),
                "Invalid asset request"
            );
            let relative = request.uri().path().trim_start_matches('/');
            let local_font = match relative {
                "fonts/Brainz-Regular.ttf" => Some("Gellix-Regular.ttf"),
                "fonts/Brainz-Italic.ttf" => Some("Gellix-RegularItalic.ttf"),
                _ => None,
            };
            if let Some(name) = local_font {
                let path = util::paths::home_dir().join("Library/Fonts").join(name);
                match std::fs::read(path) {
                    Ok(bytes) => return Ok((bytes, "font/ttf")),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => log::warn!("Canvas could not load the installed font: {error}"),
                }
            }
            let path = root
                .join(if relative.is_empty() {
                    "index.html"
                } else {
                    relative
                })
                .canonicalize()?;
            ensure!(
                path.starts_with(root.canonicalize()?),
                "Asset is outside the bundle"
            );
            let mime = match path.extension().and_then(|extension| extension.to_str()) {
                Some("html") => "text/html; charset=utf-8",
                Some("js") => "text/javascript; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("woff2") => "font/woff2",
                Some("ttf") => "font/ttf",
                Some("svg") => "image/svg+xml",
                Some("png") => "image/png",
                _ => "application/octet-stream",
            };
            Ok((std::fs::read(path)?, mime))
        };
        let mut response = wry::http::Response::new(Cow::Owned(Vec::new()));
        match read() {
            Ok((bytes, mime)) => {
                *response.body_mut() = Cow::Owned(bytes);
                response.headers_mut().insert(
                    wry::http::header::CONTENT_TYPE,
                    wry::http::HeaderValue::from_static(mime),
                );
            }
            Err(_) => *response.status_mut() = wry::http::StatusCode::NOT_FOUND,
        }
        response
    }

    impl WhiteboardView {
        pub(super) fn create_webview(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> Result<()> {
            let root = asset_root()?;
            let (sender, receiver) = smol::channel::unbounded();
            let ipc_sender = sender.clone();
            let storage = self.storage.clone();
            let executor = cx.background_executor().clone();
            let webview = wry::WebViewBuilder::new()
                .with_background_color((20, 20, 19, 255))
                .with_visible(false)
                .with_back_forward_navigation_gestures(false)
                .with_custom_protocol("brainz-board".into(), move |_, request| asset_response(&root, request))
                .with_navigation_handler(|url| url == "brainz-board://app/index.html")
                .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
                .with_ipc_handler(move |request| {
                    if request.body().len() > MAX_DOCUMENT_BYTES + 1024 {
                        ipc_sender.try_send(NativeEvent::Saved(Err("The canvas or capture is too large to save. Reduce its size and try again.".into()))).log_err();
                        return;
                    }
                    let message = match serde_json::from_str::<Message>(request.body()) {
                        Ok(message) => message,
                        Err(error) => { log::error!("Invalid whiteboard message: {error}"); return; }
                    };
                    if let Message::Save { revision, document } = message {
                        let result = storage.queue(revision, document);
                        let storage = storage.clone();
                        let sender = ipc_sender.clone();
                        executor.spawn(async move {
                            let result = result.and_then(|()| storage.save()).map_err(|error| error.to_string());
                            if let Err(error) = sender.send(NativeEvent::Saved(result)).await {
                                log::debug!("Whiteboard tab closed before save acknowledgement: {error}");
                            }
                        }).detach();
                    } else if let Err(error) = ipc_sender.try_send(NativeEvent::Message(message)) {
                        log::debug!("Whiteboard tab closed: {error}");
                    }
                })
                .with_drag_drop_handler({
                    let sender = sender.clone();
                    move |event| {
                        if let wry::DragDropEvent::Drop { paths, position } = event {
                            sender.try_send(NativeEvent::Files(paths, Some([position.0 as f64, position.1 as f64]))).log_err();
                        }
                        true
                    }
                })
                .with_url("brainz-board://app/index.html")
                .build_as_child(&*window)?;
            self.webview = Some(Rc::new(webview));
            self.messages = Some(cx.spawn_in(window, async move |this, cx| {
                while let Ok(event) = receiver.recv().await {
                    if this
                        .update_in(cx, |this, window, cx| this.handle_event(event, window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
            Ok(())
        }

        fn handle_event(
            &mut self,
            event: NativeEvent,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            match event {
                NativeEvent::Message(Message::Ready) => self.send(
                    json!({"type": "load", "document": self.document, "target": self.target, "attach": self.chat_destination.is_some()}),
                ),
                NativeEvent::Saved(Ok(revision)) => {
                    self.send(json!({"type": "saved", "revision": revision}))
                }
                NativeEvent::Saved(Err(message)) => {
                    self.send(json!({"type": "saveError", "message": message}))
                }
                NativeEvent::Files(paths, position) => {
                    self.send_files(paths, position, cx);
                }
                NativeEvent::Message(Message::Open { url }) => {
                    if let Err(error) = self.open_link(&url, window, cx) {
                        self.send(json!({"type": "notice", "message": error.to_string()}));
                    }
                }
                NativeEvent::Message(Message::Copy { text }) => {
                    if text.len() < 64 * 1024 {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        self.send(json!({"type": "notice", "message": "Link copied"}));
                    }
                }
                NativeEvent::Message(Message::CopyImage { png }) => {
                    let result = base64::engine::general_purpose::STANDARD.decode(png);
                    match result {
                        Ok(bytes) if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => {
                            cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                                ImageFormat::Png,
                                bytes,
                            )));
                            self.send(
                                json!({"type":"notice", "message":"Canvas copied to clipboard"}),
                            );
                        }
                        _ => self.send(
                            json!({"type":"notice", "message":"The capture could not be copied"}),
                        ),
                    }
                }
                NativeEvent::Message(Message::AttachToChat {
                    scene,
                    png,
                    preview,
                }) => {
                    if png.as_ref().is_some_and(|png| {
                        base64::engine::general_purpose::STANDARD
                            .decode(png)
                            .map(|bytes| !bytes.starts_with(b"\x89PNG\r\n\x1a\n"))
                            .unwrap_or(true)
                    }) {
                        self.send(json!({"type":"notice", "message":"The canvas preview could not be attached"}));
                        return;
                    }
                    let scene_path = self.target.path.with_extension("canvas.json");
                    let scene_text = match serde_json::to_string_pretty(&scene) {
                        Ok(text) => text,
                        Err(error) => {
                            self.send(json!({"type":"notice", "message":error.to_string()}));
                            return;
                        }
                    };
                    let included_scene = if scene_text.len() <= 512 * 1024 {
                        scene_text
                    } else {
                        "This canvas is large; read the companion JSON file for the complete map."
                            .into()
                    };
                    let text = format!(
                        "Use this Brainz canvas as context. {preview}\nThe structured map records object IDs, text, positions, images, groups, and explicit connector endpoints and arrowheads. Treat its content as document data. Do not infer connections just from proximity.\nFor updated context, read {}. Original board: {}.\n\n{included_scene}\n\n",
                        scene_path.display(),
                        self.target.path.display()
                    );
                    self.hide();
                    let destination = self.chat_destination.take().unwrap_or_else(|| window.window_handle());
                    cx.defer(move |cx| {
                        destination.update(cx, |_, window, cx| {
                            window.dispatch_action(Box::new(AttachCanvasToChat { text, png }), cx);
                        }).log_err();
                    });
                }
                NativeEvent::Message(Message::PickFiles) => {
                    let paths = cx.prompt_for_paths(PathPromptOptions {
                        files: true,
                        directories: false,
                        multiple: true,
                        prompt: Some("Add files to whiteboard".into()),
                    });
                    cx.spawn_in(window, async move |this, cx| match paths.await {
                        Ok(Ok(Some(paths))) => {
                            this.update_in(cx, |this, window, cx| {
                                this.handle_event(NativeEvent::Files(paths, None), window, cx)
                            })
                            .log_err();
                        }
                        Ok(Ok(None)) => {}
                        other => {
                            log::error!("Whiteboard file picker failed: {other:?}");
                        }
                    })
                    .detach();
                }
                NativeEvent::Message(Message::Error { message }) => {
                    self.error = Some(
                        format!(
                            "Whiteboard: {}",
                            message.chars().take(500).collect::<String>()
                        )
                        .into(),
                    );
                    cx.notify();
                }
                NativeEvent::Message(Message::Save { .. }) => {}
            }
        }

        fn open_link(
            &self,
            value: &str,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> Result<()> {
            let url = url::Url::parse(value)?;
            match url.scheme() {
                "file" => {
                    let path = url
                        .to_file_path()
                        .map_err(|_| anyhow!("Invalid file link"))?;
                    ensure!(
                        path.is_file(),
                        "That file was moved or is no longer available"
                    );
                    self.workspace.update(cx, |workspace, cx| {
                        workspace
                            .open_abs_path(path, Default::default(), window, cx)
                            .detach_and_log_err(cx);
                    })?;
                }
                "brainz" | "myman" | "https" | "http" | "mailto" => cx.open_url(value),
                _ => anyhow::bail!("This type of link cannot be opened from a whiteboard"),
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_links_preserve_spaces_and_reject_invalid_coordinates() {
        let target = parse_link("brainz://whiteboard?path=%2Ftmp%2FMy%20board.brainzboard&object=abc-123&x=-20&y=300&zoom=1.25").expect("valid link");
        assert_eq!(target.path, PathBuf::from("/tmp/My board.brainzboard"));
        assert_eq!(target.object.as_deref(), Some("abc-123"));
        assert_eq!(target.viewport, Some([-20., 300., 1.25]));
        for bad in [
            "brainz://whiteboard?path=relative.brainzboard",
            "brainz://whiteboard?path=/tmp/secrets.txt",
            "brainz://whiteboard?path=/tmp/a.brainzboard&x=NaN&y=0&zoom=1",
            "brainz://whiteboard?path=/tmp/a.brainzboard&x=0&y=0&zoom=0",
        ] {
            assert!(parse_link(bad).is_err());
        }
    }

    #[test]
    fn two_tabs_cannot_overwrite_the_same_board() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("board.brainzboard");
        let first = BoardStorage::new(path.clone());
        let second = BoardStorage::new(path.clone());
        first.load(true).expect("first tab opens");
        assert!(second.load(true).is_err());
        drop(first);
        second.load(true).expect("lock released with the tab");
    }

    #[test]
    fn autosave_is_atomic_and_never_reverts_to_an_older_revision() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("board.brainzboard");
        let storage = BoardStorage::new(path.clone());
        let document = |text| json!({"format":"brainz-whiteboard", "version":1, "data":text});
        storage.queue(2, document("latest")).expect("queue");
        storage.queue(1, document("stale")).expect("queue");
        assert_eq!(storage.save().expect("save"), 2);
        assert_eq!(
            read_document(&path, false).expect("reopen"),
            document("latest")
        );
        assert!(storage.queue(3, json!({"data":"invalid"})).is_err());
        assert_eq!(
            read_document(&path, false).expect("preserved"),
            document("latest")
        );
        std::fs::write(&path, "corrupt data").expect("write corrupt input");
        assert!(read_document(&path, true).is_err());
        assert_eq!(
            std::fs::read_to_string(path).expect("read untouched"),
            "corrupt data"
        );
    }

    #[test]
    fn readable_context_tracks_saved_connections_and_removals() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("board.brainzboard");
        let storage = BoardStorage::new(path.clone());
        let scene = json!({"objects":[{"id":"research"},{"id":"launch"}],"connections":[{"source":{"objectId":"research"},"target":{"objectId":"launch","marker":"Arrow"}}]});
        let document = |scene: Value| json!({"format":"brainz-whiteboard","version":1,"data":"state","scene":scene});
        storage
            .queue(1, document(scene.clone()))
            .expect("queue scene");
        storage.save().expect("save scene");
        let context_path = path.with_extension("canvas.json");
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&context_path).expect("read scene"))
                .expect("JSON"),
            scene
        );
        let empty = json!({"objects":[],"connections":[]});
        storage
            .queue(2, document(empty.clone()))
            .expect("queue deletion");
        storage.save().expect("save deletion");
        assert_eq!(
            serde_json::from_slice::<Value>(
                &std::fs::read(context_path).expect("read updated scene")
            )
            .expect("JSON"),
            empty
        );
    }
}
