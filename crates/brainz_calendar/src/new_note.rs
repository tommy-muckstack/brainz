use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use brainz_templates::{CreatePolicy, InsertMode, Registry, RenderRequest, Rendered, Template};
use editor::{Editor, MultiBufferOffset, scroll::Autoscroll};
use fuzzy::StringMatchCandidate;
use gpui::{
    App, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Subscription, Task,
    WeakEntity, Window, actions,
};
use markdown::{Markdown, MarkdownElement};
use picker::{Picker, PickerDelegate};
use project::Project;
use serde_json::{Map, Value, json};
use ui::{ContextMenu, ListItem, ListItemSpacing, PopoverMenu, prelude::*};
use util::ResultExt;
use workspace::{ModalView, Workspace, notifications::DetachAndPromptErr as _};

actions!(
    brainz_calendar,
    [
        /// Create a note from a local Markdown template.
        NewNote,
        /// Open today's daily note, creating it only if it does not exist.
        OpenDailyNote,
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &NewNote, window, cx| {
            open(workspace, false, window, cx)
        });
        workspace.register_action(|workspace, _: &OpenDailyNote, window, cx| {
            open(workspace, true, window, cx)
        });
    })
    .detach();
}

fn open(workspace: &mut Workspace, daily: bool, window: &mut Window, cx: &mut Context<Workspace>) {
    if !workspace.project().read(cx).is_local() {
        Editor::new_file(workspace, &workspace::NewFile, window, cx);
        return;
    }
    let Some(root) = workspace
        .root_paths(cx)
        .first()
        .map(|root| root.to_path_buf())
    else {
        Editor::new_file(workspace, &workspace::NewFile, window, cx);
        return;
    };
    let project = workspace.project().clone();
    let folder = workspace
        .active_item(cx)
        .and_then(|item| item.project_path(cx))
        .and_then(|path| {
            let project = project.read(cx);
            let worktree = project.worktree_for_id(path.worktree_id, cx)?.read(cx);
            let path = worktree.absolutize(&path.path);
            path.parent()?
                .strip_prefix(&root)
                .ok()
                .map(|path| path.to_string_lossy().into_owned())
        })
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| ".".into());
    let weak_workspace = cx.weak_entity();
    workspace.toggle_modal(window, cx, |window, cx| {
        NotePicker::new(root, folder, project, weak_workspace, daily, window, cx)
    });
}

fn local_context(root: &Path) -> Map<String, Value> {
    static CALENDAR: OnceLock<Mutex<Option<(Instant, Vec<super::CalendarEvent>)>>> =
        OnceLock::new();
    let cached = CALENDAR.get_or_init(|| Mutex::new(None));
    let events = cached.lock().ok().and_then(|cache| {
        cache
            .as_ref()
            .filter(|(time, _)| time.elapsed() < Duration::from_secs(60))
            .map(|(_, events)| events.clone())
    });
    let events = events.unwrap_or_else(|| {
        let events = match super::load_events() {
            Ok((super::LoadState::Ready, events)) => events,
            Ok(_) => Vec::new(),
            Err(error) => {
                log::warn!("Template calendar context unavailable: {error}");
                Vec::new()
            }
        };
        if let Ok(mut cache) = cached.lock() {
            *cache = Some((Instant::now(), events.clone()));
        }
        events
    });
    context_for_events(root, &events)
}

fn context_for_events(root: &Path, events: &[super::CalendarEvent]) -> Map<String, Value> {
    let now = chrono::Local::now();
    let event_json = |event: &super::CalendarEvent| json!({"id":event.id,"title":event.title.as_ref(),"start":event.start.to_rfc3339(),"end":event.end.to_rfc3339(),"calendar":event.calendar.as_ref(),"attendee_names":event.attendee_names,"location":event.location.as_ref().map(|location| location.as_ref())});
    let current = events
        .iter()
        .filter(|event| !event.all_day && event.end > now)
        .min_by_key(|event| event.start);
    let mut context = Map::new();
    context.insert("calendar".into(), json!({
        "events": events.iter().filter(|event| !event.all_day && event.end > now).map(&event_json).collect::<Vec<_>>(),
        "current_or_next": current.map(&event_json),
        "today": events.iter().filter(|event| event.start.date_naive() == now.date_naive()).map(&event_json).collect::<Vec<_>>(),
        "next_week": events.iter().filter(|event| event.start > now && event.start < now + chrono::Duration::days(7)).map(&event_json).collect::<Vec<_>>()
    }));
    let config = super::brain_config::BrainConfig::load(root);
    let index = super::brain_match::BrainIndex::load(root, &config);
    let event_matches = events.iter().filter_map(|event| {
        index.match_event(&event.title, &event.attendee_names, &event.attendee_emails, event.start.date_naive()).map(|found| (event.id.clone(), json!({"folder":found.folder,"folder_name":found.folder_name,"who":found.who,"prep_file":found.prep_file})))
    }).collect::<Map<_, _>>();
    context.insert("event_matches".into(), Value::Object(event_matches));
    if let Some(event) = current {
        context.insert("event".into(), event_json(event));
        if let Some(found) = index.match_event(
            &event.title,
            &event.attendee_names,
            &event.attendee_emails,
            event.start.date_naive(),
        ) {
            context.insert("match".into(), json!({"folder":found.folder,"folder_name":found.folder_name,"who":found.who,"prep_file":found.prep_file}));
        }
        context.insert(
            "people".into(),
            json!(
                index
                    .people
                    .iter()
                    .filter(|person| event
                        .attendee_names
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case(&person.name))
                        || person.emails.iter().any(|email| event
                            .attendee_emails
                            .iter()
                            .any(|attendee| attendee.eq_ignore_ascii_case(email))))
                    .map(|person| json!({"name":person.name,"file":person.file}))
                    .collect::<Vec<_>>()
            ),
        );
    }
    context
}

struct NotePicker {
    root: PathBuf,
    project: Entity<Project>,
    workspace: WeakEntity<Workspace>,
    picker: Entity<Picker<TemplateDelegate>>,
    templates: Vec<Template>,
    selected: Option<usize>,
    request: RenderRequest,
    fields: Vec<(String, String, Entity<Editor>)>,
    preview: Entity<Markdown>,
    rendered: Option<Rendered>,
    error: Option<String>,
    warnings: Vec<String>,
    loading: bool,
    saving: bool,
    daily: bool,
    destination: Option<PathBuf>,
    folder_picker: Option<Entity<super::canvas_folder_picker::CanvasFolderPicker>>,
    subscriptions: Vec<Subscription>,
    field_subscriptions: Vec<Subscription>,
    _load: Task<()>,
}

impl NotePicker {
    fn new(
        root: PathBuf,
        folder: String,
        project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        daily: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let owner = cx.weak_entity();
        let picker = cx.new(|cx| {
            Picker::uniform_list(
                TemplateDelegate {
                    owner,
                    names: Vec::new(),
                    visible: Vec::new(),
                    selected: 0,
                },
                window,
                cx,
            )
            .embedded()
            .max_height(rems(20.))
        });
        let dismissed = cx.subscribe(&picker, |_, _, _: &DismissEvent, cx| cx.emit(DismissEvent));
        let load_root = root.clone();
        let load = cx.spawn_in(window, async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    Registry::load_brain(&load_root, None)
                        .map(|registry| (registry, local_context(&load_root)))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.loading = false;
                match loaded {
                    Ok((registry, context)) => {
                        this.request.context = context;
                        this.warnings = registry
                            .errors
                            .into_iter()
                            .map(|error| format!("{}: {}", error.path.display(), error.message))
                            .collect();
                        this.templates = registry
                            .templates
                            .into_values()
                            .filter(|template| template.metadata.modes.contains(&InsertMode::New))
                            .collect();
                        this.templates
                            .sort_by(|left, right| left.metadata.name.cmp(&right.metadata.name));
                        let mut names = vec![(
                            "Blank note".to_owned(),
                            "Start writing in your current folder".to_owned(),
                        )];
                        names.extend(this.templates.iter().map(|template| {
                            (
                                template.metadata.name.clone(),
                                format!(
                                    "{} {} {}",
                                    template.metadata.description,
                                    template.metadata.id,
                                    template.metadata.tags.join(" ")
                                ),
                            )
                        }));
                        this.picker.update(cx, |picker, cx| {
                            picker.delegate.names = names;
                            picker.refresh(window, cx);
                        });
                        if this.daily {
                            let index = this
                                .templates
                                .iter()
                                .position(|template| template.metadata.id == "daily");
                            if let Some(index) = index {
                                this.choose(index + 1, window, cx);
                                this.create(false, window, cx);
                            } else {
                                this.error = Some("The daily template is unavailable".into());
                            }
                        } else {
                            this.choose(0, window, cx);
                        }
                    }
                    Err(error) => this.error = Some(format!("Could not load templates: {error:#}")),
                }
                cx.notify();
            })
            .log_err();
        });
        Self {
            root,
            project,
            workspace,
            picker,
            templates: Vec::new(),
            selected: None,
            request: RenderRequest {
                folder,
                ..Default::default()
            },
            fields: Vec::new(),
            preview: cx.new(|cx| Markdown::new("".into(), None, None, cx)),
            rendered: None,
            error: None,
            warnings: Vec::new(),
            loading: true,
            saving: false,
            daily,
            destination: None,
            folder_picker: None,
            subscriptions: vec![dismissed],
            field_subscriptions: Vec::new(),
            _load: load,
        }
    }

    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        self.selected = index.checked_sub(1);
        self.fields.clear();
        self.field_subscriptions.clear();
        self.request.input.clear();
        let fields =
            if let Some(template) = self.selected.and_then(|index| self.templates.get(index)) {
                template
                    .metadata
                    .inputs
                    .iter()
                    .filter(|input| input.default.is_none() && input.kind != "calendar.event")
                    .map(|input| {
                        (
                            input.id.clone(),
                            if input.label.is_empty() {
                                input.id.clone()
                            } else {
                                input.label.clone()
                            },
                        )
                    })
                    .collect::<Vec<_>>()
            } else {
                vec![("title".into(), "Note title".into())]
            };
        for (id, label) in fields {
            let field = cx.new(|cx| {
                let mut editor = Editor::single_line(window, cx);
                editor.set_placeholder_text(&label, window, cx);
                editor
            });
            self.field_subscriptions.push(cx.subscribe_in(
                &field,
                window,
                |this, _, event: &editor::EditorEvent, _, cx| {
                    if matches!(event, editor::EditorEvent::BufferEdited) {
                        this.refresh_preview(cx);
                    }
                },
            ));
            self.fields.push((id, label, field));
        }
        self.refresh_preview(cx);
    }

    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        for (id, _, field) in &self.fields {
            let text = field.read(cx).text(cx);
            let kind = self
                .selected
                .and_then(|index| self.templates.get(index))
                .and_then(|template| {
                    template
                        .metadata
                        .inputs
                        .iter()
                        .find(|input| input.id == *id)
                })
                .map(|input| input.kind.as_str());
            let value = if kind == Some("person") && !text.trim().is_empty() {
                json!({"name":text})
            } else {
                Value::String(text.clone())
            };
            self.request.input.insert(id.clone(), value);
            if id == "title" {
                self.request.title = text;
            }
        }
        let result =
            if let Some(template) = self.selected.and_then(|index| self.templates.get(index)) {
                brainz_templates::render(template, &self.request)
            } else {
                brainz_templates::parse(
                    "---\nname: Blank note\npath: '{{folder}}/{{slug title}}.md'\n---\n{{cursor}}",
                    "blank",
                    brainz_templates::TemplateSource::Builtin,
                )
                .and_then(|template| {
                    let mut request = self.request.clone();
                    if request.title.trim().is_empty() {
                        request.title = "Untitled".into();
                    }
                    brainz_templates::render(&template, &request)
                })
            };
        match result {
            Ok(mut rendered) => {
                if let Some(destination) = &self.destination {
                    rendered.path = rendered
                        .path
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .map(|filename| destination.join(filename));
                }
                if let Some(path) = &rendered.path {
                    // A read-only proposal; creation reserves the filename atomically later.
                    if !self.daily {
                        match brainz_templates::available_path(&self.root, path) {
                            Ok(path) => rendered.path = Some(path),
                            Err(error) => self.error = Some(error.to_string()),
                        }
                    }
                }
                self.preview
                    .update(cx, |preview, cx| preview.replace(rendered.body.clone(), cx));
                self.rendered = Some(rendered);
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.rendered = None;
            }
        }
        cx.notify();
    }

    fn create(&mut self, without_opening: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.saving {
            return;
        }
        self.refresh_preview(cx);
        if self.error.is_some() {
            return;
        }
        let Some(rendered) = self.rendered.as_ref() else {
            return;
        };
        if let Some(missing) = rendered
            .report
            .missing
            .iter()
            .find(|missing| missing.starts_with("input."))
        {
            self.error = Some(format!(
                "Fill in {} before creating the note",
                missing.trim_start_matches("input.")
            ));
            if let Some((_, _, field)) = self
                .fields
                .iter()
                .find(|(id, _, _)| missing == &format!("input.{id}"))
            {
                window.focus(&field.focus_handle(cx), cx);
            }
            cx.notify();
            return;
        }
        let Some(rendered) = self.rendered.take() else {
            return;
        };
        self.saving = true;
        let root = self.root.clone();
        let workspace = self.workspace.clone();
        let policy = if self.daily {
            CreatePolicy::OpenExisting
        } else {
            CreatePolicy::Unique
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn({
                    let root = root.clone();
                    async move { brainz_templates::create_note(&root, &rendered, policy) }
                })
                .await;
            let note = match result {
                Ok(note) => note,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.saving = false;
                        this.error = Some(format!("Could not create note: {error:#}"));
                        cx.notify();
                    })
                    .log_err();
                    return Ok::<_, anyhow::Error>(());
                }
            };
            this.update(cx, |_, cx| cx.emit(DismissEvent))?;
            if !without_opening {
                open_note_editor(workspace, root.join(&note.path), note.cursor, cx).await?;
            }
            Ok(())
        })
        .detach_and_prompt_err(
            "The note was saved, but could not be opened",
            window,
            cx,
            |error, _, _| Some(error.to_string()),
        );
        cx.notify();
    }

    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = smol::channel::bounded(1);
        let picker = cx.new(|cx| {
            super::canvas_folder_picker::CanvasFolderPicker::new(
                self.project.clone(),
                self.root.join(&self.request.folder),
                "New note.md".into(),
                sender,
                window,
                cx,
            )
            .with_title("Save note in", IconName::File)
        });
        self.subscriptions.push(cx.subscribe_in(
            &picker,
            window,
            |this, _, _: &DismissEvent, window, cx| {
                this.folder_picker = None;
                this.picker
                    .update(cx, |picker, cx| picker.focus(window, cx));
                cx.notify();
            },
        ));
        window.focus(&picker.focus_handle(cx), cx);
        self.folder_picker = Some(picker);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(folder) = receiver.recv().await {
                this.update(cx, |this, cx| match folder.strip_prefix(&this.root) {
                    Ok(folder) => {
                        this.destination = Some(folder.to_path_buf());
                        this.request.folder = if folder.as_os_str().is_empty() {
                            ".".into()
                        } else {
                            folder.to_string_lossy().into_owned()
                        };
                        this.refresh_preview(cx);
                    }
                    Err(_) => {
                        this.error = Some("Choose a folder inside this brain".into());
                        cx.notify();
                    }
                })
                .log_err();
            }
        })
        .detach();
        cx.notify();
    }
    fn event_picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let template = self.selected.and_then(|index| self.templates.get(index))?;
        if !template
            .metadata
            .inputs
            .iter()
            .any(|input| input.kind == "calendar.event")
        {
            return None;
        }
        let events = self
            .request
            .context
            .get("calendar")
            .and_then(|calendar| calendar.get("events"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let current = self
            .request
            .input
            .get("event")
            .or_else(|| self.request.context.get("event"));
        let label = current
            .and_then(|event| event.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("Choose an event…")
            .to_owned();
        if events.is_empty() {
            return Some(Label::new("No upcoming calendar events. Choose another template or add an event in Calendar.").color(Color::Muted).size(LabelSize::Small).into_any_element());
        }
        let owner = cx.weak_entity();
        Some(
            PopoverMenu::new("template-event-picker")
                .trigger(
                    Button::new("template-event", label)
                        .start_icon(Icon::new(IconName::BrainzCalendar))
                        .end_icon(Icon::new(IconName::ChevronDown)),
                )
                .menu(move |window, cx| {
                    let owner = owner.clone();
                    let events = events.clone();
                    Some(ContextMenu::build(window, cx, move |mut menu, _, _| {
                        for event in events {
                            let title = event
                                .get("title")
                                .and_then(Value::as_str)
                                .unwrap_or("Untitled event");
                            let start = event
                                .get("start")
                                .and_then(Value::as_str)
                                .and_then(|start| chrono::DateTime::parse_from_rfc3339(start).ok())
                                .map(|start| start.format("%a %-I:%M %p").to_string())
                                .unwrap_or_default();
                            let owner = owner.clone();
                            menu = menu.entry(format!("{title} · {start}"), None, move |_, cx| {
                                owner
                                    .update(cx, |owner, cx| {
                                        let matched = event
                                            .get("id")
                                            .and_then(Value::as_str)
                                            .and_then(|id| {
                                                owner
                                                    .request
                                                    .context
                                                    .get("event_matches")
                                                    .and_then(|matches| matches.get(id))
                                            })
                                            .cloned();
                                        owner.request.context.remove("match");
                                        owner.request.context.remove("people");
                                        if let Some(matched) = matched {
                                            owner.request.context.insert("match".into(), matched);
                                        }
                                        owner.request.context.insert("event".into(), event.clone());
                                        owner.request.input.insert("event".into(), event.clone());
                                        owner.refresh_preview(cx);
                                    })
                                    .log_err();
                            });
                        }
                        menu
                    }))
                })
                .into_any_element(),
        )
    }
}

impl ModalView for NotePicker {}
impl EventEmitter<DismissEvent> for NotePicker {}
impl Focusable for NotePicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.folder_picker
            .as_ref()
            .map(|picker| picker.focus_handle(cx))
            .unwrap_or_else(|| self.picker.focus_handle(cx))
    }
}
impl Render for NotePicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(picker) = &self.folder_picker {
            return picker.clone().into_any_element();
        }
        let event_picker = self.event_picker(cx);
        v_flex()
            .key_context("BrainzNewNote")
            .on_action(
                cx.listener(|this, _: &menu::Confirm, window, cx| this.create(false, window, cx)),
            )
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .w(px(820.))
            .max_w(window.viewport_size().width - px(40.))
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().elevated_surface_background)
            .child(
                h_flex()
                    .p_3()
                    .gap_2()
                    .child(Icon::new(IconName::File))
                    .child(
                        Label::new(if self.daily { "Daily note" } else { "New note" })
                            .weight(gpui::FontWeight::SEMIBOLD),
                    ),
            )
            .when(self.loading, |this| {
                this.child(Label::new("Loading templates…").color(Color::Muted))
            })
            .child(
                h_flex()
                    .items_start()
                    .child(div().w(px(340.)).flex_shrink_0().child(self.picker.clone()))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .p_3()
                            .gap_3()
                            .children(event_picker)
                            .children(self.fields.iter().enumerate().map(
                                |(index, (_, label, field))| {
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            Label::new(label.clone())
                                                .size(LabelSize::Small)
                                                .color(Color::Muted),
                                        )
                                        .child(
                                            div()
                                                .id(("template-field", index))
                                                .p_2()
                                                .rounded_md()
                                                .border_1()
                                                .border_color(cx.theme().colors().border)
                                                .child(field.clone()),
                                        )
                                },
                            ))
                            .child(
                                div()
                                    .id("template-preview")
                                    .max_h(rems(24.))
                                    .overflow_y_scroll()
                                    .child(MarkdownElement::new(
                                        self.preview.clone(),
                                        super::brief::BriefView::narrative_style(window, cx),
                                    )),
                            ),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .child(Label::new(error).color(Color::Error).size(LabelSize::Small)),
                )
            })
            .when(!self.warnings.is_empty(), |this| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .children(self.warnings.iter().map(|warning| {
                            Label::new(warning.clone())
                                .color(Color::Warning)
                                .size(LabelSize::Small)
                        })),
                )
            })
            .child(
                h_flex()
                    .p_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        Button::new("note-folder", self.request.folder.clone())
                            .start_icon(Icon::new(IconName::Folder))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.choose_folder(window, cx)),
                            ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Label::new(
                                self.rendered
                                    .as_ref()
                                    .and_then(|rendered| rendered.path.as_ref())
                                    .map(|path| path.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                            )
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate(),
                        ),
                    )
                    .child(
                        Button::new(
                            "create-note",
                            if self.saving {
                                "Creating…"
                            } else {
                                "Create note"
                            },
                        )
                        .style(ButtonStyle::Filled)
                        .disabled(self.loading || self.saving || self.error.is_some())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.create(false, window, cx)),
                        ),
                    ),
            )
            .into_any_element()
    }
}

async fn open_note_editor(
    workspace: WeakEntity<Workspace>,
    path: PathBuf,
    cursor: Option<usize>,
    cx: &mut gpui::AsyncWindowContext,
) -> anyhow::Result<Entity<Editor>> {
    let project = workspace.read_with(cx, |workspace, _| workspace.project().clone())?;
    let buffer = project
        .update(cx, |project, cx| project.open_local_buffer(path, cx))
        .await?;
    let editor = workspace.update_in(cx, |workspace, window, cx| {
        let existing = workspace.items_of_type::<Editor>(cx).find(|editor| {
            editor.read(cx).buffer().read(cx).as_singleton().as_ref() == Some(&buffer)
        });
        let editor = existing.unwrap_or_else(|| cx.new(|cx| Editor::for_buffer(buffer, Some(project.clone()), window, cx)));
        if !workspace.activate_item(&editor, true, true, window, cx) {
            markdown_preview::markdown_preview_view::MarkdownPreviewView::suppress_auto_preview_once(&editor, cx);
            workspace.add_item_to_active_pane(Box::new(editor.clone()), None, true, window, cx);
        }
        editor
    })?;
    editor.update_in(cx, |editor, window, cx| {
        if let Some(cursor) = cursor {
            let cursor = MultiBufferOffset(cursor);
            editor.change_selections(Some(Autoscroll::fit()).into(), window, cx, |selections| {
                selections.select_ranges([cursor..cursor]);
            });
        }
        window.focus(&editor.focus_handle(cx), cx);
    })?;
    Ok(editor)
}

struct TemplateDelegate {
    owner: WeakEntity<NotePicker>,
    names: Vec<(String, String)>,
    visible: Vec<usize>,
    selected: usize,
}
impl TemplateDelegate {
    fn notify_selection(&self, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(index) = self.visible.get(self.selected).copied() else {
            return;
        };
        let owner = self.owner.clone();
        cx.defer_in(window, move |_, window, cx| {
            owner
                .update(cx, |owner, cx| owner.choose(index, window, cx))
                .log_err();
        });
    }
}
impl PickerDelegate for TemplateDelegate {
    type ListItem = ListItem;
    fn name() -> &'static str {
        "brainz note templates"
    }
    fn match_count(&self) -> usize {
        self.visible.len()
    }
    fn selected_index(&self) -> usize {
        self.selected
    }
    fn select_on_hover(&self) -> bool {
        false
    }
    fn set_selected_index(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        self.selected = index;
        self.notify_selection(window, cx);
    }
    fn placeholder_text(&self, _: &mut Window, _: &mut App) -> Arc<str> {
        "Find a template…".into()
    }
    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let candidates = self
            .names
            .iter()
            .enumerate()
            .map(|(index, (name, description))| {
                StringMatchCandidate::new(index, &format!("{name} {description}"))
            })
            .collect::<Vec<_>>();
        let count = candidates.len();
        cx.spawn_in(window, async move |picker, cx| {
            let visible = if query.is_empty() {
                (0..count).collect()
            } else {
                fuzzy::match_strings(
                    &candidates,
                    &query,
                    false,
                    true,
                    100,
                    &Default::default(),
                    cx.background_executor().clone(),
                )
                .await
                .into_iter()
                .map(|matched| matched.candidate_id)
                .collect()
            };
            picker
                .update_in(cx, |picker, window, cx| {
                    picker.delegate.visible = visible;
                    picker.delegate.selected = 0;
                    picker.delegate.notify_selection(window, cx);
                    cx.notify();
                })
                .log_err();
        })
    }
    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let owner = self.owner.clone();
        cx.defer_in(window, move |_, window, cx| {
            owner
                .update(cx, |owner, cx| owner.create(secondary, window, cx))
                .log_err();
        });
    }
    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }
    fn render_match(
        &self,
        index: usize,
        selected: bool,
        _: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Option<ListItem> {
        let (name, description) = self.names.get(*self.visible.get(index)?)?;
        Some(
            ListItem::new(index)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(IconName::File).color(Color::Muted))
                        .child(
                            v_flex().min_w_0().child(Label::new(name.clone())).child(
                                Label::new(description.clone())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted)
                                    .truncate(),
                            ),
                        ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::UpdateGlobal as _;

    #[gpui::test]
    async fn created_note_opens_at_the_caret_in_edit_mode_and_reuses_the_buffer(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(db::AppDatabase::test_new());
            workspace::AppState::test(cx);
            editor::init(cx);
            markdown_preview::init(cx);
            settings::SettingsStore::update_global(cx, |store, cx| {
                store.update_user_settings(cx, |settings| {
                    settings
                        .markdown_preview
                        .get_or_insert_default()
                        .open_markdown_files_in_preview = Some(true);
                })
            });
        });
        let fs = project::FakeFs::new(cx.executor());
        fs.insert_tree("/brain", json!({"note.md":"# Note\n\nFirst bullet\n"}))
            .await;
        let project = Project::test(fs, [Path::new("/brain")], cx).await;
        project
            .read_with(cx, |project, _| project.languages().clone())
            .add(Arc::new(language::Language::new(
                language::LanguageConfig {
                    name: "Markdown".into(),
                    matcher: Arc::new(language::LanguageMatcher {
                        path_suffixes: vec!["md".into()],
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                None,
            )));
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        let mut first = None;
        for _ in 0..2 {
            let editor = workspace
                .update_in(cx, |_, window, cx| {
                    cx.spawn_in(window, async move |workspace, cx| {
                        open_note_editor(workspace, "/brain/note.md".into(), Some(8), cx).await
                    })
                })
                .await
                .expect("open note");
            cx.run_until_parked();
            workspace.read_with(cx, |workspace, cx| {
                assert_eq!(workspace.items_of_type::<Editor>(cx).count(), 1);
                assert_eq!(workspace.active_item_as::<Editor>(cx), Some(editor.clone()));
                assert_eq!(workspace.items_of_type::<markdown_preview::markdown_preview_view::MarkdownPreviewView>(cx).count(), 0);
            });
            editor.update_in(cx, |editor, window, cx| {
                assert!(editor.focus_handle(cx).contains_focused(window, cx));
                let snapshot = editor.snapshot(window, cx);
                assert_eq!(
                    editor
                        .selections
                        .newest::<MultiBufferOffset>(&snapshot)
                        .head(),
                    MultiBufferOffset(8)
                );
            });
            if let Some(first) = &first {
                assert_eq!(first, &editor);
            }
            first = Some(editor);
        }
    }

    #[gpui::test]
    async fn template_search_and_inputs_keep_focus_and_update_the_preview(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let settings = settings::SettingsStore::test(cx);
            cx.set_global(settings);
            cx.set_global(db::AppDatabase::test_new());
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
        });
        let root = tempfile::tempdir().expect("temporary brain");
        let fs = project::FakeFs::new(cx.executor());
        fs.insert_tree(root.path(), json!({"projects": {}})).await;
        let project = Project::test(fs, [root.path()], cx).await;
        let (workspace, _) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));
        let weak_workspace = workspace.downgrade();
        let (modal, cx) = cx.add_window_view(|window, cx| {
            NotePicker::new(
                root.path().to_path_buf(),
                "projects".into(),
                project,
                weak_workspace,
                false,
                window,
                cx,
            )
        });
        let picker =
            modal.update_in(cx, |modal, window, cx| {
                modal._load = Task::ready(());
                modal.loading = false;
                modal.templates = Registry::builtin()
                    .expect("templates")
                    .templates
                    .into_values()
                    .collect();
                let mut names = vec![("Blank note".into(), "".into())];
                names.extend(modal.templates.iter().map(|template| {
                    (template.metadata.name.clone(), template.metadata.id.clone())
                }));
                modal.picker.update(cx, |picker, cx| {
                    picker.delegate.names = names;
                    picker.refresh(window, cx);
                    picker.focus(window, cx);
                });
                modal.picker.clone()
            });
        cx.run_until_parked();
        cx.simulate_input("decis");
        cx.run_until_parked();
        let field = modal.read_with(cx, |modal, _| {
            assert_eq!(
                modal.templates[modal.selected.expect("selected")]
                    .metadata
                    .id,
                "decision"
            );
            modal
                .fields
                .iter()
                .find(|(id, _, _)| id == "title")
                .expect("title input")
                .2
                .clone()
        });
        field.update_in(cx, |editor, window, cx| {
            window.focus(&editor.focus_handle(cx), cx)
        });
        cx.simulate_input("Choose an offline format");
        cx.run_until_parked();
        modal.read_with(cx, |modal, _| {
            let rendered = modal.rendered.as_ref().expect("preview");
            assert!(rendered.body.contains("Choose an offline format"));
            assert!(
                !rendered
                    .report
                    .missing
                    .iter()
                    .any(|missing| missing == "input.title")
            );
        });
        cx.update(|window, cx| assert!(field.focus_handle(cx).contains_focused(window, cx)));
        assert_eq!(
            std::fs::read_dir(root.path())
                .expect("no writes during preview")
                .count(),
            0
        );
        picker.update_in(cx, |picker, window, cx| picker.focus(window, cx));
        cx.dispatch_action(menu::Cancel);
        cx.run_until_parked();
    }
}
