use crate::{FeatureFlag, PresenceFlag, register_feature_flag};
use gpui::App;
use settings::{BrainzUpdateChannel, RegisterSetting, Settings, SettingsContent};

pub struct BrainzFlagDescription {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub introduced: &'static str,
}

macro_rules! brainz_flags {
    ($($type:ident => ($name:literal, $title:literal, $description:literal)),* $(,)?) => {
        $(
            pub struct $type;
            impl FeatureFlag for $type {
                const NAME: &'static str = $name;
                type Value = PresenceFlag;
                fn enabled_for_staff() -> bool { false }
            }
            register_feature_flag!($type);
        )*
        pub const BRAINZ_FLAGS: &[BrainzFlagDescription] = &[
            $(BrainzFlagDescription { name: $name, title: $title, description: $description, introduced: "0.2" }),*
        ];
    };
}

brainz_flags! {
    BrainzCanvasStorageV2 => ("brainz-canvas-storage-v2", "Incremental canvas storage", "Save small canvas edits efficiently and keep images in separate local files. Older canvases are backed up before upgrading."),
    BrainzCanvasAgentApi => ("brainz-canvas-agent-api", "Canvas assistant", "Preview and apply structured canvas changes from your assistant."),
    BrainzTemplates => ("brainz-templates", "Templates", "Create notes and canvases from reusable templates."),
    BrainzSkills => ("brainz-skills", "Skills", "Find and run personal workflows with reviewable outputs."),
    BrainzSearch => ("brainz-search", "Semantic search", "Search the meaning and text of your local notes and canvases."),
    BrainzRelatedPanel => ("brainz-related-panel", "Related content", "Find relevant material alongside the item you are viewing."),
    BrainzAutoAttach => ("brainz-auto-attach", "Suggested chat context", "Offer relevant sources for new chats. Automatic attachment still requires a separate opt-in."),
}

#[derive(Clone, Copy, Debug, RegisterSetting)]
struct BrainzFlagSettings(BrainzUpdateChannel);

impl Settings for BrainzFlagSettings {
    fn from_settings(content: &SettingsContent) -> Self {
        Self(
            content
                .brainz
                .as_ref()
                .and_then(|brainz| brainz.update_channel)
                .unwrap_or_default(),
        )
    }
}

pub(crate) fn brainz_flag_key(name: &str, cx: &App) -> &'static str {
    match crate::FeatureFlagStore::override_for(name, cx) {
        Some("on") => "on",
        Some("off") => "off",
        _ if BrainzFlagSettings::get_global(cx).0 == BrainzUpdateChannel::Preview => "on",
        _ => "off",
    }
}
