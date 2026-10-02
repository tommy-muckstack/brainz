//! Brainz: standing permission rules for Brainz's own Claude, so routine
//! work in the brain stops asking. Written only when the user clicks the
//! offer in the conversation; read-only checks otherwise.

use std::path::Path;

use anyhow::{Context as _, Result};

/// Type tag for notification ids.
pub struct Marker;

fn settings_path() -> std::path::PathBuf {
    paths::config_dir().join("claude").join("settings.json")
}

/// True when Brainz's Claude settings carry no `permissions` block yet.
pub fn needs_setup() -> bool {
    let Ok(text) = std::fs::read_to_string(settings_path()) else {
        return true;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .map(|value| value.get("permissions").is_none())
        .unwrap_or(false)
}

/// The rules Brainz proposes: edits inside the brain are accepted, reading
/// and searching anywhere under the home folder and git in the brain never
/// ask, the configured MCP connectors are allowed, and the destructive
/// shapes stay prompted or denied.
pub fn default_rules(brain: &Path, mcp_servers: &[String]) -> serde_json::Value {
    let brain = brain.to_string_lossy();
    let home = util::paths::home_dir().to_string_lossy().into_owned();
    let mut allow: Vec<String> = vec![
        format!("Read(//{}/**)", home.trim_start_matches('/')),
        format!("Edit(//{}/**)", brain.trim_start_matches('/')),
        format!("Write(//{}/**)", brain.trim_start_matches('/')),
    ];
    for command in [
        "git", "gh pr", "gh issue", "ls", "cat", "head", "tail", "wc", "grep", "rg", "find",
        "sed -n", "awk", "sort", "uniq", "mkdir", "mv", "cp", "touch", "date", "echo", "open",
        "python3", "jq",
    ] {
        allow.push(format!("Bash({command}:*)"));
    }
    allow.push("WebFetch".to_owned());
    allow.push("WebSearch".to_owned());
    for server in mcp_servers {
        allow.push(format!("mcp__{server}"));
    }
    serde_json::json!({
        "defaultMode": "acceptEdits",
        "allow": allow,
        "deny": ["Bash(rm -rf:*)", "Bash(git push --force:*)"],
    })
}

/// Merges the default rules into Brainz's Claude settings, keeping every
/// other key. Blocking file I/O: run on the background executor.
pub fn write_defaults(brain: &Path, mcp_servers: &[String]) -> Result<usize> {
    let path = settings_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_owned());
    let mut value: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let rules = default_rules(brain, mcp_servers);
    let count = rules["allow"].as_array().map(|a| a.len()).unwrap_or(0);
    let object = value
        .as_object_mut()
        .context("settings.json is not an object")?;
    object.insert("permissions".to_owned(), rules);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&value)? + "\n")
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_cover_the_brain_and_connectors() {
        let rules = default_rules(Path::new("/Users/ada/brain"), &["granola".into()]);
        assert_eq!(rules["defaultMode"], "acceptEdits");
        let allow: Vec<String> = rules["allow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert!(allow.contains(&"Edit(//Users/ada/brain/**)".to_owned()));
        assert!(allow.contains(&"Bash(git:*)".to_owned()));
        assert!(allow.contains(&"mcp__granola".to_owned()));
        assert!(!allow.iter().any(|rule| rule.contains("rm -rf")));
    }
}
