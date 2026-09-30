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

const FALLBACK_TITLE: &str = "Google Doc";
const MAX_CACHED_LINKS: usize = 128;
const MAX_TITLE_RESPONSE_BYTES: u64 = 512 * 1024;
const TITLE_TIMEOUT: Duration = Duration::from_secs(8);

pub(crate) fn document_metadata_url(url: &str) -> Option<String> {
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
            document_metadata_url(link.as_str())?;
            Some((link.start()..link.end(), Url::parse(link.as_str()).ok()?))
        })
        .collect()
}

fn document_title(html: &[u8]) -> Option<String> {
    let dom = parse_document(RcDom::default(), Default::default())
        .from_utf8()
        .read_from(&mut &html[..])
        .log_err()?;
    // RcDom clears descendants when the root is dropped, even with cloned handles.
    let mut nodes = vec![dom.document.clone()];
    while let Some(node) = nodes.pop() {
        if let NodeData::Element { name, .. } = &node.data
            && name.local.as_ref() == "title"
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
            // Login and access-error pages are not document metadata.
            let title = title.strip_suffix(" - Google Docs")?.trim();
            return (!title.is_empty()).then(|| title.chars().take(200).collect());
        }
        nodes.extend(node.children.borrow().iter().rev().cloned());
    }
    None
}

async fn fetch_title(
    client: Arc<HttpClientWithUrl>,
    metadata_url: String,
) -> Result<Option<String>> {
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
    Ok(document_title(&html))
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
                    if view.title == FALLBACK_TITLE {
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
        let metadata_url = document_metadata_url(&url);
        let title_task = cx.spawn(async move |this, cx| {
            let (Some(metadata_url), Some(client)) = (metadata_url, client) else {
                return;
            };
            let request = cx
                .background_spawn(fetch_title(client, metadata_url))
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
                Err(error) => log::debug!("Could not load Google Doc title: {error}"),
            }
        });
        Self {
            title: title_hint(title, &url).unwrap_or_else(|| FALLBACK_TITLE.into()),
            url,
            _title_task: title_task,
        }
    }
}

impl Render for DocumentLink {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let url = self.url.clone();
        let button = Button::new(("google-doc-link", cx.entity_id()), self.title.clone())
            .style(ButtonStyle::Subtle)
            .size(ButtonSize::Compact)
            .label_size(LabelSize::Small)
            .start_icon(Icon::new(IconName::File).size(IconSize::XSmall))
            .end_icon(Icon::new(IconName::ArrowUpRight).size(IconSize::XSmall))
            .truncate(true)
            .tooltip(Tooltip::text(format!("{}\n{}", self.title, self.url)))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                cx.open_url(&url);
            });
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
        assert_eq!(document_title(b"<html><head><title> Notes &amp; plans &#8212; Q4 - Google Docs </title></head></html>"), Some("Notes & plans — Q4".into()));
        assert_eq!(
            document_title(b"<title>Sign in - Google Accounts</title>"),
            None
        );
        assert_eq!(document_title(b"<title>Google Docs</title>"), None);
        assert_eq!(document_title(b"<title>Page not found</title>"), None);
    }
}
