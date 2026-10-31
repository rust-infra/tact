//! PromptBar's assembled controls and their typed events.
//!
//! The probe subscribes to the bar itself, so each assertion is about what a
//! reader can reach — an empty model catalog and a disabled submit stay visible
//! without becoming activatable, and every enabled control emits the event its
//! identity promises.

use gpui::{
    AppContext as _, Context, Entity, Modifiers, Render, Subscription, TestAppContext, Window,
};
use gpui_ai::{
    prompt_bar::{PromptBar, PromptBarEvent, PromptModel},
    stream::ProgressState,
};
use std::{cell::RefCell, rc::Rc};

struct PublicPromptProbe {
    prompt: Entity<PromptBar>,
    events: Rc<RefCell<Vec<PromptBarEvent>>>,
    _subscription: Subscription,
}

impl PublicPromptProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt = cx.new(|cx| PromptBar::new("public-prompt", window, cx));
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let _subscription = cx.subscribe(&prompt, move |_, _, event, _| {
            captured.borrow_mut().push(event.clone());
        });
        Self {
            prompt,
            events,
            _subscription,
        }
    }
}

impl Render for PublicPromptProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        self.prompt.clone()
    }
}

#[gpui::test]
fn public_prompt_bar_empty_catalog_and_disabled_submit_are_noninteractive(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicPromptProbe::new);
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("prompt-bar-model-empty").is_some());
    assert!(cx.debug_bounds("prompt-bar-model-trigger").is_none());
    let submit = cx
        .debug_bounds("prompt-bar-send-control")
        .expect("disabled submit should remain visible");
    cx.simulate_click(submit.center(), Modifiers::default());

    assert!(probe.read_with(cx, |probe, _| probe.events.borrow().is_empty()));
}

#[gpui::test]
fn public_prompt_bar_assembled_controls_activate_typed_events(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicPromptProbe::new);
    cx.update(|window, cx| {
        let prompt = probe.read(cx).prompt.clone();
        prompt.update(cx, |prompt, cx| {
            prompt.set_models(
                [
                    PromptModel::new("balanced", "Balanced"),
                    PromptModel::new("fast", "Fast"),
                ],
                cx,
            );
            prompt.set_draft("Summarize this", window, cx);
        });
        window.draw(cx).clear(cx);
    });

    assert!(cx.debug_bounds("prompt-bar-model-empty").is_none());
    let model_trigger = cx
        .debug_bounds("prompt-bar-model-trigger")
        .expect("configured model trigger should render");
    cx.simulate_click(model_trigger.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let fast_model = cx
        .debug_bounds("prompt-bar-model-option-fast")
        .expect("opened model menu should render the Fast option");
    cx.simulate_click(fast_model.center(), Modifiers::default());

    let attach = cx
        .debug_bounds("prompt-bar-attach-control")
        .expect("attach control should render");
    cx.simulate_click(attach.center(), Modifiers::default());
    let enhance = cx
        .debug_bounds("prompt-bar-enhance-control")
        .expect("enhance control should render");
    cx.simulate_click(enhance.center(), Modifiers::default());
    let submit = cx
        .debug_bounds("prompt-bar-send-control")
        .expect("enabled submit should remain visible");
    cx.simulate_click(submit.center(), Modifiers::default());

    cx.update(|window, cx| {
        let prompt = probe.read(cx).prompt.clone();
        prompt.update(cx, |prompt, cx| {
            prompt.set_progress(ProgressState::Running, cx)
        });
        window.draw(cx).clear(cx);
    });
    let cancel = cx
        .debug_bounds("prompt-bar-cancel-control")
        .expect("running prompt should render cancel");
    cx.simulate_click(cancel.center(), Modifiers::default());

    assert!(probe.read_with(cx, |probe, _| {
        let events = probe.events.borrow();
        assert!(events.iter().any(|event| matches!(
            event,
            PromptBarEvent::ModelChanged { id, model_id }
                if id == "public-prompt" && model_id == "fast"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            PromptBarEvent::AttachRequested { id } if id == "public-prompt"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            PromptBarEvent::EnhanceRequested { id, draft }
                if id == "public-prompt" && draft == "Summarize this"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            PromptBarEvent::Submit { id, submission }
                if id == "public-prompt"
                    && submission.text() == "Summarize this"
                    && submission.model_id() == Some(&"balanced".into())
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            PromptBarEvent::CancelRequested { id } if id == "public-prompt"
        )));
        true
    }));
}
