use std::sync::Arc;

use gpui::{Entity, ScrollHandle, Subscription, Task};
use language_model::{
    ANTHROPIC_PROVIDER_ID, AuthenticateError, IconOrSvg, LanguageModelProvider,
    LanguageModelRegistry, OPEN_AI_PROVIDER_ID, ProviderSettingsView,
};
use ui::{ButtonLink, prelude::*};
use ui_input::InputField;
use util::ResultExt as _;

use crate::SettingsWindow;

pub(crate) fn render_api_keys_page(
    _settings_window: &SettingsWindow,
    scroll_handle: &ScrollHandle,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let providers = [OPEN_AI_PROVIDER_ID, ANTHROPIC_PROVIDER_ID]
        .iter()
        .filter_map(|id| LanguageModelRegistry::read_global(cx).provider(id))
        .collect::<Vec<_>>();
    let forms = providers
        .into_iter()
        .map(|provider| {
            let id = format!("optional-api-key-{}", provider.id().0);
            window.use_keyed_state(id, cx, move |window, cx| {
                ApiKeyEditor::new(provider, window, cx)
            })
        })
        .collect::<Vec<_>>();

    v_flex()
        .id("optional-api-keys-page")
        .size_full()
        .px_8()
        .pt_4()
        .pb_16()
        .gap_6()
        .track_scroll(scroll_handle)
        .overflow_y_scroll()
        .child(
            v_flex()
                .gap_2()
                .child(Label::new("Brainz works without API keys."))
                .child(
                    Label::new("With a key, Brief, Themes, and To-Do automatically add AI insights using a snapshot of their data. Claude is preferred when both keys are available. Your chat subscription sign-ins stay separate.")
                        .color(Color::Muted)
                        .size(LabelSize::Small),
                )
                .child(
                    Label::new("Keys are stored in your system keychain, never in your notes or settings files. API usage is billed by the provider.")
                        .color(Color::Muted)
                        .size(LabelSize::Small),
                ),
        )
        .children(forms)
        .into_any_element()
}

struct ApiKeyEditor {
    provider: Arc<dyn LanguageModelProvider>,
    input: Entity<InputField>,
    loading: bool,
    saving: bool,
    error: Option<SharedString>,
    _input_subscription: Subscription,
    _registry_subscription: Subscription,
    _task: Option<Task<()>>,
}

impl ApiKeyEditor {
    fn new(
        provider: Arc<dyn LanguageModelProvider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (input, input_subscription) = Self::new_input(window, cx);
        let registry_subscription = cx
            .subscribe(&LanguageModelRegistry::global(cx), |_, _, _, cx| {
                cx.notify()
            });
        let authenticate = provider.authenticate(cx);
        let task = cx.spawn(async move |this, cx| {
            let result = authenticate.await;
            this.update(cx, |this, cx| {
                this.loading = false;
                if let Err(error) = result
                    && !matches!(error, AuthenticateError::CredentialsNotFound)
                {
                    this.error = Some("Couldn't read the saved key. Check your system keychain access and try again.".into());
                }
                cx.notify();
            }).log_err();
        });

        Self {
            provider,
            input,
            loading: true,
            saving: false,
            error: None,
            _input_subscription: input_subscription,
            _registry_subscription: registry_subscription,
            _task: Some(task),
        }
    }

    fn new_input(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputField>, Subscription) {
        let input = cx.new(|cx| {
            InputField::new(window, cx, "Paste an API key")
                .masked(true)
                .tab_index(0isize)
        });
        let weak_view = cx.weak_entity();
        let editor = input.read(cx).editor().clone();
        let input_subscription = editor.subscribe(
            Box::new(move |_, _, cx| {
                weak_view.update(cx, |_, cx| cx.notify()).log_err();
            }),
            window,
            cx,
        );
        (input, input_subscription)
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.input.read(cx).text(cx).trim().to_owned();
        if key.is_empty() {
            return;
        }
        self.store(Some(key), window, cx);
    }

    fn store(&mut self, key: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.saving {
            return;
        }
        let removing = key.is_none();
        self.saving = true;
        self.error = None;
        let task = self.provider.set_api_key(key, cx);
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(()) => {
                        let (input, subscription) = Self::new_input(window, cx);
                        this.input = input;
                        this._input_subscription = subscription;
                    }
                    Err(_) => {
                        this.error = Some(if removing {
                            "Couldn't remove the key. Check your system keychain access and try again."
                        } else {
                            "Couldn't save the key. Check your system keychain access and try again."
                        }.into());
                    }
                }
                cx.notify();
            }).log_err();
        }));
        cx.notify();
    }
}

impl Render for ApiKeyEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ProviderSettingsView::ApiKey(config)) = self.provider.settings_view(cx) else {
            return div().into_any_element();
        };
        let name = if self.provider.id() == ANTHROPIC_PROVIDER_ID {
            "Claude (Anthropic)"
        } else {
            "OpenAI"
        };
        let icon = match self.provider.icon() {
            IconOrSvg::Icon(icon) => Icon::new(icon),
            IconOrSvg::Svg(path) => Icon::from_external_svg(path),
        };
        let disabled = self.loading || self.saving || config.is_from_env_var;
        self.input
            .update(cx, |input, cx| input.editor().set_read_only(disabled, cx));
        let status = if self.loading {
            "Checking keychain…"
        } else if config.is_from_env_var {
            "Key supplied by an environment variable"
        } else if config.has_key {
            "API key saved in your keychain"
        } else {
            "No API key saved · optional"
        };

        v_flex()
            .gap_3()
            .child(h_flex().gap_2().child(icon).child(Label::new(name)))
            .child(
                Label::new(status)
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .when(!config.is_from_env_var, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .id("api-key-input")
                                .flex_1()
                                .role(gpui::Role::PasswordInput)
                                .aria_label(format!("{name} API key"))
                                .child(self.input.clone()),
                        )
                        .child(
                            Button::new(
                                "save-api-key",
                                if self.saving {
                                    "Saving…"
                                } else if config.has_key {
                                    "Replace Key"
                                } else {
                                    "Save Key"
                                },
                            )
                            .style(ButtonStyle::Filled)
                            .tab_index(0isize)
                            .disabled(disabled || self.input.read(cx).is_empty(cx))
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                        )
                        .when(config.has_key, |this| {
                            this.child(
                                Button::new("remove-api-key", "Remove")
                                    .tab_index(0isize)
                                    .disabled(disabled)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.store(None, window, cx)
                                    })),
                            )
                        }),
                )
            })
            .when(config.is_from_env_var, |this| {
                this.child(
                    Label::new(format!(
                        "Manage {} in your environment.",
                        config.env_var_name
                    ))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(Label::new(error).size(LabelSize::Small).color(Color::Error))
            })
            .child(
                ButtonLink::new(format!("Get a {name} API key"), config.api_key_url)
                    .label_size(LabelSize::Small),
            )
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| this.save(window, cx)))
            .into_any_element()
    }
}
