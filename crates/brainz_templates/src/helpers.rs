use crate::{AiBlock, RenderRequest, RenderState, relative_path, valid_id};
use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, TimeZone};
use handlebars::{
    Context, Handlebars, Helper, HelperDef, HelperResult, Output, RenderContext, RenderError,
    Renderable, ScopedJson,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub fn register(
    registry: &mut Handlebars<'_>,
    request: &RenderRequest,
    state: Arc<Mutex<RenderState>>,
    cursor: String,
    target: Option<PathBuf>,
) {
    for name in [
        "date",
        "slug",
        "lower",
        "upper",
        "default",
        "join",
        "bullets",
        "link",
        "wikilink",
        "plural",
        "trim",
        "indent",
        "json",
        "exists",
        "not",
        "cursor",
        "helperMissing",
    ] {
        registry.register_helper(
            name,
            Box::new(Scalar {
                name,
                now: request.now,
                state: state.clone(),
                cursor: cursor.clone(),
                target: target.clone(),
            }),
        );
    }
    registry.register_helper(
        "ai",
        Box::new(AiHelper {
            state,
            run_id: request.run_id,
        }),
    );
}

struct Scalar {
    name: &'static str,
    now: DateTime<FixedOffset>,
    state: Arc<Mutex<RenderState>>,
    cursor: String,
    target: Option<PathBuf>,
}

pub(crate) fn slug(value: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            if separator && !result.is_empty() {
                result.push('-');
            }
            result.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }
    result
}

fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        value => value.to_string(),
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.),
    }
}

impl HelperDef for Scalar {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        helper: &Helper<'reg, 'rc>,
        _: &'reg Handlebars<'reg>,
        context: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'reg, 'rc>, RenderError> {
        if !matches!(self.name, "default" | "exists" | "not") {
            let mut state = self
                .state
                .lock()
                .map_err(|_| RenderError::new("Template state is unavailable"))?;
            for parameter in helper.params() {
                if parameter.is_value_missing() {
                    if let Some(path) = parameter.relative_path() {
                        state.missing.insert(path.to_owned());
                    }
                }
            }
        }
        let first = helper
            .param(0)
            .map(|parameter| parameter.value())
            .unwrap_or(&Value::Null);
        let second = helper
            .param(1)
            .map(|parameter| parameter.value())
            .unwrap_or(&Value::Null);
        let first_text = text(first);
        let result = match self.name {
            "helperMissing" => {
                if !helper.params().is_empty() || helper.template().is_some() {
                    return Err(RenderError::new(format!(
                        "Unknown helper: {}",
                        helper.name()
                    )));
                }
                self.state
                    .lock()
                    .map_err(|_| RenderError::new("Template state is unavailable"))?
                    .missing
                    .insert(helper.name().to_owned());
                Value::Null
            }
            "cursor" => json!(self.cursor),
            "date" => {
                if helper.param(0).is_some_and(|parameter| {
                    parameter.is_value_missing() || parameter.value().is_null()
                }) {
                    Value::Null
                } else {
                    json!(date(self.now, &first_text, second.as_str())?)
                }
            }
            "slug" => {
                if helper.params().is_empty() {
                    context.data().get("slug").cloned().unwrap_or(Value::Null)
                } else {
                    json!(slug(&first_text))
                }
            }
            "lower" => json!(first_text.to_lowercase()),
            "upper" => json!(first_text.to_uppercase()),
            "trim" => json!(first_text.trim()),
            "default" => {
                if truthy(first) {
                    first.clone()
                } else {
                    second.clone()
                }
            }
            "exists" => json!(!first.is_null()),
            "not" => json!(!truthy(first)),
            "json" => json!(
                serde_json::to_string_pretty(first)
                    .map_err(|error| RenderError::new(error.to_string()))?
            ),
            "join" => json!(
                first
                    .as_array()
                    .map(|values| values
                        .iter()
                        .map(text)
                        .collect::<Vec<_>>()
                        .join(second.as_str().unwrap_or(", ")))
                    .unwrap_or_default()
            ),
            "bullets" => json!(
                first
                    .as_array()
                    .map(|values| values
                        .iter()
                        .map(|value| {
                            let value = second
                                .as_str()
                                .and_then(|field| crate::lookup(value, field))
                                .unwrap_or(value);
                            format!("- {}", text(value).replace('\n', "\n  "))
                        })
                        .collect::<Vec<_>>()
                        .join("\n"))
                    .unwrap_or_default()
            ),
            "plural" => json!(if first.as_i64() == Some(1) {
                text(second)
            } else {
                helper
                    .param(2)
                    .map(|value| text(value.value()))
                    .unwrap_or_else(|| format!("{}s", text(second)))
            }),
            "indent" => {
                let width = first.as_u64().unwrap_or(0).min(32) as usize;
                let prefix = " ".repeat(width);
                json!(
                    text(second)
                        .lines()
                        .map(|line| format!("{prefix}{line}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            }
            "wikilink" => {
                if first_text.contains(['[', ']', '\n', '\r']) {
                    return Err(RenderError::new("Invalid wiki link"));
                }
                json!(format!(
                    "[[{}]]",
                    first_text.strip_suffix(".md").unwrap_or(&first_text)
                ))
            }
            "link" => json!(relative_link(self.target.as_deref(), &first_text)?),
            _ => return Err(RenderError::new("Unknown template helper")),
        };
        Ok(ScopedJson::Derived(result))
    }
}

fn relative_link(
    target: Option<&std::path::Path>,
    destination: &str,
) -> Result<String, RenderError> {
    if destination.is_empty() {
        return Ok(String::new());
    }
    let destination =
        relative_path(destination).map_err(|error| RenderError::new(error.to_string()))?;
    let destination = destination
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let parent = target
        .and_then(std::path::Path::parent)
        .map(|path| {
            path.components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let common = parent
        .iter()
        .zip(&destination)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = vec!["..".to_owned(); parent.len() - common];
    parts.extend(destination.into_iter().skip(common));
    if parts.is_empty() {
        return Ok(".".into());
    }
    let mut encoded = String::new();
    for byte in parts.join("/").bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(encoded)
}

fn date(
    now: DateTime<FixedOffset>,
    value: &str,
    format: Option<&str>,
) -> Result<String, RenderError> {
    let (value, format) = if value.contains('%') && format.is_none() {
        ("", value)
    } else {
        (value, format.unwrap_or("%Y-%m-%d"))
    };
    if chrono::format::StrftimeItems::new(format).any(|item| item == chrono::format::Item::Error) {
        return Err(RenderError::new("Invalid date format"));
    }
    let date = if value.is_empty() {
        now
    } else if let Some(days) = value
        .strip_suffix('d')
        .filter(|_| value.starts_with(['+', '-']))
    {
        let days = days
            .parse::<i64>()
            .map_err(|_| RenderError::new("Invalid relative date"))?;
        if !(-36_500..=36_500).contains(&days) {
            return Err(RenderError::new("Relative date is out of range"));
        }
        now.checked_add_signed(Duration::days(days))
            .ok_or_else(|| RenderError::new("Date is out of range"))?
    } else if let Some(index) = [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ]
    .iter()
    .position(|day| *day == value.strip_prefix("next ").unwrap_or(value).to_lowercase())
    {
        let current = now.weekday().num_days_from_monday() as i64;
        let difference = if value.starts_with("next ") {
            let distance = (index as i64 - current).rem_euclid(7);
            if distance == 0 { 7 } else { distance }
        } else {
            -(current - index as i64).rem_euclid(7)
        };
        now.checked_add_signed(Duration::days(difference))
            .ok_or_else(|| RenderError::new("Date is out of range"))?
    } else if let Ok(date) = DateTime::parse_from_rfc3339(value) {
        date.with_timezone(now.offset())
    } else {
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map_err(|_| RenderError::new(format!("Invalid date: {value}")))?;
        let date = date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| RenderError::new("Invalid date"))?;
        now.offset()
            .from_local_datetime(&date)
            .single()
            .ok_or_else(|| RenderError::new("Invalid date"))?
    };
    Ok(date.format(format).to_string())
}

struct AiHelper {
    state: Arc<Mutex<RenderState>>,
    run_id: uuid::Uuid,
}

impl HelperDef for AiHelper {
    fn call<'reg: 'rc, 'rc>(
        &self,
        helper: &Helper<'reg, 'rc>,
        registry: &'reg Handlebars<'reg>,
        context: &'rc Context,
        render_context: &mut RenderContext<'reg, 'rc>,
        output: &mut dyn Output,
    ) -> HelperResult {
        let id = helper
            .param(0)
            .and_then(|value| value.value().as_str())
            .ok_or_else(|| RenderError::new("AI section needs an id"))?;
        if !valid_id(id) {
            return Err(RenderError::new("Invalid AI section id"));
        }
        let mut instruction = crate::BoundedOutput(String::new());
        if let Some(template) = helper.template() {
            template.render(registry, context, render_context, &mut instruction)?;
        }
        let instruction = instruction.0;
        let format = helper
            .hash_get("format")
            .and_then(|value| value.value().as_str())
            .unwrap_or("markdown");
        if !matches!(format, "markdown" | "bullets" | "json") {
            return Err(RenderError::new("Invalid AI section format"));
        }
        let words = helper
            .hash_get("words")
            .and_then(|value| value.value().as_u64())
            .unwrap_or(120)
            .clamp(1, 2000) as usize;
        let providers = helper
            .hash_get("context")
            .and_then(|value| value.value().as_str())
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect();
        let mut state = self
            .state
            .lock()
            .map_err(|_| RenderError::new("Template state is unavailable"))?;
        if state.ai_blocks.iter().any(|block| block.id == id) {
            return Err(RenderError::new("Duplicate AI section id"));
        }
        state.ai_blocks.push(AiBlock {
            id: id.into(),
            run_id: self.run_id,
            instruction: instruction.trim().into(),
            context: providers,
            words,
            format: format.into(),
        });
        output.write(&format!("<!-- brainz:ai id={id} run={} -->\n✦ AI draft: {id} · Set up a model\n<!-- /brainz:ai -->", self.run_id))?;
        Ok(())
    }
}
