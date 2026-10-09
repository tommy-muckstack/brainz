use std::path::Path;

pub(crate) use brainz_ai::Feature;
use brainz_ai::{bounded, snapshot};
use chrono::{Local, TimeZone};
use gpui::{Entity, Subscription};
use markdown::{Markdown, MarkdownElement};
use ui::prelude::*;

pub(crate) struct Insight {
    backend: Entity<brainz_ai::Insight>,
    markdown: Option<Entity<Markdown>>,
    text: Option<String>,
    _subscription: Subscription,
}

impl Insight {
    pub(crate) fn for_repo(repo: &Path, feature: Feature, cx: &mut App) -> Entity<Self> {
        let backend = brainz_ai::Insight::for_repo(repo, feature, cx);
        cx.new(|cx| {
            let subscription = cx.observe(&backend, |this: &mut Self, backend, cx| {
                let text = backend
                    .read(cx)
                    .cached
                    .as_ref()
                    .map(|cached| cached.text.as_str());
                if this.text.as_deref() != text {
                    this.text = text.map(ToOwned::to_owned);
                    this.markdown = None;
                }
                cx.notify();
            });
            Self {
                backend,
                markdown: None,
                text: None,
                _subscription: subscription,
            }
        })
    }

    pub(crate) fn set_context(&mut self, context: String, cx: &mut Context<Self>) {
        self.backend
            .update(cx, |backend, cx| backend.set_context(context, cx));
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        self.backend.update(cx, |backend, cx| backend.retry(cx));
    }
}

pub(crate) fn brief_context(brief: &crate::brief::Brief) -> String {
    use serde_json::json;
    snapshot(json!({
        "date": Local::now().date_naive().to_string(),
        "as_of_hour": brief.generated_at.map(|time| time.format("%Y-%m-%d %H:00 %:z").to_string()),
        "owner": bounded(&brief.owner, 120),
        "calendar_unavailable": brief.calendar_problem.is_some(),
        "events": brief.events.iter().take(16).map(|event| json!({
            "title": bounded(&event.title, 300), "start": event.start.to_rfc3339(),
            "all_day": event.all_day, "folder": event.folder.as_deref().map(|v| bounded(v, 200)),
            "prep_file": event.prep_file.as_deref().map(|v| bounded(v, 200)),
        })).collect::<Vec<_>>(),
        "open_tasks": brief.owes.iter().take(24).map(|task| json!({
            "section": bounded(&task.section, 100), "task": bounded(&task.title, 350),
            "tag": task.tag.as_deref().map(|v| bounded(v, 100)),
        })).collect::<Vec<_>>(),
        "tasks_source": bounded(&brief.todo_file, 200),
        "tasks_updated": brief.owes_updated.as_deref().map(|v| bounded(v, 120)),
        "blocked": brief.blocked.iter().take(8).map(|v| bounded(v, 350)).collect::<Vec<_>>(),
        "decisions": brief.decisions.iter().take(8).map(|v| bounded(v, 350)).collect::<Vec<_>>(),
        "ops_updated": brief.ops_updated.as_deref().map(|v| bounded(v, 120)),
        "open_loops": loop_context(&brief.loops),
        "stale_folders": brief.stale.iter().take(8).map(|entry| json!({
            "folder": bounded(&entry.folder, 200), "status_date": entry.status_date.to_string(),
            "newest_note_date": entry.newest_date.to_string(),
        })).collect::<Vec<_>>(),
        "unlogged_recordings": brief.recordings.iter().take(6).map(|meeting| json!({
            "title": bounded(&meeting.title, 200), "started": meeting.started.to_rfc3339(),
        })).collect::<Vec<_>>(),
        "scope": "A limited snapshot; omitted items may exist. No meeting transcripts or note bodies included."
    }))
}

pub(crate) fn themes_context(signals: &crate::themes_signals::Signals) -> String {
    use serde_json::json;
    let mut themes = signals
        .themes
        .iter()
        .filter(|theme| !theme.hidden)
        .collect::<Vec<_>>();
    themes.sort_by_key(|theme| {
        (
            std::cmp::Reverse(theme.pinned),
            std::cmp::Reverse(theme.recent),
            std::cmp::Reverse(theme.total),
        )
    });
    snapshot(json!({
        "snapshot_date": bounded(&signals.generated_at, 120),
        "weeks": signals.weeks.iter().take(52).map(|v| bounded(v, 32)).collect::<Vec<_>>(),
        "themes": themes.into_iter().take(24).map(|theme| json!({
            "name": bounded(&theme.name, 100), "category": theme.category,
            "recent": theme.recent, "prior": theme.prior, "momentum": theme.momentum,
            "new": theme.is_new, "pinned": theme.pinned,
            "folders": theme.folders.iter().take(3).map(|folder| bounded(&folder.name, 200)).collect::<Vec<_>>(),
            "source_files": theme.files.iter().take(2).map(|file| bounded(&file.name, 200)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "open_loops": loop_context(&signals.open_loops),
        "scope": "Up to 24 visible themes ranked by pins and recent activity. Recent and prior are weighted note mentions, not measures of success. Hidden themes are excluded."
    }))
}

pub(crate) fn todo_context(
    board: &crate::todo::TodoBoard,
    loops: &[crate::themes_signals::OpenLoop],
    tasks: &[crate::myman::MyManTask],
    source: &str,
    unavailable: bool,
) -> String {
    use serde_json::json;
    snapshot(json!({
        "date": Local::now().date_naive().to_string(),
        "source": bounded(source, 200), "updated": board.updated.as_deref().map(|v| bounded(v, 120)), "board_unavailable": unavailable,
        "open_tasks": board.sections.iter().flat_map(|section| section.items.iter().filter(|item| !item.done).map(move |item| json!({
            "section": bounded(&section.name, 100), "task": bounded(&item.title, 350),
            "tag": item.tag.as_deref().map(|v| bounded(v, 100)),
            "note": item.note.as_deref().map(|v| bounded(v, 200)),
        }))).take(24).collect::<Vec<_>>(),
        "flagged_in_notes": loop_context(loops),
        "meeting_tasks": tasks.iter().take(8).map(|task| bounded(&task.board_line(), 350)).collect::<Vec<_>>(),
        "scope": "A limited snapshot of open tasks. Completed tasks excluded. Suggestions do not change the board."
    }))
}

fn loop_context(loops: &[crate::themes_signals::OpenLoop]) -> Vec<serde_json::Value> {
    loops
        .iter()
        .take(8)
        .map(|item| {
            serde_json::json!({
                "source": bounded(&item.file, 200), "text": bounded(&item.text, 300),
                "first_seen": bounded(&item.first_seen, 32), "marker": bounded(&item.marker, 12),
            })
        })
        .collect()
}

impl Render for Insight {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (enabled, running, outdated, error, feature, cached) = {
            let backend = self.backend.read(cx);
            (
                backend.enabled,
                backend.running,
                backend.outdated,
                backend.error,
                backend.feature,
                backend.cached.clone(),
            )
        };
        if !enabled {
            return div().into_any_element();
        }
        let cached = cached.as_ref();
        if self.markdown.is_none() {
            self.markdown = cached.map(|cached| {
                cx.new(|cx| Markdown::new(cached.text.clone().into(), None, None, cx))
            });
        }
        let subtitle = if running {
            "Updating with AI…".to_owned()
        } else if let Some(cached) = cached {
            let time = Local
                .timestamp_opt(cached.generated_at, 0)
                .single()
                .map(|time| time.format("%b %-d, %-I:%M %p").to_string())
                .unwrap_or_default();
            format!(
                "{} · {}{}",
                cached.model,
                time,
                if outdated {
                    " · previous snapshot"
                } else {
                    ""
                }
            )
        } else {
            "Automatic AI insight".to_owned()
        };
        v_flex()
            .w_full()
            .my_2()
            .p_3()
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().colors().border_variant)
            .bg(cx.theme().colors().surface_background)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Icon::new(IconName::Sparkle)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new(feature.title())
                            .size(LabelSize::Small)
                            .weight(gpui::FontWeight::SEMIBOLD),
                    )
                    .child(
                        Label::new(subtitle)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .when_some(self.markdown.clone(), |this, markdown| {
                this.child(MarkdownElement::new(
                    markdown,
                    crate::brief::BriefView::narrative_style(window, cx),
                ))
            })
            .when_some(error, |this, error| {
                this.child(
                    h_flex()
                        .gap_2()
                        .child(Label::new(error).size(LabelSize::Small).color(Color::Muted))
                        .child(
                            Button::new("retry-background-ai", "Retry")
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                        ),
                )
            })
            .when(outdated && !running && error.is_none(), |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .child(
                            Label::new("An update is scheduled.")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .child(
                            Button::new("update-background-ai", "Update now")
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                        ),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brainz_ai::MAX_CONTEXT_BYTES;

    #[test]
    fn snapshots_are_bounded_and_exclude_completed_or_hidden_items() {
        let board = crate::todo::parse_board(
            "# Tasks\n## Today\n- [ ] Call Ada\n- [x] Already completed\n",
        );
        let snapshot = todo_context(&board, &[], &[], "TODO.md", false);
        assert!(snapshot.contains("Call Ada"));
        assert!(!snapshot.contains("Already completed"));
        let signals = crate::themes_signals::Signals {
            themes: vec![
                crate::themes_signals::Theme {
                    name: "Hidden topic".into(),
                    hidden: true,
                    ..Default::default()
                },
                crate::themes_signals::Theme {
                    name: "Visible topic".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let context = themes_context(&signals);
        assert!(context.contains("Visible topic"));
        assert!(!context.contains("Hidden topic"));
        let mut brief = crate::brief::Brief::default();
        for _ in 0..500 {
            brief.owes.push(crate::brief::Owe {
                section: "Today".into(),
                title: "🧠".repeat(500),
                tag: None,
            });
        }
        brief.blocked = vec!["x".repeat(400); 50];
        brief.decisions = vec!["y".repeat(400); 50];
        let context = brief_context(&brief);
        assert!(context.len() <= MAX_CONTEXT_BYTES);
        assert!(serde_json::from_str::<serde_json::Value>(&context).is_ok());
        assert_eq!(bounded("🧠🧠", 5), "🧠");
    }
}
