//! Brainz: standing permission rules for Brainz's own Claude, so routine
//! work in the brain stops asking. Written only when the user clicks the
//! offer in the conversation; read-only checks otherwise.

use std::{io::Write as _, path::Path};

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
    write_defaults_at(&settings_path(), default_rules(brain, mcp_servers))
}

fn write_defaults_at(path: &Path, rules: serde_json::Value) -> Result<usize> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "{}".to_owned(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let mut value: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let object = value
        .as_object_mut()
        .context("settings.json is not an object")?;
    let permissions = object
        .entry("permissions")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("permissions is not an object")?;
    for (key, proposed) in rules
        .as_object()
        .context("permission defaults are not an object")?
    {
        if matches!(key.as_str(), "allow" | "deny" | "ask") {
            let saved = permissions
                .entry(key.clone())
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .with_context(|| format!("permissions.{key} is not an array"))?;
            for rule in proposed
                .as_array()
                .context("permission rules are not an array")?
            {
                if !saved.contains(rule) {
                    saved.push(rule.clone());
                }
            }
        } else {
            permissions
                .entry(key.clone())
                .or_insert_with(|| proposed.clone());
        }
    }
    let count = permissions
        .get("allow")
        .and_then(|allow| allow.as_array())
        .map_or(0, Vec::len);
    let parent = path
        .parent()
        .context("settings file has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    // A failed write must leave the previously approved rules intact.
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all((serde_json::to_string_pretty(&value)? + "\n").as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_permissions_survive_reopening_and_repeated_setup() {
        let directory = tempfile::tempdir().expect("settings directory");
        let path = directory.path().join("settings.json");
        let saved = serde_json::json!({
            "theme": "dark",
            "permissions": {
                "defaultMode": "default",
                "allow": ["mcp__myman__get_meeting", "Bash(git status:*)"],
                "deny": ["Bash(rm:*)"],
                "ask": ["WebFetch(domain:private.example)"],
                "additionalDirectories": ["/other/brain"]
            }
        });
        std::fs::write(&path, serde_json::to_vec(&saved).expect("serialize"))
            .expect("write settings");
        let defaults = default_rules(Path::new("/Users/ada/brain"), &["granola".into()]);
        write_defaults_at(&path, defaults.clone()).expect("merge defaults");
        let first = std::fs::read(&path).expect("reopen settings");
        write_defaults_at(&path, defaults).expect("repeat setup");
        assert_eq!(first, std::fs::read(&path).expect("reopen again"));
        let loaded: serde_json::Value = serde_json::from_slice(&first).expect("parse settings");
        assert_eq!(loaded["theme"], saved["theme"]);
        for key in ["defaultMode", "ask", "additionalDirectories"] {
            assert_eq!(loaded["permissions"][key], saved["permissions"][key]);
        }
        for key in ["allow", "deny"] {
            for rule in saved["permissions"][key].as_array().expect("saved rules") {
                assert!(
                    loaded["permissions"][key]
                        .as_array()
                        .expect("loaded rules")
                        .contains(rule)
                );
            }
        }
    }

    #[test]
    fn invalid_saved_permissions_are_never_overwritten() {
        let directory = tempfile::tempdir().expect("settings directory");
        let path = directory.path().join("settings.json");
        for saved in [
            "invalid JSON",
            r#"{"permissions":{"allow":"invalid"}}"#,
            r#"{"permissions":null}"#,
        ] {
            std::fs::write(&path, saved).expect("write settings");
            assert!(write_defaults_at(&path, default_rules(Path::new("/brain"), &[])).is_err());
            assert_eq!(
                std::fs::read_to_string(&path).expect("settings retained"),
                saved
            );
        }
    }

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
