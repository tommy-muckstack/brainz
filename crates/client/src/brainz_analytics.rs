use anyhow::Result;
use posthog_rs::{Client, ClientOptionsBuilder};
use serde_json::{Map, Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use telemetry_events::FlexibleEvent;

pub(crate) struct BrainzAnalytics {
    client: Client,
    installation_id: String,
    session_id: String,
    version: String,
}

impl BrainzAnalytics {
    pub(crate) fn from_environment(
        installation_id: &str,
        session_id: &str,
        version: &str,
        enabled: Arc<AtomicBool>,
    ) -> Result<Option<Arc<Self>>> {
        let token = std::env::var("BRAINZ_POSTHOG_TOKEN")
            .ok()
            .or_else(|| option_env!("BRAINZ_POSTHOG_TOKEN").map(str::to_owned));
        let host = std::env::var("BRAINZ_POSTHOG_HOST")
            .ok()
            .or_else(|| option_env!("BRAINZ_POSTHOG_HOST").map(str::to_owned));
        let (Some(token), Some(host)) = (token, host) else {
            return Ok(None);
        };
        if token.trim().is_empty() || host.trim().is_empty() {
            return Ok(None);
        }
        let mut options = ClientOptionsBuilder::default();
        options
            .api_key(token)
            .host(host)
            .is_server(false)
            .disable_geoip(true)
            .request_timeout_seconds(5)
            .shutdown_timeout_ms(2000)
            .flush_at(20)
            .max_queue_size(500)
            .flush_interval_ms(15000);
        // Opting out also drops events already waiting in the SDK's queue.
        options.before_send(move |event| enabled.load(Ordering::Relaxed).then_some(event));
        Ok(Some(Arc::new(Self {
            client: posthog_rs::client(options.build()?),
            installation_id: installation_id.to_owned(),
            session_id: session_id.to_owned(),
            version: version.to_owned(),
        })))
    }

    pub(crate) fn capture(&self, name: &str, properties: Map<String, Value>) -> Result<()> {
        let mut event = posthog_rs::Event::new(name, &self.installation_id);
        event.insert_prop("app", "brainz")?;
        event.insert_prop("app_version", &self.version)?;
        event.insert_prop("$session_id", &self.session_id)?;
        event.insert_prop("$process_person_profile", false)?;
        for (key, value) in properties {
            event.insert_prop(key, value)?;
        }
        self.client.capture(event);
        Ok(())
    }

    #[cfg_attr(feature = "test-support", allow(dead_code))]
    pub(crate) fn shutdown(&self) {
        self.client.shutdown();
    }
}

// Inherited telemetry carries arbitrary text, paths, and provider errors.
// Only this explicit event/property allowlist can reach PostHog.
pub(crate) fn sanitized_event(event: &FlexibleEvent) -> Option<(&'static str, Map<String, Value>)> {
    let mut properties = Map::new();
    let name = match event.event_type.as_str() {
        "Brainz Session Started" => "brainz session started",
        "App Closed" => "brainz session ended",
        "Agent Message Sent" => "brainz chat message sent",
        "Agent Turn Completed" => {
            copy_enum(event, &mut properties, "status", &["success", "failure"]);
            copy_number(event, &mut properties, "turn_time_ms");
            "brainz chat turn completed"
        }
        "Agent Panel Error Shown" => {
            copy_enum(
                event,
                &mut properties,
                "kind",
                &[
                    "payment_required",
                    "refusal",
                    "data_retention_consent_required",
                    "authentication_required",
                    "rate_limit_exceeded",
                    "server_overloaded",
                    "prompt_too_large",
                    "no_api_key",
                    "permission_denied",
                    "provider_rejection",
                    "max_output_tokens",
                    "no_model_selected",
                    "api_error",
                    "other",
                ],
            );
            "brainz chat error shown"
        }
        "Brainz Note Created" => {
            copy_enum(event, &mut properties, "source", &["blank", "template"]);
            "brainz note created"
        }
        "Brainz View Opened" => {
            copy_enum(
                event,
                &mut properties,
                "view",
                &["note", "canvas", "themes", "calendar", "todo", "connectors"],
            );
            "brainz view opened"
        }
        "Brainz Chat Copied" => {
            copy_enum(
                event,
                &mut properties,
                "scope",
                &["selection", "message", "thread"],
            );
            "brainz chat copied"
        }
        "Brainz MCP Selection Changed" => {
            copy_enum(
                event,
                &mut properties,
                "selection",
                &["all", "none", "custom"],
            );
            copy_number(event, &mut properties, "selected_count");
            copy_number(event, &mut properties, "available_count");
            "brainz mcp selection changed"
        }
        "Brainz Theme Action" => {
            copy_enum(
                event,
                &mut properties,
                "action",
                &["pin", "unpin", "hide", "rename", "merge", "refresh"],
            );
            "brainz theme action"
        }
        "Settings Viewed" => "brainz settings opened",
        _ => return None,
    };
    if event.event_type.starts_with("Agent ") {
        copy_enum(
            event,
            &mut properties,
            "agent",
            &[
                "claude-acp",
                "claude",
                "codex-acp",
                "codex",
                "zed",
                "native",
                "gemini",
            ],
        );
    }
    Some((name, properties))
}

fn copy_enum(
    event: &FlexibleEvent,
    properties: &mut Map<String, Value>,
    key: &str,
    allowed: &[&str],
) {
    if let Some(value) = event.event_properties.get(key).and_then(Value::as_str)
        && allowed.contains(&value)
    {
        properties.insert(key.to_owned(), json!(value));
    }
}

fn copy_number(event: &FlexibleEvent, properties: &mut Map<String, Value>, key: &str) {
    if let Some(value) = event.event_properties.get(key).and_then(Value::as_u64) {
        properties.insert(key.to_owned(), json!(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "sends a labeled setup-validation event to the configured PostHog project"]
    fn verify_configured_posthog_delivery() {
        let analytics = BrainzAnalytics::from_environment(
            "brainz-setup-validation",
            "00000000-0000-4000-8000-000000000061",
            "0.1.61",
            Arc::new(AtomicBool::new(true)),
        )
        .expect("analytics configuration")
        .expect("configured project");
        analytics
            .capture(
                "brainz analytics verified",
                Map::from_iter([("validation_only".into(), json!(true))]),
            )
            .expect("validation event");
        analytics.shutdown();
    }
    #[test]
    fn analytics_excludes_content_and_unknown_events() {
        let event = FlexibleEvent {
            event_type: "Agent Panel Error Shown".into(),
            event_properties: std::collections::HashMap::from([
                ("kind".into(), json!("api_error")),
                ("message".into(), json!("private note and credential")),
                ("session_id".into(), json!("provider-conversation-id")),
                ("agent".into(), json!("custom agent containing a name")),
            ]),
        };
        let (_, properties) = sanitized_event(&event).expect("allowed event");
        assert_eq!(
            properties,
            Map::from_iter([("kind".into(), json!("api_error"))])
        );
        assert!(
            sanitized_event(&FlexibleEvent {
                event_type: "Settings Searched".into(),
                ..event
            })
            .is_none()
        );
    }

    #[test]
    fn analytics_accepts_only_typed_workflow_counts() {
        let event = FlexibleEvent {
            event_type: "Brainz MCP Selection Changed".into(),
            event_properties: std::collections::HashMap::from([
                ("selection".into(), json!("custom")),
                ("selected_count".into(), json!(2)),
                ("available_count".into(), json!("private server name")),
                ("servers".into(), json!(["private"])),
            ]),
        };
        let (_, properties) = sanitized_event(&event).expect("allowed event");
        assert_eq!(
            properties,
            Map::from_iter([
                ("selection".into(), json!("custom")),
                ("selected_count".into(), json!(2))
            ])
        );
    }
}
