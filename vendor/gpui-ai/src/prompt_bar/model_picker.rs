//! Model-menu navigation, placement, and option presentation.

use super::*;

pub(super) fn prompt_model_control(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    expanded: bool,
    cx: &mut App,
) -> Button {
    let tokens = cx.theme().semantic_tokens();
    let label = label.into();
    let visible = label
        .strip_prefix("Model: ")
        .map(|visible| SharedString::from(visible.to_owned()))
        .unwrap_or_else(|| label.clone());
    prompt_option(
        id,
        label,
        h_flex()
            .items_center()
            .gap(tokens.spacing.xs)
            .child(
                Icon::new(IconName::Cpu)
                    .xsmall()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(visible)
            .child(
                Icon::new(if expanded {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .xsmall()
                .text_color(cx.theme().muted_foreground),
            ),
        cx,
    )
    .selected(expanded)
    .aria_expanded(expanded)
}

/// A menu row with custom content and the same geometry as [`prompt_control`].
pub(super) fn prompt_option(
    id: impl Into<ElementId>,
    accessibility_label: impl Into<SharedString>,
    content: impl IntoElement,
    cx: &mut App,
) -> Button {
    prompt_option_bare(id, accessibility_label, content, cx)
        .hover(|style| style.bg(cx.theme().button_hover))
        .active(|style| style.bg(cx.theme().button_active))
}

/// [`prompt_option`] without hover and press styles, for rows whose
/// states come from the shared selection surface — GPUI allows each
/// state style to be set exactly once.
pub(super) fn prompt_option_bare(
    id: impl Into<ElementId>,
    accessibility_label: impl Into<SharedString>,
    content: impl IntoElement,
    cx: &mut App,
) -> Button {
    let tokens = cx.theme().semantic_tokens();
    composed_button(id, accessibility_label)
        .flex()
        .items_center()
        .justify_start()
        .px(tokens.spacing.sm)
        .py(tokens.spacing.xs)
        .border_1()
        .border_color(cx.theme().transparent)
        .rounded(tokens.radius.sm)
        .bg(cx.theme().transparent)
        .text_token(tokens.typography.sm)
        .text_color(cx.theme().foreground)
        .focus_visible(|style| style.border_color(cx.theme().ring))
        .styles(|styles| {
            styles.disabled(|style| {
                style
                    .bg(cx.theme().muted)
                    .text_color(cx.theme().muted_foreground)
            })
        })
        .child(content)
}

pub(super) fn apply_model_option_state(
    button: Button,
    selected: bool,
    active: bool,
    position: usize,
    set_size: usize,
    glide: Option<(&SharedString, &gpui::Entity<crate::glide::GlideHover>)>,
    cx: &App,
) -> Button {
    // The keyboard cursor rides the shared selection surface: the
    // list-active fill at the nesting-rule radius against the popup's
    // frame. `selected` stays the semantic choice (checkmark + aria),
    // `active` the visual cursor. With the gliding highlight on, hover
    // is the one highlight element and rows paint none of their own.
    let tokens = cx.theme().semantic_tokens();
    let button = button
        .selected(selected)
        .aria_selected(selected)
        .aria_position_in_set(position)
        .aria_size_of_set(set_size)
        .active(|style| style.bg(cx.theme().list_active))
        .when(active, |button| button.aria_active_descendant());
    crate::surface::selection_surface(
        button,
        active,
        cx.theme().radius,
        tokens.spacing.xs,
        glide,
        cx,
    )
}

/// Models grouped by provider in first-appearance order; ungrouped models
/// keep their place under a `None` heading.
pub(super) fn model_groups(
    models: &[PromptModel],
) -> Vec<(Option<SharedString>, Vec<&PromptModel>)> {
    let mut groups: Vec<(Option<SharedString>, Vec<&PromptModel>)> = Vec::new();
    for model in models {
        match groups
            .iter_mut()
            .find(|(provider, _)| *provider == model.provider)
        {
            Some((_, members)) => members.push(model),
            None => groups.push((model.provider.clone(), vec![model])),
        }
    }
    groups
}

pub(super) fn retain_active_model(
    previous: Option<SharedString>,
    selected: Option<&SharedString>,
    models: &[PromptModel],
) -> Option<SharedString> {
    previous
        .filter(|candidate| {
            models
                .iter()
                .any(|model| &model.id == candidate && !model.disabled)
        })
        .or_else(|| {
            selected
                .filter(|selected| {
                    models
                        .iter()
                        .any(|model| &model.id == *selected && !model.disabled)
                })
                .cloned()
        })
        .or_else(|| {
            models
                .iter()
                .find(|model| !model.disabled)
                .map(|model| model.id.clone())
        })
}

impl PromptBar {
    pub(super) fn toggle_model_menu(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.models.is_empty() {
            return;
        }
        self.model_menu_open = !self.model_menu_open;
        if self.model_menu_open {
            self.active_model = retain_active_model(
                self.active_model.take(),
                self.selected_model.as_ref(),
                &self.models,
            );
        }
        self.focus(window, cx);
        cx.notify();
    }

    pub(super) fn close_model_menu(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if !self.model_menu_open {
            return;
        }
        self.model_menu_open = false;
        self.focus(window, cx);
        cx.notify();
    }

    pub(super) fn move_active_model(&mut self, direction: isize, cx: &mut gpui::Context<Self>) {
        let enabled_models: Vec<_> = self.models.iter().filter(|model| !model.disabled).collect();
        if enabled_models.is_empty() {
            return;
        }
        let current_ix = self
            .active_model
            .as_ref()
            .and_then(|active| enabled_models.iter().position(|model| &model.id == active))
            .unwrap_or(0);
        let next_ix = if direction < 0 {
            current_ix
                .checked_sub(1)
                .unwrap_or(enabled_models.len() - 1)
        } else {
            (current_ix + 1) % enabled_models.len()
        };
        self.active_model = enabled_models.get(next_ix).map(|model| model.id.clone());
        cx.notify();
    }

    pub(super) fn move_active_model_to_edge(&mut self, end: bool, cx: &mut gpui::Context<Self>) {
        self.active_model = if end {
            self.models
                .iter()
                .rev()
                .find(|model| !model.disabled)
                .map(|model| model.id.clone())
        } else {
            self.models
                .iter()
                .find(|model| !model.disabled)
                .map(|model| model.id.clone())
        };
        cx.notify();
    }

    pub(super) fn confirm_active_model(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(model_id) = self.active_model.clone().filter(|active| {
            self.models
                .iter()
                .any(|model| &model.id == active && !model.disabled)
        }) else {
            return;
        };
        self.model_menu_open = false;
        cx.emit(PromptBarEvent::ModelChanged {
            id: self.id.clone(),
            model_id,
        });
        self.focus(window, cx);
        cx.notify();
    }

    pub(super) fn render_model_picker(
        &self,
        root_id: &SharedString,
        label: SharedString,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let tokens = cx.theme().semantic_tokens();
        let glide = crate::glide::glide_hover_state(
            (gpui::ElementId::from(root_id.clone()), "model-glide").into(),
            window,
            cx,
        );
        let model_count = self.models.len();
        let mut model_ix = 0;
        let mut model_options = Vec::new();
        for (provider, members) in model_groups(&self.models) {
            if let Some(provider) = provider {
                model_options.push(
                    eyebrow(provider.clone(), cx)
                        .px(tokens.spacing.sm)
                        .pt(tokens.spacing.xs)
                        .into_any_element(),
                );
            }
            for model in members {
                model_ix += 1;
                let model_id = model.id.clone();
                let model_debug = model.id.clone();
                let selected = self.selected_model.as_ref() == Some(&model.id);
                let active = self.active_model.as_ref() == Some(&model.id);
                let content = h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(tokens.spacing.sm)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .items_start()
                            .child(div().child(model.label.clone()))
                            .when_some(model.description.clone(), |this, description| {
                                this.child(
                                    div()
                                        .w_full()
                                        .truncate()
                                        .text_token(tokens.typography.xs)
                                        .text_color(cx.theme().muted_foreground)
                                        .child(description),
                                )
                            }),
                    )
                    .when_some(model.context_window, |this, window_tokens| {
                        this.child(
                            meta(format!("{} ctx", format_tokens(window_tokens)), cx).flex_none(),
                        )
                    })
                    .when(selected, |this| {
                        this.child(
                            Icon::new(IconName::Check)
                                .xsmall()
                                .text_color(cx.theme().primary),
                        )
                    });
                let option = prompt_option_bare(
                    (
                        gpui::ElementId::from(root_id.clone()),
                        format!("model-{}", model.id),
                    ),
                    model.label.clone(),
                    content,
                    cx,
                )
                .when_some(model.description.clone(), |button, description| {
                    button.aria_description(description)
                })
                .debug_selector(move || format!("prompt-bar-model-option-{model_debug}"))
                .role(Role::ListBoxOption)
                .disabled(model.disabled);
                model_options.push(
                    apply_model_option_state(
                        option,
                        selected,
                        active,
                        model_ix,
                        model_count,
                        Some((&model.id, &glide)),
                        cx,
                    )
                    .w_full()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.active_model = Some(model_id.clone());
                        this.confirm_active_model(window, cx);
                    }))
                    .into_any_element(),
                );
            }
        }

        let surface = prompt_listbox(
            (gpui::ElementId::from(root_id.clone()), "models").into(),
            label,
        )
        .debug_selector(|| "prompt-bar-model-picker".to_owned())
        .occlude()
        .w(tokens.spacing.xxl * 10.0)
        .max_h(tokens.spacing.xxl * 7.0)
        .overflow_y_scrollbar()
        .p(tokens.spacing.xs)
        .relative()
        .on_mouse_down_out(cx.listener(|this, _, window, cx| {
            this.close_model_menu(window, cx);
        }));
        let surface = crate::popup::popover_surface(surface, cx);
        let surface = crate::glide::glide_frame(surface, &glide)
            .child(crate::glide::glide_highlight(
                (gpui::ElementId::from(root_id.clone()), "model-glide").into(),
                &glide,
                crate::surface::nested_radius(
                    cx.theme().radius,
                    tokens.spacing.xs,
                    tokens.radius.sm,
                ),
                "prompt-bar-model-glide",
                window,
                cx,
            ))
            .children(model_options);

        // The side comes from the crate's popup policy; the positioner
        // still flips and clamps when the chosen side does not fit, so a
        // composer at the foot of a window opens its menu upward.
        let placement = crate::popup::PopupTokens::read(cx).side().placement(
            self.model_trigger_bounds.top(),
            window.viewport_size().height,
        );
        deferred(
            Positioner::side(self.model_trigger_bounds)
                .placement(placement)
                .align(Align::Start)
                .offset(tokens.spacing.xs)
                .child(surface),
        )
        .with_priority(POPUP_PRIORITY)
        .into_any_element()
    }
}
