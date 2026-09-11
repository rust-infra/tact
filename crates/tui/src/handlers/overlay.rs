//! Shared keyboard handling for scrollable overlay popups
//! (thinking / tool-diff / code).

use crossterm::event::{KeyCode, KeyEvent};

use crate::widgets::state::App;

/// Handle a key while an overlay popup is open.
/// Returns `true` if an overlay was active (key is consumed either way).
///
/// Modal input modes ([`InputMode::Select`], FilePicker, Palette) take priority —
/// otherwise ↑/↓/j/k would scroll the overlay instead of moving the selection.
pub(crate) fn handle_overlay_key(app: &mut App, key: KeyEvent) -> bool {
    use crate::widgets::state::InputMode;
    if matches!(
        app.input_mode,
        InputMode::Select | InputMode::FilePicker | InputMode::Palette
    ) {
        return false;
    }
    if !app.has_overlay_popup() {
        return false;
    }

    match key.code {
        KeyCode::Esc => app.close_overlay_popup(),
        KeyCode::Char('y') => app.copy_overlay_popup(),
        KeyCode::Tab if app.mermaid_popup.is_some() => app.toggle_mermaid_popup_view(),
        KeyCode::Char('j') | KeyCode::Down => app.overlay_popup_scroll_down(),
        KeyCode::Char('k') | KeyCode::Up => app.overlay_popup_scroll_up(),
        KeyCode::Char('G')
            if app.code_popup.is_some()
                || app.mermaid_popup.is_some()
                || app.task_dag_popup.is_some()
                || app.has_subagent_popup() =>
        {
            if let Some(ref mut p) = app.code_popup {
                p.scroll = u16::MAX;
            }
            if let Some(ref mut p) = app.mermaid_popup {
                p.scroll = u16::MAX;
            }
            if let Some(ref mut p) = app.task_dag_popup {
                p.scroll = u16::MAX;
            }
            if let Some(ref mut p) = app.subagent_popup_mut() {
                p.scroll = u16::MAX;
            }
        }
        KeyCode::Char('g')
            if app.code_popup.is_some()
                || app.mermaid_popup.is_some()
                || app.task_dag_popup.is_some()
                || app.has_subagent_popup() =>
        {
            if let Some(ref mut p) = app.code_popup {
                p.scroll = 0;
            }
            if let Some(ref mut p) = app.mermaid_popup {
                p.scroll = 0;
            }
            if let Some(ref mut p) = app.task_dag_popup {
                p.scroll = 0;
            }
            if let Some(ref mut p) = app.subagent_popup_mut() {
                p.scroll = 0;
            }
        }
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::{
        render::test_harness::make_app,
        widgets::state::{CodePopup, DiffPopup, ThinkingPopup},
    };

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn returns_false_when_no_overlay() {
        let mut app = make_app();
        assert!(!handle_overlay_key(&mut app, key(KeyCode::Esc)));
    }

    #[test]
    fn yields_to_select_mode_even_with_overlay() {
        use crate::widgets::state::InputMode;
        let mut app = make_app();
        app.thinking_mut().popup = Some(ThinkingPopup {
            phys_idx: 0,
            title: "t".into(),
            scroll: 0,
            selection: None,
            selection_text: String::new(),
        });
        app.input_mode = InputMode::Select;
        assert!(
            !handle_overlay_key(&mut app, key(KeyCode::Down)),
            "Select mode must receive ↑↓, not the overlay"
        );
    }

    #[test]
    fn esc_closes_diff_popup() {
        let mut app = make_app();
        app.tools_mut().popup = Some(DiffPopup {
            title: "t".into(),
            file_path: None,
            git_diff_path: None,
            workspace_dir: None,
            inline_content: Some("x".into()),
            lang: String::new(),
            use_diff_gutter: false,
            is_diff: false,
            scroll: 0,
            selection: None,
            cached_content: None,
            highlighted_lines: Vec::new(),
        });
        assert!(handle_overlay_key(&mut app, key(KeyCode::Esc)));
        assert!(app.tools_mut().popup.is_none());
    }

    #[test]
    fn j_scrolls_thinking_popup() {
        let mut app = make_app();
        app.thinking_mut().popup = Some(ThinkingPopup {
            phys_idx: 0,
            title: "t".into(),
            scroll: 0,
            selection: None,
            selection_text: String::new(),
        });
        assert!(handle_overlay_key(&mut app, key(KeyCode::Char('j'))));
        assert_eq!(app.thinking_mut().popup.as_ref().unwrap().scroll, 1);
    }

    #[test]
    fn g_jumps_code_popup_to_top() {
        let mut app = make_app();
        app.code_popup = Some(CodePopup {
            block_idx: 0,
            lang: "rs".into(),
            scroll: 10,
        });
        assert!(handle_overlay_key(&mut app, key(KeyCode::Char('g'))));
        assert_eq!(app.code_popup.as_ref().unwrap().scroll, 0);
    }
}

#[cfg(test)]
mod mermaid_view_tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::render::test_harness::make_app;
    use agent_tui_kit::state::MermaidPopupView;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn tab_toggles_mermaid_popup_between_diagram_and_source() {
        let mut app = make_app();
        app.open_mermaid_popup(0); // no blocks registered → no popup
        assert!(app.mermaid_popup.is_none());

        app.mermaid_blocks
            .push(crate::widgets::state::MermaidBlock {
                start_idx: 0,
                end_idx: 1,
                source: "sequenceDiagram\n  Alice->>Bob: Hello".into(),
            });
        app.open_mermaid_popup(0);
        assert_eq!(
            app.mermaid_popup.as_ref().unwrap().view,
            MermaidPopupView::Diagram
        );

        assert!(handle_overlay_key(&mut app, key(KeyCode::Tab)));
        assert_eq!(
            app.mermaid_popup.as_ref().unwrap().view,
            MermaidPopupView::Source
        );

        assert!(handle_overlay_key(&mut app, key(KeyCode::Tab)));
        assert_eq!(
            app.mermaid_popup.as_ref().unwrap().view,
            MermaidPopupView::Diagram
        );
    }

    #[test]
    fn tab_is_ignored_when_no_mermaid_popup_is_open() {
        let mut app = make_app();
        app.thinking_mut().popup = Some(crate::widgets::state::ThinkingPopup {
            phys_idx: 0,
            title: "t".into(),
            scroll: 3,
            selection: None,
            selection_text: String::new(),
        });
        assert!(handle_overlay_key(&mut app, key(KeyCode::Tab)));
        assert_eq!(
            app.thinking_mut().popup.as_ref().unwrap().scroll,
            3,
            "Tab must not disturb other overlays"
        );
    }
}
