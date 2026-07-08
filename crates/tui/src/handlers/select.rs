use crate::widgets::state::{App, InputMode};
use crossterm::event::{KeyCode, KeyEvent};

/// Select popup mode key handling: up/down to navigate, Enter to confirm, Esc to cancel.
pub(crate) fn handle_select_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            if app.select.options.is_empty() {
                let msgs = app.msgs();
                app.add_system_message(msgs.no_options.to_string());
            } else {
                let log_confirm = app.select.log_confirm;
                let idx = app.select.confirm().unwrap_or(0);
                if log_confirm {
                    let msgs = app.msgs();
                    app.add_system_message(
                        msgs.selected_tmpl.replace(
                            "{}",
                            &app.select
                                .options
                                .get(idx)
                                .cloned()
                                .unwrap_or_else(|| "?".to_string()),
                        ),
                    );
                }
            }
            app.input_mode = InputMode::Normal;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.select.move_down();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.select.move_up();
        }
        KeyCode::Esc => {
            app.select.cancel();
            let msgs = app.msgs();
            app.add_system_message(msgs.selection_cancelled.to_string());
            app.input_mode = InputMode::Normal;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::test_harness::make_app;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn seed_select(app: &mut App) -> tokio::sync::oneshot::Receiver<Option<usize>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.input_mode = InputMode::Select;
        app.select.set(
            "Pick one".into(),
            vec!["Allow once".into(), "Deny".into()],
            tx,
            true,
        );
        rx
    }

    #[test]
    fn j_k_navigates_options() {
        let mut app = make_app();
        let _rx = seed_select(&mut app);

        assert_eq!(app.select.selected, 0);
        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        assert_eq!(app.select.selected, 1);
        handle_select_mode(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.select.selected, 0);
    }

    #[test]
    fn enter_confirms_selection_and_returns_to_normal() {
        let mut app = make_app();
        let mut rx = seed_select(&mut app);

        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert_eq!(rx.try_recv(), Ok(Some(1)));
    }

    #[test]
    fn esc_cancels_and_sends_none() {
        let mut app = make_app();
        let mut rx = seed_select(&mut app);

        handle_select_mode(&mut app, key(KeyCode::Esc));

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert_eq!(rx.try_recv(), Ok(None));
    }
}
