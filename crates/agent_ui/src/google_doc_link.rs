use std::{collections::VecDeque, ops::Range, sync::Arc, time::Duration};

use anyhow::Result;
use futures::{AsyncReadExt, FutureExt};
use gpui::{Entity, Global, IntoElement, Task};
use html5ever::{parse_document, tendril::TendrilSink};
use http_client::HttpClientWithUrl;
use markup5ever_rcdom::{NodeData, RcDom};
use ui::{Tooltip, prelude::*};
use url::Url;
use util::ResultExt;

const MAX_CACHED_LINKS: usize = 128;

/// Brainz: the kinds of shared-note links that become title pills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkKind {
    GoogleDoc,
    Granola,
    WisprFlow,
    GitHub,
}

impl LinkKind {
    /// The label a pill shows before (or without) a fetched title. GitHub
    /// labels come from the path, so even an unreachable link reads well.
    pub(crate) fn fallback_label(self, url: &str) -> String {
        match self {
            LinkKind::GoogleDoc => "Google Doc".to_owned(),
            LinkKind::Granola => "Granola notes".to_owned(),
            LinkKind::WisprFlow => "Wispr Flow notes".to_owned(),
            LinkKind::GitHub => github_label(url).unwrap_or_else(|| "GitHub".to_owned()),
        }
    }

    /// A logo for the pill when Brainz has one; otherwise the icon.
    pub(crate) fn logo(self) -> Option<&'static str> {
        match self {
            LinkKind::Granola => Some("icons/brainz/logos/granola.png"),
            LinkKind::GoogleDoc | LinkKind::WisprFlow | LinkKind::GitHub => None,
        }
    }

    pub(crate) fn icon(self) -> IconName {
        match self {
            LinkKind::GitHub => IconName::Github,
            LinkKind::GoogleDoc | LinkKind::Granola | LinkKind::WisprFlow => IconName::File,
        }
    }

    /// Page titles that only name the service, not the note.
    fn is_generic_title(self, title: &str) -> bool {
        let lower = title.trim().to_lowercase();
        match self {
            LinkKind::GoogleDoc => lower == "google docs",
            LinkKind::Granola => lower == "granola" || lower == "granola notes",
            LinkKind::WisprFlow => lower.starts_with("wispr flow"),
            LinkKind::GitHub => lower == "github",
        }
    }

    /// Trims the service's framing off a fetched title: GitHub's
    /// "<title> by <user> · Pull Request #33 · owner/repo" becomes
    /// "#33 <title>", and a repo page becomes "owner/repo".
    fn clean_title(self, title: String, url: &str) -> String {
        if self != LinkKind::GitHub {
            return title;
        }
        let title = title.trim().trim_end_matches(" · GitHub").to_owned();
        if let Some(rest) = title.strip_prefix("GitHub - ") {
            return rest.split(':').next().unwrap_or(rest).trim().to_owned();
        }
        for marker in [" · Pull Request #", " · Issue #", " · Discussion #"] {
            if let Some((head, tail)) = title.split_once(marker) {
                let number: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
                let head = head.rsplit_once(" by ").map(|(h, _)| h).unwrap_or(head);
                return format!("#{number} {}", head.trim());
            }
        }
        if let Some((head, _)) = title.split_once(" · ") {
            return head.trim().to_owned();
        }
        github_label(url).unwrap_or(title)
    }
}

/// `owner/repo`, `owner/repo #33`, `owner/repo@abc1234`, or
/// `owner/repo · path/to/file` from a github.com URL.
fn github_label(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let segments: Vec<&str> = parsed.path_segments()?.filter(|s| !s.is_empty()).collect();
    let (owner, repo) = match segments.as_slice() {
        [owner, repo, ..] => (*owner, repo.trim_end_matches(".git")),
        [owner] => return Some((*owner).to_owned()),
        _ => return None,
    };
    let base = format!("{owner}/{repo}");
    Some(match &segments[2..] {
        ["pull" | "issues" | "discussions", number, ..] => format!("{base} #{number}"),
        ["commit", sha, ..] => format!("{base}@{}", sha.chars().take(7).collect::<String>()),
        ["blob" | "tree", _branch, rest @ ..] if !rest.is_empty() => {
            format!("{base} · {}", rest.join("/"))
        }
        ["releases", "tag", tag] => format!("{base} {tag}"),
        _ => base,
    })
}

/// Recognizes a shared-note link and the URL whose HTML carries its title.
pub(crate) fn known_link(url: &str) -> Option<(LinkKind, String)> {
    if let Some(metadata_url) = google_doc_metadata_url(url) {
        return Some((LinkKind::GoogleDoc, metadata_url));
    }
    let parsed = Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "https" | "http") || !parsed.username().is_empty() {
        return None;
    }
    let segments: Vec<&str> = parsed.path_segments()?.filter(|s| !s.is_empty()).collect();
    match (parsed.host_str()?, segments.as_slice()) {
        ("notes.granola.ai", ["t", id, ..]) if !id.is_empty() => Some((
            LinkKind::Granola,
            format!("https://notes.granola.ai/t/{id}"),
        )),
        ("notes.wisprflow.ai", ["shared", id, ..]) if !id.is_empty() => Some((
            LinkKind::WisprFlow,
            format!("https://notes.wisprflow.ai/shared/{id}"),
        )),
        ("github.com" | "www.github.com", [_owner, _repo, ..]) => {
            let mut metadata_url = parsed.clone();
            metadata_url.set_fragment(None);
            Some((LinkKind::GitHub, metadata_url.into()))
        }
        _ => None,
    }
}

pub(crate) fn link_kind(url: &str) -> Option<LinkKind> {
    known_link(url).map(|(kind, _)| kind)
}

/// Absolute paths to files that exist on this Mac, pasted as text.
pub(crate) fn pasted_local_files(text: &str) -> Vec<(Range<usize>, std::path::PathBuf)> {
    let home = util::paths::home_dir();
    let mut files = Vec::new();
    let mut offset = 0;
    for token in text.split_inclusive(char::is_whitespace) {
        let trimmed = token.trim_end();
        let core = trimmed.trim_end_matches([',', ';', ')', ']', '>', '"', '\'']);
        if core.starts_with('/') || core.starts_with("~/") {
            let path = if let Some(rest) = core.strip_prefix("~/") {
                home.join(rest)
            } else {
                std::path::PathBuf::from(core)
            };
            if path.is_file() {
                files.push((offset..offset + core.len(), path));
            }
        }
        offset += token.len();
    }
    files
}
const MAX_TITLE_RESPONSE_BYTES: u64 = 512 * 1024;
const TITLE_TIMEOUT: Duration = Duration::from_secs(8);

/// The URL to fetch for a recognized link's title, if the link is one Brainz
/// turns into a pill.
pub(crate) fn document_metadata_url(url: &str) -> Option<String> {
    known_link(url).map(|(_, metadata_url)| metadata_url)
}

fn google_doc_metadata_url(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str() != Some("docs.google.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let segments = url.path_segments()?.collect::<Vec<_>>();
    let rest = match segments.as_slice() {
        ["document", "d", rest @ ..] => rest,
        ["document", "u", account, "d", rest @ ..]
            if !account.is_empty() && account.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            rest
        }
        _ => return None,
    };
    let (identifier, published) = match rest {
        ["e", identifier, "pub", ..] => (*identifier, true),
        [identifier, ..] if *identifier != "e" => (*identifier, false),
        _ => return None,
    };
    if identifier.is_empty()
        || !identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    let metadata_url = if published {
        format!("https://docs.google.com/document/d/e/{identifier}/pub")
    } else {
        format!("https://docs.google.com/document/d/{identifier}/edit")
    };
    let mut metadata_url = Url::parse(&metadata_url).ok()?;
    if let Some((_, resource_key)) = url.query_pairs().find(|(key, _)| key == "resourcekey") {
        metadata_url
            .query_pairs_mut()
            .append_pair("resourcekey", &resource_key);
    }
    Some(metadata_url.into())
}

pub(crate) fn pasted_document_links(text: &str) -> Vec<(Range<usize>, Url)> {
    let mut finder = linkify::LinkFinder::new();
    finder.kinds(&[linkify::LinkKind::Url]);
    finder
        .links(text)
        .filter_map(|link| {
            known_link(link.as_str())?;
            Some((link.start()..link.end(), Url::parse(link.as_str()).ok()?))
        })
        .collect()
}

fn document_title(html: &[u8], kind: LinkKind) -> Option<String> {
    let dom = parse_document(RcDom::default(), Default::default())
        .from_utf8()
        .read_from(&mut &html[..])
        .log_err()?;
    // RcDom clears descendants when the root is dropped, even with cloned
    // handles, so `dom` has to outlive the walk; the clone is deliberate.
    #[allow(clippy::redundant_clone)]
    let mut nodes = vec![dom.document.clone()];
    let mut og_title: Option<String> = None;
    let mut page_title: Option<String> = None;
    while let Some(node) = nodes.pop() {
        if let NodeData::Element { name, attrs, .. } = &node.data
            && name.local.as_ref() == "meta"
        {
            let attrs = attrs.borrow();
            let is_og_title = attrs.iter().any(|attribute| {
                attribute.name.local.as_ref() == "property" && &*attribute.value == "og:title"
            });
            if is_og_title
                && let Some(content) = attrs
                    .iter()
                    .find(|attribute| attribute.name.local.as_ref() == "content")
            {
                let content = content
                    .value
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if !content.is_empty() && og_title.is_none() {
                    og_title = Some(content);
                }
            }
        }
        if let NodeData::Element { name, .. } = &node.data
            && name.local.as_ref() == "title"
            && page_title.is_none()
        {
            let mut title = String::new();
            let mut children = node
                .children
                .borrow()
                .iter()
                .rev()
                .cloned()
                .collect::<Vec<_>>();
            while let Some(child) = children.pop() {
                if let NodeData::Text { contents } = &child.data {
                    title.push_str(&contents.borrow());
                }
                children.extend(child.children.borrow().iter().rev().cloned());
            }
            let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
            page_title = Some(title);
        }
        nodes.extend(node.children.borrow().iter().rev().cloned());
    }
    let title = match kind {
        // Login and access-error pages are not document metadata.
        LinkKind::GoogleDoc => page_title?
            .strip_suffix(" - Google Docs")?
            .trim()
            .to_owned(),
        LinkKind::Granola | LinkKind::WisprFlow | LinkKind::GitHub => {
            og_title.or(page_title)?.trim().to_owned()
        }
    };
    if title.is_empty() || kind.is_generic_title(&title) {
        return None;
    }
    Some(title.chars().take(200).collect())
}

async fn fetch_title(
    client: Arc<HttpClientWithUrl>,
    metadata_url: String,
    kind: LinkKind,
) -> Result<Option<String>> {
    let original_url = metadata_url.clone();
    let mut response = client.get(&metadata_url, Default::default(), true).await?;
    if !response.status().is_success() {
        return Ok(None);
    }
    let mut html = Vec::new();
    response
        .body_mut()
        .take(MAX_TITLE_RESPONSE_BYTES)
        .read_to_end(&mut html)
        .await?;
    Ok(document_title(&html, kind).map(|title| kind.clean_title(title, &original_url)))
}

#[derive(Default)]
struct DocumentLinkCache(VecDeque<(String, Entity<DocumentLink>)>);

impl Global for DocumentLinkCache {}

#[derive(IntoElement)]
pub(crate) struct GoogleDocLink {
    url: String,
    title: Option<String>,
    client: Option<Arc<HttpClientWithUrl>>,
}

impl GoogleDocLink {
    pub(crate) fn new(
        url: impl Into<String>,
        title: Option<String>,
        client: Option<Arc<HttpClientWithUrl>>,
    ) -> Self {
        Self {
            url: url.into(),
            title,
            client,
        }
    }
}

fn title_hint(title: Option<String>, url: &str) -> Option<SharedString> {
    title
        .filter(|title| {
            !title.trim().is_empty()
                && title != url
                && !title.starts_with("http://")
                && !title.starts_with("https://")
        })
        .map(|title| title.trim().chars().take(200).collect::<String>().into())
}

impl RenderOnce for GoogleDocLink {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let cache = cx.default_global::<DocumentLinkCache>();
        if let Some(index) = cache.0.iter().position(|(url, _)| url == &self.url)
            && let Some(entry) = cache.0.remove(index)
        {
            let view = entry.1.clone();
            cache.0.push_back(entry);
            if let Some(title) = title_hint(self.title, &self.url) {
                view.update(cx, |view, cx| {
                    if view.title == view.kind.fallback_label(&view.url) {
                        view.title = title;
                        cx.notify();
                    }
                });
            }
            return view;
        }
        let url = self.url.clone();
        let cacheable = self.client.is_some();
        let view = cx.new(|cx| DocumentLink::new(self.url, self.title, self.client, cx));
        if !cacheable {
            return view;
        }
        let cache = cx.default_global::<DocumentLinkCache>();
        while cache.0.len() >= MAX_CACHED_LINKS {
            cache.0.pop_front();
        }
        cache.0.push_back((url, view.clone()));
        view
    }
}

struct DocumentLink {
    url: String,
    kind: LinkKind,
    title: SharedString,
    _title_task: Task<()>,
}

impl DocumentLink {
    fn new(
        url: String,
        title: Option<String>,
        client: Option<Arc<HttpClientWithUrl>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (kind, metadata_url) = match known_link(&url) {
            Some((kind, metadata_url)) => (kind, Some(metadata_url)),
            None => (LinkKind::GoogleDoc, None),
        };
        let title_task = cx.spawn(async move |this, cx| {
            let (Some(metadata_url), Some(client)) = (metadata_url, client) else {
                return;
            };
            let request = cx
                .background_spawn(fetch_title(client, metadata_url, kind))
                .fuse();
            let timeout = cx.background_executor().timer(TITLE_TIMEOUT).fuse();
            futures::pin_mut!(request, timeout);
            let result = futures::select! {
                result = request => result,
                _ = timeout => return,
            };
            match result {
                Ok(Some(title)) => {
                    this.update(cx, |this, cx| {
                        this.title = title.into();
                        cx.notify();
                    })
                    .log_err();
                }
                Ok(None) => {}
                Err(error) => log::debug!("Could not load link title: {error}"),
            }
        });
        Self {
            title: title_hint(title, &url).unwrap_or_else(|| kind.fallback_label(&url).into()),
            url,
            kind,
            _title_task: title_task,
        }
    }
}

impl Render for DocumentLink {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let url = self.url.clone();
        let glyph: AnyElement = match self.kind.logo() {
            Some(logo) => gpui::img(logo.to_owned())
                .size_3()
                .flex_none()
                .object_fit(gpui::ObjectFit::Contain)
                .into_any_element(),
            None => Icon::new(self.kind.icon())
                .size(IconSize::XSmall)
                .into_any_element(),
        };
        let button = ui::ButtonLike::new(("google-doc-link", cx.entity_id()))
            .style(ButtonStyle::Subtle)
            .size(ButtonSize::Compact)
            .tooltip(Tooltip::text(format!("{}\n{}", self.title, self.url)))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                cx.open_url(&url);
            })
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .min_w_0()
                    .child(glyph)
                    .child(
                        Label::new(self.title.clone())
                            .size(LabelSize::Small)
                            .truncate(),
                    )
                    .child(
                        Icon::new(IconName::ArrowUpRight)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    ),
            );
        h_flex().max_w(rems(24.)).child(
            div()
                .id(("google-doc-pill", cx.entity_id()))
                .debug_selector(|| "google-doc-pill".into())
                .max_w_full()
                .rounded_full()
                .border_1()
                .border_color(cx.theme().colors().border)
                .bg(cx.theme().colors().element_background)
                .overflow_hidden()
                .child(button),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn google_doc_title_loads_and_pill_opens_the_original_url(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            settings::init(cx);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
        let client = http_client::FakeHttpClient::create(|request| async move {
            assert_eq!(
                request.uri().to_string(),
                "https://docs.google.com/document/d/abc123/edit"
            );
            Ok(http_client::Response::builder()
                .status(200)
                .body("<title>Notes with Vivek - Google Docs</title>".into())
                .unwrap())
        });
        let url = "https://docs.google.com/document/d/abc123/edit?usp=sharing#heading=h.one";
        let (view, cx) =
            cx.add_window_view(|_, cx| DocumentLink::new(url.into(), None, Some(client), cx));
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.title.to_string()),
            "Notes with Vivek"
        );
        let bounds = cx
            .debug_bounds("google-doc-pill")
            .expect("document pill is rendered");
        assert!(bounds.size.width <= px(384.));
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
        assert_eq!(cx.opened_url().as_deref(), Some(url));
    }

    #[test]
    fn recognizes_docs_and_preserves_original_pasted_urls() {
        let text = "Notes https://docs.google.com/document/d/abc_123/edit?usp=sharing#heading=h.one and https://docs.google.com/document/u/0/d/xyz-456/edit. Done";
        let links = pasted_document_links(text);
        assert_eq!(links.len(), 2);
        for (range, url) in &links {
            assert_eq!(&text[range.clone()], url.as_str());
        }
        assert_eq!(
            document_metadata_url(links[0].1.as_str()).as_deref(),
            Some("https://docs.google.com/document/d/abc_123/edit")
        );
        assert_eq!(
            document_metadata_url(links[1].1.as_str()).as_deref(),
            Some("https://docs.google.com/document/d/xyz-456/edit")
        );
        assert_eq!(
            document_metadata_url("https://docs.google.com/document/d/abc/edit?usp=sharing&resourcekey=key-123#heading=h.one").as_deref(),
            Some("https://docs.google.com/document/d/abc/edit?resourcekey=key-123")
        );
        for url in [
            "https://docs.google.com.evil.example/document/d/abc/edit",
            "https://docs.google.com/spreadsheets/d/abc/edit",
            "https://docs.google.com/document/d/",
            "https://user@docs.google.com/document/d/abc/edit",
        ] {
            assert!(document_metadata_url(url).is_none());
        }
    }

    #[test]
    fn parses_doc_titles_without_showing_login_or_error_titles() {
        assert_eq!(document_title(b"<html><head><title> Notes &amp; plans &#8212; Q4 - Google Docs </title></head></html>", LinkKind::GoogleDoc), Some("Notes & plans — Q4".into()));
        assert_eq!(
            document_title(
                b"<title>Sign in - Google Accounts</title>",
                LinkKind::GoogleDoc
            ),
            None
        );
        assert_eq!(
            document_title(b"<title>Google Docs</title>", LinkKind::GoogleDoc),
            None
        );
        assert_eq!(
            document_title(b"<title>Page not found</title>", LinkKind::GoogleDoc),
            None
        );
        assert_eq!(
            document_title(b"<html><head><meta property=\"og:title\" content=\"Grace / Ada\"><title>Granola</title></head></html>", LinkKind::Granola),
            Some("Grace / Ada".into())
        );
        assert_eq!(
            document_title(b"<title>Wispr Flow Notes</title>", LinkKind::WisprFlow),
            None
        );
        assert_eq!(
            known_link(
                "https://notes.granola.ai/t/00000000-0000-4000-8000-000000000000-abcdefgh?x=1"
            )
            .map(|(kind, _)| kind),
            Some(LinkKind::Granola)
        );
        assert_eq!(
            known_link(
                "https://notes.wisprflow.ai/shared/ExampleSharedNoteId0000000000000000000000"
            )
            .map(|(kind, _)| kind),
            Some(LinkKind::WisprFlow)
        );
        assert_eq!(known_link("https://example.com/t/abc"), None);
        assert_eq!(
            known_link("https://github.com/acme/widgets/pull/33#discussion_r1"),
            Some((
                LinkKind::GitHub,
                "https://github.com/acme/widgets/pull/33".to_owned()
            ))
        );
        assert_eq!(known_link("https://github.com/acme"), None);
        assert_eq!(
            LinkKind::GitHub.fallback_label("https://github.com/acme/widgets"),
            "acme/widgets"
        );
        assert_eq!(
            LinkKind::GitHub.fallback_label("https://github.com/acme/widgets/pull/33"),
            "acme/widgets #33"
        );
        assert_eq!(
            LinkKind::GitHub.fallback_label("https://github.com/acme/widgets/issues/7"),
            "acme/widgets #7"
        );
        assert_eq!(
            LinkKind::GitHub.fallback_label("https://github.com/acme/widgets/commit/abcdef1234567"),
            "acme/widgets@abcdef1"
        );
        assert_eq!(
            LinkKind::GitHub.fallback_label("https://github.com/acme/widgets/blob/main/src/lib.rs"),
            "acme/widgets · src/lib.rs"
        );
        assert_eq!(
            LinkKind::GitHub.fallback_label("https://github.com/acme/widgets/releases/tag/v0.1.0"),
            "acme/widgets v0.1.0"
        );
        assert_eq!(
            LinkKind::GitHub.clean_title(
                "Scrub names from fixtures by ada · Pull Request #33 · acme/widgets".into(),
                "https://github.com/acme/widgets/pull/33"
            ),
            "#33 Scrub names from fixtures"
        );
        assert_eq!(
            LinkKind::GitHub.clean_title(
                "GitHub - acme/widgets: Widgets at the speed of thought".into(),
                "https://github.com/acme/widgets"
            ),
            "acme/widgets"
        );
        assert_eq!(
            LinkKind::GitHub.clean_title(
                "widgets/README.md at main · acme/widgets".into(),
                "https://github.com/acme/widgets/blob/main/README.md"
            ),
            "widgets/README.md at main"
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("2026-10-01-C387B67A.md");
        std::fs::write(&file, "notes").unwrap();
        let text = format!("MyMan: {} and /definitely/missing.md", file.display());
        let found = pasted_local_files(&text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, file);
        assert_eq!(&text[found[0].0.clone()], file.to_string_lossy().as_ref());
    }
}
