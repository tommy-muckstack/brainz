use std::{
    fs,
    io::Read as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};
use globset::GlobBuilder;
use serde::{Deserialize, Serialize};

use crate::{InsertMode, MAX_TEMPLATE_BYTES, Registry, relative_path, scoped_path, valid_id};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct TemplatesConfig {
    pub dir: Option<PathBuf>,
    pub daily: String,
    pub folder: Vec<FolderDefault>,
}

impl Default for TemplatesConfig {
    fn default() -> Self {
        Self {
            dir: None,
            daily: "daily".into(),
            folder: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FolderDefault {
    pub path: String,
    pub template: String,
    #[serde(default)]
    pub auto: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TemplateChoice {
    pub template: String,
    pub pattern: String,
    pub auto: bool,
}

impl TemplatesConfig {
    pub fn load(root: &Path) -> Result<Self> {
        #[derive(Default, Deserialize)]
        struct BrainConfig {
            #[serde(default)]
            templates: TemplatesConfig,
        }
        let path = scoped_path(root, Path::new("brainz.toml"))?;
        if !path.try_exists()? {
            return Ok(Self::default());
        }
        let mut content = String::new();
        fs::File::open(path)?
            .take(MAX_TEMPLATE_BYTES + 1)
            .read_to_string(&mut content)?;
        ensure!(
            content.len() as u64 <= MAX_TEMPLATE_BYTES,
            "Brain config is larger than 256 KB"
        );
        let config: BrainConfig = toml::from_str(&content).context("Invalid brainz.toml")?;
        let templates = config.templates;
        if let Some(directory) = &templates.dir {
            relative_path(&directory.to_string_lossy())?;
        }
        ensure!(valid_id(&templates.daily), "Invalid daily template id");
        ensure!(
            templates.folder.len() <= 256,
            "At most 256 template folder rules are supported"
        );
        Ok(templates)
    }
}

fn match_specificity(pattern: &str, path: &Path) -> Result<Option<(usize, usize)>> {
    ensure!(
        pattern.len() <= 1024,
        "Template folder pattern is longer than 1024 bytes"
    );
    relative_path(pattern)?;
    let matcher = GlobBuilder::new(pattern)
        .literal_separator(true)
        .backslash_escape(false)
        .build()
        .with_context(|| format!("Invalid template folder pattern: {pattern}"))?
        .compile_matcher();
    if !path
        .ancestors()
        .filter(|ancestor| !ancestor.as_os_str().is_empty())
        .any(|ancestor| matcher.is_match(ancestor))
    {
        return Ok(None);
    }
    let literal_components = pattern
        .split('/')
        .filter(|component| !component.contains(['*', '?', '[', '{']))
        .count();
    let literal_characters = pattern
        .chars()
        .filter(|character| !matches!(character, '*' | '?' | '[' | ']' | '{' | '}' | ','))
        .count();
    Ok(Some((literal_components, literal_characters)))
}

impl Registry {
    /// Explicit folder rules take priority over a template's `applies_to` globs.
    /// Ties prefer the earlier rule, then alphabetical template IDs.
    pub fn default_for_path(
        &self,
        config: &TemplatesConfig,
        path: &Path,
    ) -> Result<Option<TemplateChoice>> {
        let path = relative_path(&path.to_string_lossy())?;
        let mut selected = None;
        let mut best = None;
        for rule in &config.folder {
            let Some(specificity) = match_specificity(&rule.path, &path)? else {
                continue;
            };
            if best.is_some_and(|best| best >= specificity) {
                continue;
            }
            best = Some(specificity);
            selected = Some(TemplateChoice {
                template: rule.template.clone(),
                pattern: rule.path.clone(),
                auto: rule.auto,
            });
        }
        if let Some(choice) = &selected {
            let template = self.templates.get(&choice.template).with_context(|| {
                format!(
                    "Folder rule refers to missing template: {}",
                    choice.template
                )
            })?;
            ensure!(
                template.metadata.modes.contains(&InsertMode::New),
                "Folder template {} does not support new notes",
                choice.template
            );
            return Ok(selected);
        }
        for template in self
            .templates
            .values()
            .filter(|template| template.metadata.modes.contains(&InsertMode::New))
        {
            for pattern in &template.metadata.applies_to {
                let Some(specificity) = match_specificity(pattern, &path)? else {
                    continue;
                };
                if best.is_some_and(|best| best >= specificity) {
                    continue;
                }
                best = Some(specificity);
                selected = Some(TemplateChoice {
                    template: template.metadata.id.clone(),
                    pattern: pattern.clone(),
                    auto: false,
                });
            }
        }
        Ok(selected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_defaults_choose_specific_rules_and_preserve_suggestion_mode() {
        let mut registry = Registry::builtin().expect("built-ins");
        registry
            .templates
            .get_mut("decision")
            .expect("decision")
            .metadata
            .applies_to = vec!["**/decisions/**".into()];
        let config: TemplatesConfig = toml::from_str(
            r#"
            daily = "weekly-review"
            [[folder]]
            path = "projects/*"
            template = "project-kickoff"
            [[folder]]
            path = "projects/alpha/meetings/**"
            template = "meeting"
            auto = true
        "#,
        )
        .expect("config");
        for (path, expected, auto) in [
            ("projects/alpha/plan.md", "project-kickoff", false),
            ("projects/alpha/meetings/sync.md", "meeting", true),
            (
                "projects/alpha/decisions/choice.md",
                "project-kickoff",
                false,
            ),
            ("personal/decisions/choice.md", "decision", false),
        ] {
            let found = registry
                .default_for_path(&config, Path::new(path))
                .expect("match")
                .expect("default");
            assert_eq!(found.template, expected, "{path}");
            assert_eq!(found.auto, auto);
        }
        assert!(
            registry
                .default_for_path(&config, Path::new("notes.md"))
                .expect("match")
                .is_none()
        );
        assert!(
            registry
                .default_for_path(&config, Path::new("../outside.md"))
                .is_err()
        );
        let invalid = TemplatesConfig {
            folder: vec![FolderDefault {
                path: "[broken".into(),
                template: "daily".into(),
                auto: true,
            }],
            ..Default::default()
        };
        assert!(
            registry
                .default_for_path(&invalid, Path::new("notes.md"))
                .is_err()
        );
    }

    #[test]
    fn configured_daily_and_missing_template_errors_are_explicit() {
        let root = tempfile::tempdir().expect("temporary brain");
        fs::write(root.path().join("brainz.toml"), "[templates]\ndaily = 'weekly-review'\n[[templates.folder]]\npath = 'notes/**'\ntemplate = 'missing'\n").expect("config");
        let config = TemplatesConfig::load(root.path()).expect("load");
        assert_eq!(config.daily, "weekly-review");
        let registry = Registry::load_brain(root.path(), None).expect("registry remains available");
        assert!(
            registry
                .default_for_path(&config, Path::new("notes/new.md"))
                .expect_err("missing default")
                .to_string()
                .contains("missing")
        );
    }
}
