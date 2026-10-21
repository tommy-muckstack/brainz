//! Brainz: Markdown notes open in a rich text editor, BlockSuite's doc mode
//! running in a web view. The page receives the note as top-level segments
//! and sends back the whole file, keeping the bytes of every block the person
//! did not touch. Saves go through the project buffer, so undo, the file
//! watcher and agents editing the same note all see one copy of the text.

use gpui::{App, actions};

actions!(
    brainz_note,
    [
        /// Open the active Markdown file in the rich text editor.
        OpenRichNote,
        /// Show the Markdown source of the open note.
        ShowMarkdownSource,
    ]
);

pub fn init(cx: &mut App) {
    #[cfg(target_os = "macos")]
    native::init(cx);
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
}

/// Whether `href` leaves Brainz: web pages and mail open in the browser.
fn is_external_link(href: &str) -> bool {
    let lower = href.trim().to_ascii_lowercase();
    ["http://", "https://", "mailto:", "tel:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

/// A relative link such as `../people/sam.md#notes` resolved against the
/// note's folder, without its fragment or query. None for anchors alone.
fn resolve_note_link(note: &std::path::Path, href: &str) -> Option<std::path::PathBuf> {
    let path = href.split(['#', '?']).next()?.trim();
    if path.is_empty() {
        return None;
    }
    let path = percent_decode(path);
    let candidate = std::path::Path::new(&path);
    Some(if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        note.parent()?.join(candidate)
    })
}

fn percent_decode(text: &str) -> String {
    url::form_urlencoded::parse(format!("p={}", text.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, value)| value.into_owned())
        .unwrap_or_else(|| text.to_owned())
}

#[cfg(target_os = "macos")]
mod native {
    use std::{borrow::Cow, cell::Cell, path::PathBuf, rc::Rc};

    use anyhow::{Context as _, Result};
    use editor::Editor;
    use gpui::{
        App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Subscription,
        Task, WeakEntity, Window,
    };
    use language::{Buffer, BufferEvent};
    use markdown_preview::markdown_preview_view::{MarkdownPreviewView, RichNoteOpener};
    use project::Project;
    use serde::Deserialize;
    use serde_json::{Value, json};
    use ui::prelude::*;
    use util::ResultExt as _;
    use workspace::{
        Item, ItemHandle as _, OpenOptions, Pane, Workspace,
        item::{ItemBufferKind, ItemEvent},
    };

    use super::{OpenRichNote, ShowMarkdownSource};

    pub(super) fn init(cx: &mut App) {
        cx.set_global(RichNoteOpener(Rc::new(
            |workspace, editor, _pane, focus, window, cx| {
                let view = RichNoteView::for_editor(workspace, &editor, focus, window, cx)?;
                Some(Box::new(view))
            },
        )));
        cx.observe_new(|workspace: &mut Workspace, _, _| {
            workspace.register_action(|workspace, _: &OpenRichNote, window, cx| {
                let Some(editor) = workspace
                    .active_item(cx)
                    .and_then(|item| item.downcast::<Editor>())
                else {
                    return;
                };
                let Some(pane) = workspace.pane_for(&editor) else {
                    return;
                };
                let Some(view) = RichNoteView::for_editor(workspace, &editor, true, window, cx)
                else {
                    return;
                };
                pane.update(cx, |pane, cx| {
                    let index = pane.index_for_item(&editor);
                    pane.add_item(Box::new(view), true, true, index, window, cx);
                    pane.remove_item(editor.item_id(), false, false, window, cx);
                });
            });
        })
        .detach();
    }

    #[derive(Deserialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    enum Message {
        Ready,
        Save { revision: u64, markdown: String },
        Open { href: String },
        Error { message: String },
        Source,
    }

    pub struct RichNoteView {
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        buffer: Entity<Buffer>,
        path: PathBuf,
        focus_handle: FocusHandle,
        webview: Option<Rc<wry::WebView>>,
        bounds: Rc<Cell<Bounds<Pixels>>>,
        ready: bool,
        /// The text the page currently holds, so buffer edits that only echo
        /// the page's own save do not reload it.
        page_text: Option<String>,
        revision: u64,
        error: Option<SharedString>,
        save: Option<Task<()>>,
        messages: Option<Task<()>>,
        _subscriptions: Vec<Subscription>,
    }

    impl RichNoteView {
        fn for_editor(
            workspace: &mut Workspace,
            editor: &Entity<Editor>,
            focus: bool,
            window: &mut Window,
            cx: &mut Context<Workspace>,
        ) -> Option<Entity<Self>> {
            let project = workspace.project().clone();
            if !project.read(cx).is_local() {
                return None;
            }
            let buffer = editor.read(cx).buffer().read(cx).as_singleton()?;
            let path = buffer
                .read(cx)
                .file()
                .and_then(|file| file.as_local())
                .map(|file| file.abs_path(cx))?;
            let workspace_handle = cx.entity().downgrade();
            let view = cx.new(|cx| Self::new(workspace_handle, project, buffer, path, window, cx));
            if focus {
                let focus_handle = view.read(cx).focus_handle.clone();
                focus_handle.focus(window, cx);
            }
            Some(view)
        }

        fn new(
            workspace: WeakEntity<Workspace>,
            project: Entity<Project>,
            buffer: Entity<Buffer>,
            path: PathBuf,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> Self {
            let focus_handle = cx.focus_handle();
            let subscriptions = vec![
                cx.subscribe(&buffer, |this, _, event: &BufferEvent, cx| match event {
                    BufferEvent::Edited { .. } | BufferEvent::Reloaded => this.load_from_buffer(cx),
                    BufferEvent::FileHandleChanged => cx.emit(ItemEvent::UpdateTab),
                    _ => {}
                }),
                cx.on_focus(&focus_handle, window, |this, _, _| {
                    if let Some(webview) = &this.webview {
                        webview.focus().log_err();
                    }
                    this.send(json!({"type": "focus"}));
                }),
            ];
            Self {
                workspace,
                project,
                buffer,
                path,
                focus_handle,
                webview: None,
                bounds: Rc::default(),
                ready: false,
                page_text: None,
                revision: 0,
                error: None,
                save: None,
                messages: None,
                _subscriptions: subscriptions,
            }
        }

        fn send(&self, message: Value) {
            if let Some(webview) = &self.webview {
                webview
                    .evaluate_script(&format!(
                        "window.brainzNote && window.brainzNote.receive({message});"
                    ))
                    .log_err();
            }
        }

        fn hide(&self) {
            if let Some(webview) = &self.webview {
                // Hiding WKWebView does not hand AppKit's first responder back
                // to GPUI; without this keys keep going to the hidden page.
                webview.focus_parent().log_err();
                webview.set_visible(false).log_err();
            }
        }

        fn load_from_buffer(&mut self, cx: &mut Context<Self>) {
            if !self.ready {
                return;
            }
            let text = self.buffer.read(cx).text();
            if self.page_text.as_deref() == Some(text.as_str()) {
                return;
            }
            let segments = brainz_editor::BlockMap::parse(text.as_str())
                .note_segments()
                .into_iter()
                .enumerate()
                .map(|(id, segment)| json!({"id": id, "kind": segment.kind, "text": segment.text}))
                .collect::<Vec<_>>();
            let read_only = self.buffer.read(cx).read_only();
            self.send(json!({
                "type": "load",
                "path": self.path,
                "readOnly": read_only,
                "segments": segments,
            }));
            self.page_text = Some(text);
        }

        fn apply_page_text(&mut self, revision: u64, markdown: String, cx: &mut Context<Self>) {
            if revision <= self.revision {
                return;
            }
            self.revision = revision;
            self.page_text = Some(markdown.clone());
            if self.buffer.read(cx).text() == markdown {
                self.send(json!({"type": "saved", "revision": revision}));
                return;
            }
            let diff = self.buffer.read(cx).diff(markdown, cx);
            let buffer = self.buffer.clone();
            let project = self.project.clone();
            self.save = Some(cx.spawn(async move |this, cx| {
                let diff = diff.await;
                buffer.update(cx, |buffer, cx| {
                    buffer.finalize_last_transaction();
                    buffer.apply_diff(diff, cx);
                    buffer.finalize_last_transaction();
                });
                let result = project
                    .update(cx, |project, cx| project.save_buffer(buffer, cx))
                    .await;
                this.update(cx, |this, cx| match result {
                    Ok(()) => this.send(json!({"type": "saved", "revision": revision})),
                    Err(error) => {
                        this.error = Some(format!("Could not save this note: {error}").into());
                        cx.notify();
                    }
                })
                .log_err();
            }));
        }

        fn open_link(&mut self, href: String, window: &mut Window, cx: &mut Context<Self>) {
            if super::is_external_link(&href) {
                cx.open_url(href.trim());
                return;
            }
            let Some(path) = super::resolve_note_link(&self.path, &href) else {
                return;
            };
            let Some(workspace) = self.workspace.upgrade() else {
                return;
            };
            workspace.update(cx, |workspace, cx| {
                workspace
                    .open_abs_path(path, OpenOptions::default(), window, cx)
                    .detach_and_log_err(cx);
            });
        }

        /// Swaps this tab for a plain editor on the same buffer, so the raw
        /// Markdown is one keystroke away and its undo history is shared.
        fn show_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            let Some(workspace) = self.workspace.upgrade() else {
                return;
            };
            let this = cx.entity();
            let buffer = self.buffer.clone();
            let project = self.project.clone();
            self.hide();
            window.defer(cx, move |window, cx| {
                workspace.update(cx, |workspace, cx| {
                    let Some(pane) = workspace.pane_for(&this) else {
                        return;
                    };
                    let editor = cx.new(|cx| Editor::for_buffer(buffer, Some(project), window, cx));
                    MarkdownPreviewView::suppress_auto_preview_once(&editor, cx);
                    pane.update(cx, |pane: &mut Pane, cx| {
                        let index = pane.index_for_item(&this);
                        pane.add_item(Box::new(editor), true, true, index, window, cx);
                        pane.remove_item(this.item_id(), false, false, window, cx);
                    });
                });
            });
        }

        fn create_webview(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<()> {
            let root = crate::whiteboard::native::asset_root()?;
            anyhow::ensure!(
                root.join("note.html").is_file(),
                "The note editor is missing from this build. Rebuild it with script/brainz-whiteboard-build."
            );
            let (sender, receiver) = smol::channel::unbounded::<Message>();
            let webview = wry::WebViewBuilder::new()
                .with_background_color((20, 20, 19, 255))
                .with_visible(false)
                .with_back_forward_navigation_gestures(false)
                .with_custom_protocol("brainz-note".into(), move |_, request| {
                    asset_response(&root, request)
                })
                .with_navigation_handler(|url| url == "brainz-note://app/note.html")
                .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
                .with_ipc_handler(move |request| {
                    match serde_json::from_str::<Message>(request.body()) {
                        Ok(message) => {
                            if let Err(error) = sender.try_send(message) {
                                log::debug!("Note tab closed: {error}");
                            }
                        }
                        Err(error) => log::error!("Invalid note editor message: {error}"),
                    }
                })
                .with_url("brainz-note://app/note.html")
                .build_as_child(&*window)?;
            self.webview = Some(Rc::new(webview));
            self.messages = Some(cx.spawn_in(window, async move |this, cx| {
                while let Ok(message) = receiver.recv().await {
                    if this
                        .update_in(cx, |this, window, cx| this.handle(message, window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
            Ok(())
        }

        fn handle(&mut self, message: Message, window: &mut Window, cx: &mut Context<Self>) {
            match message {
                Message::Ready => {
                    self.ready = true;
                    self.page_text = None;
                    self.load_from_buffer(cx);
                    if self.focus_handle.is_focused(window) {
                        if let Some(webview) = &self.webview {
                            webview.focus().log_err();
                        }
                        self.send(json!({"type": "focus"}));
                    }
                }
                Message::Save { revision, markdown } => {
                    self.apply_page_text(revision, markdown, cx)
                }
                Message::Open { href } => self.open_link(href, window, cx),
                Message::Error { message } => {
                    log::error!(
                        "Note editor: {}",
                        message.chars().take(500).collect::<String>()
                    )
                }
                Message::Source => self.show_source(window, cx),
            }
        }
    }

    fn asset_response(
        root: &std::path::Path,
        request: wry::http::Request<Vec<u8>>,
    ) -> wry::http::Response<Cow<'static, [u8]>> {
        let read = || -> Result<(Vec<u8>, String)> {
            anyhow::ensure!(
                request.method() == "GET" && request.uri().host() == Some("app"),
                "Invalid asset request"
            );
            let relative = request.uri().path().trim_start_matches('/');
            crate::whiteboard::native::bundle_asset(
                root,
                if relative.is_empty() {
                    "note.html"
                } else {
                    relative
                },
            )
            .context("note editor asset")
        };
        let mut response = wry::http::Response::new(Cow::Owned(Vec::new()));
        match read() {
            Ok((bytes, mime)) => {
                *response.body_mut() = Cow::Owned(bytes);
                if let Ok(value) = wry::http::HeaderValue::from_str(&mime) {
                    response
                        .headers_mut()
                        .insert(wry::http::header::CONTENT_TYPE, value);
                }
            }
            Err(_) => *response.status_mut() = wry::http::StatusCode::NOT_FOUND,
        }
        response
    }

    impl EventEmitter<ItemEvent> for RichNoteView {}

    impl Focusable for RichNoteView {
        fn focus_handle(&self, _: &App) -> FocusHandle {
            self.focus_handle.clone()
        }
    }

    impl Item for RichNoteView {
        type Event = ItemEvent;

        fn to_item_events(event: &ItemEvent, emit: &mut dyn FnMut(ItemEvent)) {
            emit(*event);
        }

        fn tab_content_text(&self, _: usize, _: &App) -> SharedString {
            self.path
                .file_stem()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Note".into())
                .into()
        }

        fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
            Some(self.path.to_string_lossy().into_owned().into())
        }

        fn tab_icon(&self, _: &Window, _: &App) -> Option<Icon> {
            Some(Icon::new(IconName::FileTextOutlined))
        }

        fn for_each_project_item(
            &self,
            cx: &App,
            visit: &mut dyn FnMut(gpui::EntityId, &dyn project::ProjectItem),
        ) {
            visit(self.buffer.entity_id(), self.buffer.read(cx));
        }

        fn buffer_kind(&self, _: &App) -> ItemBufferKind {
            ItemBufferKind::Singleton
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
    }

    impl Render for RichNoteView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if self.webview.is_none() && self.error.is_none() {
                if let Err(error) = self.create_webview(window, cx) {
                    self.error = Some(format!("The note editor could not start: {error}").into());
                }
            }
            let mut body = div()
                .id("brainz-rich-note")
                .key_context("BrainzRichNote")
                .on_action(cx.listener(|this, _: &ShowMarkdownSource, window, cx| {
                    this.show_source(window, cx)
                }))
                .size_full()
                .overflow_hidden()
                .track_focus(&self.focus_handle)
                .bg(cx.theme().colors().editor_background);
            if let Some(error) = &self.error {
                self.hide();
                return body.child(
                    v_flex()
                        .p_8()
                        .gap_3()
                        .child(Label::new(error.clone()))
                        .child(
                            Button::new("brainz-note-source", "Open the Markdown instead")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.show_source(window, cx)),
                                ),
                        ),
                );
            }
            let Some(webview) = self.webview.clone() else {
                return body;
            };
            let modal_open = self
                .workspace
                .upgrade()
                .is_some_and(|workspace| workspace.read(cx).has_active_modal(window, cx));
            // The page is a native view above GPUI, so popovers and modals
            // would sit behind it; step aside while one is open.
            if modal_open || ui::has_open_popover(window, cx) || cx.has_active_drag() {
                self.hide();
                return body;
            }
            let view_bounds = self.bounds.clone();
            body = body.child(
                gpui::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
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
                        window.on_mouse_event(move |event: &gpui::MouseDownEvent, phase, _, _| {
                            if phase.capture() && !bounds.contains(&event.position) {
                                webview.focus_parent().log_err();
                            }
                        });
                    },
                )
                .size_full(),
            );
            body
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn links_resolve_against_the_note_folder() {
        let note = Path::new("/brain/people/sam.md");
        assert_eq!(
            resolve_note_link(note, "../projects/q4.md#goals"),
            Some(Path::new("/brain/people/../projects/q4.md").to_path_buf())
        );
        assert_eq!(
            resolve_note_link(note, "Coffee%20notes.md"),
            Some(Path::new("/brain/people/Coffee notes.md").to_path_buf())
        );
        assert_eq!(resolve_note_link(note, "#top"), None);
        assert_eq!(
            resolve_note_link(note, "/tmp/x.md"),
            Some(Path::new("/tmp/x.md").to_path_buf())
        );
    }

    #[test]
    fn web_and_mail_links_leave_brainz() {
        assert!(is_external_link("https://example.com"));
        assert!(is_external_link(" MAILTO:sam@example.com"));
        assert!(!is_external_link("notes/plan.md"));
    }
}
