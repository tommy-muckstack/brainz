//! Brainz: a PDF tab. Zed treats a `.pdf` as an unsupported binary; Brainz
//! renders its pages to PNGs with the bundled `brainz-pdf` helper
//! (CoreGraphics) and shows them as a scrollable stack, with a button to
//! hand off to the system viewer for anything more.

use std::{
    hash::{Hash as _, Hasher as _},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, Result, anyhow};
use gpui::{
    App, Entity, EventEmitter, FocusHandle, Focusable, ScrollHandle, Task, WeakEntity, Window,
};
use project::{Project, ProjectEntryId, ProjectPath};
use serde::Deserialize;
use ui::{Tooltip, prelude::*};
use workspace::{
    Item, Pane, WorkspaceId,
    item::{ItemEvent, ProjectItem, TabContentParams},
};

const PAGE_RENDER_WIDTH: u32 = 1400;
const MAX_PAGES: u32 = 200;
const DISPLAY_WIDTH: f32 = 820.;

/// The model side: which file, resolved to an absolute path.
pub struct PdfItem {
    project_path: ProjectPath,
    entry_id: Option<ProjectEntryId>,
    abs_path: PathBuf,
}

impl project::ProjectItem for PdfItem {
    fn try_open(
        project: &Entity<Project>,
        path: &ProjectPath,
        cx: &mut App,
    ) -> Option<Task<Result<Entity<Self>>>> {
        let is_pdf = path
            .path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"));
        if !is_pdf || !project.read(cx).is_local() {
            return None;
        }
        let abs_path = project.read(cx).absolute_path(path, cx)?;
        let entry_id = project
            .read(cx)
            .entry_for_path(path, cx)
            .map(|entry| entry.id);
        let project_path = path.clone();
        let item = cx.new(|_| PdfItem {
            project_path,
            entry_id,
            abs_path,
        });
        Some(Task::ready(Ok(item)))
    }

    fn entry_id(&self, _: &App) -> Option<ProjectEntryId> {
        self.entry_id
    }

    fn project_path(&self, _: &App) -> Option<ProjectPath> {
        Some(self.project_path.clone())
    }

    fn is_dirty(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Deserialize)]
struct HelperOutput {
    status: String,
    #[serde(default)]
    pages: Vec<HelperPage>,
    #[serde(default)]
    total: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct HelperPage {
    path: PathBuf,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone)]
struct Page {
    path: Arc<Path>,
    width: u32,
    height: u32,
}

fn helper_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating Brainz executable")?;
    let dir = exe.parent().context("Brainz executable has no parent")?;
    let helper = dir.join("brainz-pdf");
    if helper.is_file() {
        Ok(helper)
    } else {
        Err(anyhow!(
            "PDF helper missing at {}; run script/brainz-local",
            helper.display()
        ))
    }
}

/// Rendered pages live under Brainz's data directory, keyed by the file's
/// path, size, and modification time, so reopening is instant and a
/// changed file re-renders.
fn cache_dir(pdf: &Path) -> Result<PathBuf> {
    let metadata = std::fs::metadata(pdf).with_context(|| format!("reading {}", pdf.display()))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    pdf.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    if let Ok(modified) = metadata.modified()
        && let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH)
    {
        since.as_secs().hash(&mut hasher);
    }
    Ok(paths::data_dir()
        .join("pdf-pages")
        .join(format!("{:016x}", hasher.finish())))
}

/// Blocking on purpose: only ever called from the background executor.
#[allow(clippy::disallowed_methods)]
fn render_pages(pdf: &Path) -> Result<(Vec<Page>, u32)> {
    let dir = cache_dir(pdf)?;
    let manifest = dir.join("pages.json");
    let output: HelperOutput = match std::fs::read_to_string(&manifest) {
        Ok(text) => serde_json::from_str(&text)?,
        Err(_) => {
            let helper = helper_path()?;
            let output = std::process::Command::new(&helper)
                .arg(pdf)
                .arg(&dir)
                .arg(PAGE_RENDER_WIDTH.to_string())
                .arg(MAX_PAGES.to_string())
                .output()
                .with_context(|| format!("running {}", helper.display()))?;
            if !output.status.success() {
                return Err(anyhow!(
                    "brainz-pdf exited with {:?}: {}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            let parsed: HelperOutput =
                serde_json::from_slice(&output.stdout).context("reading brainz-pdf output")?;
            match parsed.status.as_str() {
                "ok" => {}
                "encrypted" => return Err(anyhow!("this PDF is password protected")),
                "unreadable" => return Err(anyhow!("this file could not be read as a PDF")),
                other => return Err(anyhow!("brainz-pdf reported {other}")),
            }
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&manifest, &output.stdout)?;
            parsed
        }
    };
    let pages = output
        .pages
        .into_iter()
        .filter(|page| page.path.is_file())
        .map(|page| Page {
            path: Arc::from(page.path),
            width: page.width,
            height: page.height,
        })
        .collect::<Vec<_>>();
    if pages.is_empty() {
        return Err(anyhow!("this PDF has no pages Brainz could render"));
    }
    Ok((pages, output.total))
}

enum State {
    Loading,
    Ready { pages: Vec<Page>, total: u32 },
    Failed(String),
}

pub struct PdfView {
    focus_handle: FocusHandle,
    item: Entity<PdfItem>,
    project: Entity<Project>,
    state: State,
    scroll: ScrollHandle,
    _load: Task<()>,
}

impl PdfView {
    fn new(item: Entity<PdfItem>, project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let abs_path = item.read(cx).abs_path.clone();
        let load = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let result = cx
                .background_spawn(async move { render_pages(&abs_path) })
                .await;
            this.update(cx, |this, cx| {
                this.state = match result {
                    Ok((pages, total)) => State::Ready { pages, total },
                    Err(error) => {
                        log::error!("brainz pdf: {error:#}");
                        State::Failed(format!("{error:#}"))
                    }
                };
                cx.notify();
            })
            .ok();
        });
        Self {
            focus_handle: cx.focus_handle(),
            item,
            project,
            state: State::Loading,
            scroll: ScrollHandle::new(),
            _load: load,
        }
    }

    fn abs_path(&self, cx: &App) -> PathBuf {
        self.item.read(cx).abs_path.clone()
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let path = self.abs_path(cx);
        let summary: SharedString = match &self.state {
            State::Loading => "Rendering pages…".into(),
            State::Ready { pages, total } => {
                if *total as usize > pages.len() {
                    format!("Showing {} of {total} pages", pages.len()).into()
                } else if pages.len() == 1 {
                    "1 page".into()
                } else {
                    format!("{} pages", pages.len()).into()
                }
            }
            State::Failed(_) => "Could not render".into(),
        };
        h_flex()
            .w_full()
            .px_3()
            .py_1p5()
            .gap_2()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(
                Label::new(summary)
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                Button::new("brainz-pdf-open-default", "Open in Default App")
                    .style(ButtonStyle::Subtle)
                    .label_size(LabelSize::Small)
                    .tooltip(Tooltip::text("Open in Preview or your PDF app"))
                    .on_click(move |_, _, cx| cx.open_with_system(&path)),
            )
    }
}

impl EventEmitter<()> for PdfView {}

impl Focusable for PdfView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PdfView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: gpui::AnyElement = match &self.state {
            State::Loading => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(ui::bouncing_dots(
                    "brainz-pdf-loading",
                    cx.theme().colors().text_accent,
                ))
                .into_any_element(),
            State::Failed(reason) => {
                let path = self.abs_path(cx);
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(Label::new("Could not show this PDF").size(LabelSize::Large))
                    .child(Label::new(reason.clone()).color(Color::Muted))
                    .child(
                        Button::new("brainz-pdf-open-fallback", "Open in Default App")
                            .style(ButtonStyle::Filled)
                            .on_click(move |_, _, cx| cx.open_with_system(&path)),
                    )
                    .into_any_element()
            }
            State::Ready { pages, .. } => {
                let pages = pages.clone();
                v_flex()
                    .id("brainz-pdf-pages")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .items_center()
                    .py_6()
                    .gap_4()
                    .children(pages.into_iter().enumerate().map(|(ix, page)| {
                        let height = DISPLAY_WIDTH * page.height as f32 / page.width.max(1) as f32;
                        div()
                            .id(("brainz-pdf-page", ix))
                            .flex_none()
                            .w(px(DISPLAY_WIDTH))
                            .h(px(height))
                            .rounded_sm()
                            .overflow_hidden()
                            .bg(gpui::white())
                            .shadow_md()
                            .child(
                                gpui::img(page.path)
                                    .size_full()
                                    .object_fit(gpui::ObjectFit::Contain),
                            )
                    }))
                    .into_any_element()
            }
        };
        v_flex()
            .id("brainz-pdf")
            .key_context("BrainzPdf")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .child(self.render_toolbar(cx))
            .child(body)
    }
}

impl Item for PdfView {
    type Event = ();

    fn to_item_events(_: &Self::Event, _: &mut dyn FnMut(ItemEvent)) {}

    fn for_each_project_item(
        &self,
        cx: &App,
        callback: &mut dyn FnMut(gpui::EntityId, &dyn project::ProjectItem),
    ) {
        callback(self.item.entity_id(), self.item.read(cx));
    }

    fn tab_tooltip_text(&self, cx: &App) -> Option<SharedString> {
        Some(self.abs_path(cx).to_string_lossy().into_owned().into())
    }

    fn tab_content(&self, params: TabContentParams, _: &Window, cx: &App) -> gpui::AnyElement {
        Label::new(self.tab_content_text(0, cx))
            .single_line()
            .color(params.text_color())
            .when(params.preview, |this| this.italic())
            .into_any_element()
    }

    fn tab_content_text(&self, _: usize, cx: &App) -> SharedString {
        self.abs_path(cx)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "PDF".to_owned())
            .into()
    }

    fn tab_icon(&self, _: &Window, _: &App) -> Option<Icon> {
        Some(Icon::new(IconName::File))
    }

    fn can_split(&self) -> bool {
        true
    }

    fn clone_on_split(
        &self,
        _: Option<WorkspaceId>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<Entity<Self>>>
    where
        Self: Sized,
    {
        let item = self.item.clone();
        let project = self.project.clone();
        Task::ready(Some(cx.new(|cx| Self::new(item, project, cx))))
    }

    fn buffer_kind(&self, _: &App) -> workspace::item::ItemBufferKind {
        workspace::item::ItemBufferKind::Singleton
    }
}

impl ProjectItem for PdfView {
    type Item = PdfItem;

    fn for_project_item(
        project: Entity<Project>,
        _: Option<&Pane>,
        item: Entity<Self::Item>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self
    where
        Self: Sized,
    {
        Self::new(item, project, cx)
    }
}

pub fn init(cx: &mut App) {
    // Registered after the editor and image viewer, and the registry asks
    // the newest opener first, so PDFs come here.
    workspace::register_project_item::<PdfView>(cx);
}
