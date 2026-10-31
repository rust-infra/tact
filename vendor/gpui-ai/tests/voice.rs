//! Voice controls: accessible naming per state, typed dictate / speak
//! events, keyboard reach, and the interim transcript as a status.

use gpui::{
    Context, Element as _, IntoElement as _, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers,
    ParentElement as _, Render, RenderOnce as _, Role, Styled as _, TestAppContext,
    VisualTestContext, Window, accesskit, canvas, div, px, size,
};
use gpui_ai::voice::{VoiceControls, VoiceEvent, VoiceState};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

struct CapturedNode {
    role: Option<Role>,
    node: accesskit::Node,
}

struct A11yProbe {
    state: VoiceState,
    captured: Arc<Mutex<Option<CapturedNode>>>,
}

impl Render for A11yProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let captured = self.captured.clone();
        let state = self.state;
        canvas(
            move |_, window, cx| {
                let mut node = accesskit::Node::new(Role::Unknown);
                let element = VoiceControls::new("voice", state)
                    .speakable(true)
                    .on_event(|_, _, _| {})
                    .render(window, cx)
                    .into_element();
                let role = element.a11y_role();
                element.write_a11y_info(&mut node);
                *captured.lock().expect("capture mutex should be available") =
                    Some(CapturedNode { role, node });
            },
            |_, _, _, _| {},
        )
    }
}

fn capture(state: VoiceState, cx: &mut TestAppContext) -> CapturedNode {
    cx.update(gpui_ai::init);
    let captured = Arc::new(Mutex::new(None));
    let (_, cx) = cx.add_window_view({
        let captured = captured.clone();
        move |_, _| A11yProbe { state, captured }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    captured
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("probe should capture its element")
}

#[gpui::test]
fn the_group_reads_its_state(cx: &mut TestAppContext) {
    let idle = capture(VoiceState::Idle, cx);
    assert_eq!(idle.role, Some(Role::Group));
    assert_eq!(idle.node.label(), Some("Voice controls, idle"));
    let listening = capture(VoiceState::Listening { level: 0.4 }, cx);
    assert_eq!(listening.node.label(), Some("Voice controls, listening"));
}

struct Probe {
    state: VoiceState,
    events: Rc<RefCell<Vec<VoiceEvent>>>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let events = self.events.clone();
        div().size_full().child(
            VoiceControls::new("voice", self.state)
                .transcript(if self.state.is_listening() {
                    "compare the three suppl"
                } else {
                    ""
                })
                .speakable(true)
                .on_event(move |event, _, _| events.borrow_mut().push(event.clone())),
        )
    }
}

fn harness(
    state: VoiceState,
    cx: &mut TestAppContext,
) -> (Rc<RefCell<Vec<VoiceEvent>>>, &mut VisualTestContext) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view({
        let events = events.clone();
        move |_, _| Probe { state, events }
    });
    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.simulate_resize(size(px(480.), px(240.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (events, cx)
}

fn activate_key(cx: &mut VisualTestContext, key: &str) {
    let keystroke = Keystroke::parse(key).expect("test key should parse");
    cx.simulate_event(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(KeyUpEvent { keystroke });
}

fn click_center(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} should render"));
    cx.simulate_click(bounds.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn idle_controls_start_dictation_and_speech(cx: &mut TestAppContext) {
    let (events, cx) = harness(VoiceState::Idle, cx);
    assert!(cx.debug_bounds("voice-cancel-voice").is_none());
    assert!(cx.debug_bounds("voice-transcript-voice").is_none());
    click_center(cx, "voice-dictate-voice");
    click_center(cx, "voice-speak-voice");
    assert_eq!(
        events.borrow().as_slice(),
        &[VoiceEvent::DictationStarted, VoiceEvent::SpeakRequested]
    );
}

#[gpui::test]
fn listening_controls_stop_or_cancel_and_show_the_transcript(cx: &mut TestAppContext) {
    let (events, cx) = harness(VoiceState::Listening { level: 0.7 }, cx);
    assert!(cx.debug_bounds("voice-transcript-voice").is_some());
    click_center(cx, "voice-dictate-voice");
    click_center(cx, "voice-cancel-voice");
    assert_eq!(
        events.borrow().as_slice(),
        &[VoiceEvent::DictationStopped, VoiceEvent::DictationCancelled]
    );
}

#[gpui::test]
fn speaking_offers_stop_and_transcribing_disables_dictation(cx: &mut TestAppContext) {
    let (events, cx) = harness(VoiceState::Speaking, cx);
    click_center(cx, "voice-speak-voice");
    assert_eq!(events.borrow().as_slice(), &[VoiceEvent::SpeakStopped]);

    let (events, cx) = harness(VoiceState::Transcribing, cx);
    click_center(cx, "voice-dictate-voice");
    assert!(
        events.borrow().is_empty(),
        "transcribing has nothing to toggle"
    );
}

#[gpui::test]
fn keyboard_reaches_dictation_first(cx: &mut TestAppContext) {
    let (events, cx) = harness(VoiceState::Idle, cx);
    cx.update(|window, cx| window.focus_next(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    activate_key(cx, "enter");
    assert_eq!(events.borrow().as_slice(), &[VoiceEvent::DictationStarted]);
}
