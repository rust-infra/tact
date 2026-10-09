// Input handlers — split by mode.
mod file_picker;
mod hooks;
mod insert;
mod mcp;
mod mouse;
mod normal;
mod overlay;
mod palette;
mod plugin;
mod select;
mod skills;

use chrono::Local;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
pub(crate) use file_picker::handle_file_picker_mode;
pub(crate) use insert::{handle_insert_mode, insert_transcript};
pub(crate) use mouse::handle_mouse_event;
pub(crate) use normal::handle_normal_mode;
pub(crate) use overlay::handle_overlay_key;
pub(crate) use palette::handle_palette_mode;
pub(crate) use select::handle_select_mode;
pub(crate) use skills::flush_pending_when_idle;
use tact_protocol::UserCommand;

use crate::widgets::state::{App, InputMode, SelectKind, SlashCommand, Status};

/// Whether the active sticky panel (task / subagent / background) currently accepts scroll
/// input — the panel must be on screen and its sticky tab expanded.
///
/// Shared by the keyboard (`normal`) and wheel (`mouse`) paths so the two can
/// never disagree about what is scrollable.
pub(crate) fn sticky_scrollable(app: &App) -> bool {
    crate::render::task_panel::sticky_scrollable(app)
}

/// Scroll the active sticky domain's panel by `delta` rows (signed, clamped
/// at zero).
pub(crate) fn scroll_active_sticky(app: &mut App, delta: isize) {
    let tab = crate::render::task_panel::active_sticky_tab(app);
    crate::render::task_panel::scroll_sticky(app, tab, delta);
}

/// One global shortcut: the key it answers to after `Ctrl+`, and what it does.
type GlobalShortcut = (char, fn(&mut App));

/// The built-in global shortcuts — `Ctrl+<key>` — in one table.
///
/// Read twice on purpose: [`handle_global_shortcut`] dispatches on it, and
/// [`is_global_shortcut`] lets the startup check refuse a `voice.voice_keybind`
/// that names one of them. A second list of the same keys would drift the first
/// time a shortcut is added, and the drift would be silent — the new shortcut
/// would simply stay bindable by voice, where it can never fire.
const GLOBAL_SHORTCUTS: &[GlobalShortcut] = &[
    ('c', |app| app.should_quit = true),
    ('h', |app| {
        app.show_history = !app.show_history;
        app.show_help = false;
    }),
    ('t', |app| app.toggle_theme()),
    ('l', |app| app.toggle_language()),
    ('?', |app| {
        app.show_help = !app.show_help;
        app.show_history = false;
    }),
];

/// Global shortcuts, active in **any** input mode. Returns `true` when the key
/// was consumed and the caller must not keep dispatching it.
///
/// Consuming is the point: the mode handlers below match on `KeyCode` alone, so
/// an unconsumed `Ctrl+T` reached the insert handler and typed its letter — the
/// theme switched *and* a `t` landed in the input box on every press. Same for
/// `Ctrl+H` / `Ctrl+L` / `Ctrl+?` / `Ctrl+C`.
///
/// Consuming is also why a user binding may not name one of these keys: nothing
/// dispatched after this function can ever see the event again.
pub(crate) fn handle_global_shortcut(app: &mut App, key: &KeyEvent) -> bool {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    let KeyCode::Char(c) = key.code else {
        return false;
    };
    match GLOBAL_SHORTCUTS.iter().find(|(k, _)| *k == c) {
        Some((_, run)) => {
            run(app);
            true
        }
        None => false,
    }
}

/// Whether `Ctrl+<c>` is one of the built-in global shortcuts.
pub(crate) fn is_global_shortcut(c: char) -> bool {
    GLOBAL_SHORTCUTS.iter().any(|(k, _)| *k == c)
}

/// One built-in global shortcut as a `Ctrl+<KEY>` label — the same spelling the
/// help panel uses for the voice binding.
pub(crate) fn global_shortcut_label(c: char) -> String {
    format!("Ctrl+{}", c.to_uppercase())
}

/// The built-in global shortcuts as labels, for error messages.
pub(crate) fn global_shortcut_labels() -> String {
    GLOBAL_SHORTCUTS
        .iter()
        .map(|(c, _)| global_shortcut_label(*c))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Returns the byte index of the previous char boundary before `cursor`.
fn prev_char_boundary(s: &str, cursor: usize) -> usize {
    let cursor = s.floor_char_boundary(cursor.min(s.len()));
    s[..cursor]
        .char_indices()
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Returns the byte index of the next char boundary after `cursor`.
fn next_char_boundary(s: &str, cursor: usize) -> usize {
    let cursor = s.floor_char_boundary(cursor.min(s.len()));
    s[cursor..]
        .chars()
        .next()
        .map(|c| cursor + c.len_utf8())
        .unwrap_or(cursor)
}

/// Returns the byte index at the start of the line that contains `cursor`.
fn start_of_line(s: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let cursor = s.floor_char_boundary(cursor.min(s.len()));
    s[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

/// Returns the byte index at the end of the line that contains `cursor` (newline position, or string length for the last line).
fn end_of_line(s: &str, cursor: usize) -> usize {
    let cursor = s.floor_char_boundary(cursor.min(s.len()));
    s[cursor..]
        .find('\n')
        .map(|i| cursor + i)
        .unwrap_or(s.len())
}

/// Exit history navigation mode.
fn exit_history(app: &mut App) {
    app.input_history.index = None;
    app.input_history.saved.clear();
}

/// Compute the (line, column) of the cursor position, counting columns in characters.
fn cursor_line_col(s: &str, cursor: usize) -> (usize, usize) {
    let mut line = 0;
    let mut col = 0;
    for (i, c) in s.char_indices() {
        if i >= cursor {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Returns the character length (excluding newline) of the given line.
fn line_length(s: &str, target_line: usize) -> usize {
    let mut line = 0;
    let mut len = 0;
    for c in s.chars() {
        if line == target_line {
            if c == '\n' {
                break;
            }
            len += 1;
        } else if c == '\n' {
            line += 1;
        }
    }
    len
}

/// Convert (line, column) to a byte index.
fn line_col_to_cursor(s: &str, target_line: usize, target_col: usize) -> usize {
    let mut line = 0;
    let mut col = 0;
    for (i, c) in s.char_indices() {
        if line == target_line && col == target_col {
            return i;
        }
        if c == '\n' {
            if line == target_line {
                return i;
            }
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    s.len()
}

/// Returns true if the character is a word character (alphanumeric or underscore).
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Returns the byte index of the word start before `cursor` (backward-delete word).
fn prev_word_boundary(s: &str, cursor: usize) -> usize {
    let cursor = s.floor_char_boundary(cursor.min(s.len()));
    let mut pos = cursor;
    let mut chars = s[..cursor].chars().rev().peekable();

    // Skip whitespace
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            pos -= c.len_utf8();
            chars.next();
        } else {
            break;
        }
    }

    // Record the type of the first non-whitespace char, then skip same-type chars
    if let Some(&first) = chars.peek() {
        if is_word_char(first) {
            while let Some(&c) = chars.peek() {
                if is_word_char(c) {
                    pos -= c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if !c.is_whitespace() && !is_word_char(c) {
                    pos -= c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        }
    }

    pos
}

/// Returns the byte index of the word end after `cursor` (forward-delete word).
fn next_word_boundary(s: &str, cursor: usize) -> usize {
    let cursor = s.floor_char_boundary(cursor.min(s.len()));
    let mut pos = cursor;
    let mut chars = s[cursor..].chars().peekable();

    // Skip whitespace
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            pos += c.len_utf8();
            chars.next();
        } else {
            break;
        }
    }

    // Record the type of the first non-whitespace char, then skip same-type chars
    if let Some(&first) = chars.peek() {
        if is_word_char(first) {
            while let Some(&c) = chars.peek() {
                if is_word_char(c) {
                    pos += c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if !c.is_whitespace() && !is_word_char(c) {
                    pos += c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        }
    }

    pos
}

/// Execute a command from palette/slash input.
///
/// Returns whether the caller should clear the input box afterwards.
pub(super) struct CommandExecOutcome {
    pub handled: bool,
    pub clear_input: bool,
}

impl CommandExecOutcome {
    /// The command ran: clear the input box so the next message starts fresh.
    fn handled() -> Self {
        Self {
            handled: true,
            clear_input: true,
        }
    }

    /// Nothing here answers to that name.
    fn unhandled() -> Self {
        Self {
            handled: false,
            clear_input: false,
        }
    }
}

/// Built-ins that take a subcommand / arguments: Enter should autocomplete
/// `/{cmd} ` into the insert box instead of executing immediately.
pub(crate) fn command_needs_args(cmd: &str) -> bool {
    SlashCommand::from_name(cmd).is_some_and(SlashCommand::needs_args)
}

pub(crate) fn execute_palette_command(app: &mut App, cmd: &str) -> CommandExecOutcome {
    // A name that is not a built-in may still be a skill; built-ins always win
    // so a skill named `cancel`/`help`/… cannot shadow them.
    let Some(command) = SlashCommand::from_name(cmd) else {
        return skills::handle_skill_command(app, cmd)
            .unwrap_or_else(CommandExecOutcome::unhandled);
    };
    run_command(app, command)
}

/// Dispatch one built-in command.
///
/// The match has no `_` arm on purpose: it used to, and that is how a command
/// could be listed in the palette and answered with `handled: false` — the
/// failure surfaced only if a test happened to walk the list.
fn run_command(app: &mut App, command: SlashCommand) -> CommandExecOutcome {
    use SlashCommand as C;

    match command {
        C::Theme => {
            start_theme_picker(app);
            CommandExecOutcome::handled()
        }
        C::Model => {
            crate::handlers::select::start_model_picker(app);
            CommandExecOutcome::handled()
        }
        C::ModelSubagent => {
            crate::handlers::select::start_subagent_model_picker(app);
            CommandExecOutcome::handled()
        }
        C::Permission => {
            start_permission_picker(app);
            CommandExecOutcome::handled()
        }
        C::ViewSystemPrompt => {
            app.select.set_local(
                "View system prompt".to_string(),
                vec![
                    "Raw template".to_string(),
                    "Assembled current prompt".to_string(),
                ],
                0,
                false,
            );
            app.select_kind = SelectKind::ViewSystemPrompt;
            app.input_mode = InputMode::Select;
            CommandExecOutcome::handled()
        }
        C::Save => {
            let timestamp = Local::now().format("%Y%m%d_%H%M%S");
            let path = std::env::temp_dir().join(format!("agent_log_{timestamp}.txt"));
            if let Ok(mut file) = std::fs::File::create(&path) {
                use std::io::Write;
                for item in &app.log.items {
                    writeln!(file, "{}", item.raw).ok();
                }
                let msgs = app.msgs();
                app.add_system_message(
                    msgs.log_saved_tmpl
                        .replace("{}", &path.display().to_string()),
                );
            } else {
                let msgs = app.msgs();
                app.add_system_message(msgs.log_save_failed.to_string());
            }
            CommandExecOutcome::handled()
        }
        C::Quit => {
            app.should_quit = true;
            CommandExecOutcome::handled()
        }
        C::Help => {
            app.show_help = !app.show_help;
            app.show_history = false;
            CommandExecOutcome::handled()
        }
        C::History => {
            app.show_history = !app.show_history;
            app.show_help = false;
            CommandExecOutcome::handled()
        }
        C::Skill => skills::handle_skill_builtin_command(app),
        C::Plugin => plugin::handle_plugin_command(app),
        C::Mcp => mcp::handle_mcp_command(app),
        C::Hooks => hooks::handle_hooks_command(app),
        C::HookOutput => {
            // A boolean has no list to pick from, so this is the `Ctrl+T` shape
            // rather than the `/theme` one: flip, write, report in one message.
            app.toggle_hook_output();
            CommandExecOutcome::handled()
        }
        C::Cancel => {
            // Only cancel an in-flight task; Idle and Done have nothing to
            // abort. Queued (pending) messages are NOT touched — dropping
            // them is the `[Cancel]` button's job.
            if matches!(app.status, Status::Planning | Status::Executing { .. }) {
                app.cancel_task();
            } else {
                app.flash_msg = Some((
                    app.msgs().cancel_noop_msg.to_string(),
                    std::time::Instant::now(),
                ));
            }
            CommandExecOutcome::handled()
        }
        C::SubagentCancel => {
            // Cancel a running background subagent: `/subagent_cancel <child-id>`.
            // The child-id comes from the `async_launched { id }` handle or
            // `check_subagent`. The driver flips the child's cooperative flag;
            // it works even while the parent task is mid-turn.
            let rest = app
                .input
                .trim()
                .strip_prefix("/subagent_cancel")
                .unwrap_or("")
                .trim();
            if rest.is_empty() {
                app.flash_msg = Some((
                    app.msgs().subagent_cancel_usage_msg.to_string(),
                    std::time::Instant::now(),
                ));
            } else {
                let child_id = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let _ = app
                    .user_cmd_tx
                    .send(UserCommand::CancelSubagent { child_id });
            }
            CommandExecOutcome::handled()
        }
        C::Compact => {
            // Only compact when idle; active tasks cannot be compacted
            // from slash since compact_history synchronously rewrites
            // the session context.
            if matches!(app.status, Status::Planning | Status::Executing { .. }) {
                let msg = app.msgs().input_busy_msg.to_string();
                app.flash_msg = Some((msg, std::time::Instant::now()));
            } else {
                let _ = app.user_cmd_tx.send(UserCommand::Compact);
            }
            CommandExecOutcome::handled()
        }
        C::Balance => {
            if app.account_rx.is_none() {
                return CommandExecOutcome::handled();
            }
            let _ = app.user_cmd_tx.send(UserCommand::QueryBalance);
            CommandExecOutcome::handled()
        }
        C::Stats => {
            let _ = app.user_cmd_tx.send(UserCommand::QueryStats);
            CommandExecOutcome::handled()
        }
        C::TasksDag => {
            app.open_task_dag_popup();
            CommandExecOutcome::handled()
        }
        C::Background => {
            // `/background` lists all background tasks; `/background <id>`
            // shows one task (pretty JSON). The optional id comes from the
            // remaining input after the command token.
            let task_id = app.input.split_whitespace().nth(1).map(str::to_string);
            let _ = app.user_cmd_tx.send(UserCommand::QueryBackground(task_id));
            CommandExecOutcome::handled()
        }
        C::Lang => {
            // Toggling is the whole choice with two languages, so `Ctrl+L` and
            // `/lang` move the same way; `/lang` differs by asking whether the
            // choice should outlive the session. Silent inside because that
            // step speaks for both, exactly as `/theme`'s does.
            select::start_language_toggle(app);
            CommandExecOutcome::handled()
        }
    }
}

/// `/skill list` output is plain markdown (heading + pipe table) appended as
/// whole-Markdown messages and rendered by `MarkdownCell`. Long lists are
/// split into pages of [`SKILLS_PER_PAGE`] rows so each message stays
/// readable and scrollable instead of one giant table.
const SKILLS_PER_PAGE: usize = 15;

fn show_skills_command(app: &mut App) {
    app.add_new_line();

    let chunks = skills_list_markdown_pages(&app.skills_data, SKILLS_PER_PAGE);
    if chunks.is_empty() {
        app.append_system_markdown("(no skills available)".to_string());
    } else {
        for (i, chunk) in chunks.into_iter().enumerate() {
            if i > 0 {
                app.add_new_line();
            }
            app.append_system_markdown(chunk);
        }
    }

    // Trailing blank so the next `/skill list` (or other system block) is not flush.
    app.add_new_line();

    if app.input_mode == crate::widgets::state::InputMode::Insert
        || app.input_mode == crate::widgets::state::InputMode::Normal
    {
        app.scroll_log_to_bottom();
    }
}

/// Build markdown for the skill list: a heading plus a pipe table.
///
/// The output is plain markdown — `render_markdown_with_tables` picks the
/// table rows and lays them out width-aware (long descriptions wrap inside
/// the table), so no hand-built ratatui lines are needed.
///
/// Production output is paginated via [`skills_list_markdown_pages`]; the
/// single-table form is kept for tests.
#[cfg(test)]
fn skills_list_markdown(skills: &[crate::widgets::state::SkillEntry]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut skills: Vec<_> = skills.iter().collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills_table_markdown(&skills, None)
}

/// Split the sorted skill table into pages of `per_page` rows. Each page is
/// an independent markdown message whose heading carries the page number.
fn skills_list_markdown_pages(
    skills: &[crate::widgets::state::SkillEntry],
    per_page: usize,
) -> Vec<String> {
    if skills.is_empty() {
        return Vec::new();
    }
    let mut skills: Vec<_> = skills.iter().collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    let pages = skills.chunks(per_page.max(1)).collect::<Vec<_>>();
    let total = pages.len();
    pages
        .into_iter()
        .enumerate()
        .map(|(i, page)| skills_table_markdown(page, Some(format!("({}/{})", i + 1, total))))
        .collect()
}

/// One heading + pipe table for `skills`. `suffix` appends a page marker to
/// the heading (e.g. `(2/4)`) when the list is paginated.
fn skills_table_markdown(
    skills: &[&crate::widgets::state::SkillEntry],
    suffix: Option<String>,
) -> String {
    let mut out = match suffix {
        Some(suffix) => format!("## 📋 Available skills {suffix}\n\n"),
        None => String::from("## 📋 Available skills\n\n"),
    };
    out.push_str("| Skill | Description |\n");
    out.push_str("| ----- | ----------- |\n");
    for (i, skill) in skills.iter().enumerate() {
        if i > 0 {
            // Row separator: `format_table` recognizes these and renders a
            // dashed divider matching the column widths.
            out.push_str("| --- | --- |\n");
        }
        // Inline code keeps `/` and `:` literal in namespaced skills. Cell
        // text is flattened: newlines become spaces and `|` becomes a full
        // width pipe so the pipe table stays well-formed.
        let desc = skill
            .description
            .trim()
            .replace('\n', " ")
            .replace('|', "｜");
        out.push_str(&format!("| `{}` | {} |\n", skill.name, desc));
    }
    out
}

/// Reload skills from disk into the shared registry (agent + TUI).
///
/// Heavy: scans the filesystem while holding the registry mutex, so callers run
/// it inside `spawn_blocking` (see `App::start_skills_reload`). Kept synchronous
/// and lock-scoped: no lock is ever held across an `.await`.
pub(crate) fn reload_skills(
    registry: &tact_extensions::skill::SharedSkillRegistry,
    work_dir: &std::path::Path,
) -> Result<crate::widgets::state::app::background::SkillsSnapshot, String> {
    use crate::widgets::state::app::background::SkillsSnapshot;

    let mut reg = tact_extensions::skill::lock_skills(registry);
    // Keep search roots in sync with the current workdir (tests may set work_dir late).
    *reg = tact_extensions::skill::get_skill_registry(work_dir).map_err(|e| e.to_string())?;
    let description = reg.describe_available();
    let data = reg
        .skills()
        .values()
        .map(|doc| crate::widgets::state::SkillEntry {
            name: doc.manifest.name.clone(),
            description: doc.manifest.description.clone(),
            body: doc.body.clone(),
        })
        .collect();
    Ok(SkillsSnapshot { description, data })
}

/// Open the `/theme` SelectPopup from the palette / slash command.
///
/// Twelve themes cycled one at a time meant up to eleven presses to reach the
/// one you want, which is what the picker replaces; `Ctrl+T` still cycles for
/// the "next one" case.
pub(crate) fn start_theme_picker(app: &mut App) {
    use crate::theme::ThemeName;
    use crate::widgets::state::app::config::theme_label;

    let msgs = app.msgs();
    let current = app.theme.name;
    let options: Vec<String> = ThemeName::all()
        .iter()
        .map(|name| {
            let label = theme_label(&msgs, *name);
            if *name == current {
                format!("{label} *")
            } else {
                label.to_string()
            }
        })
        .collect();
    let selected = ThemeName::all()
        .iter()
        .position(|name| *name == current)
        .unwrap_or(0);
    app.select_kind = crate::widgets::state::SelectKind::ThemePick;
    app.select.set_local(
        msgs.theme_select_prompt.to_string(),
        options,
        selected,
        true,
    );
    app.input_mode = InputMode::Select;
}

/// Open the `/permission` SelectPopup from palette / slash command.
pub(crate) fn start_permission_picker(app: &mut App) {
    let msgs = app.msgs();
    let options = vec![
        msgs.permission_option_default.to_string(),
        msgs.permission_option_plan.to_string(),
        msgs.permission_option_auto.to_string(),
    ];
    let current = match app.status_bar_mut().permission_mode.as_str() {
        "plan" => 1,
        "auto" => 2,
        _ => 0,
    };
    app.select_kind = crate::widgets::state::SelectKind::PermissionModePick;
    app.select.set_local(
        msgs.permission_select_prompt.to_string(),
        options,
        current,
        true,
    );
    app.input_mode = crate::widgets::state::InputMode::Select;
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::{
        execute_palette_command, global_shortcut_labels, handle_global_shortcut,
        is_global_shortcut, skills_list_markdown,
    };
    use crate::test_fixtures::TestApp;
    use crate::widgets::state::{App, InputMode, SlashCommand, Status, Subcommand};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tact_protocol::UserCommand;

    /// Runs `/skill <sub>` the way the input box does: the palette dispatches on
    /// the command name alone, and the handler reads the subcommand from the
    /// input, exactly like `/mcp list`.
    fn run_skill_command(app: &mut App, sub: &str) -> super::CommandExecOutcome {
        app.input = format!("/skill {sub}");
        execute_palette_command(app, "skill")
    }

    #[test]
    fn palette_commands_are_all_handled() {
        let (mut app, _user_cmd_rx) = TestApp::new().into_commands();
        let (_tx, account_rx) = tokio::sync::mpsc::unbounded_channel();
        app.account_rx = Some(account_rx);
        let cmds = app.palette_commands();
        let commands: Vec<(&str, &str)> =
            cmds.iter().map(|(c, d)| (c.as_str(), d.as_str())).collect();

        for (cmd, _desc) in &commands {
            if *cmd == "cancel" {
                app.status = Status::Planning;
            }
            let outcome = execute_palette_command(&mut app, cmd);
            assert!(outcome.handled, "expected command `{cmd}` to be handled");
        }
    }

    #[test]
    fn unknown_command_is_not_handled() {
        let (mut app, _user_cmd_rx) = TestApp::new().into_commands();
        let outcome = execute_palette_command(&mut app, "nonexistent");
        assert!(!outcome.handled);
        assert!(!outcome.clear_input);
    }

    #[test]
    fn every_global_shortcut_is_consumed() {
        // Consumption is what keeps `Ctrl+<char>` out of the mode handlers,
        // which match on `KeyCode` alone and would type the letter.
        let (mut app, _user_cmd_rx) = TestApp::new().into_commands();
        let theme = app.theme.name;
        let language = app.language;

        for code in [
            KeyCode::Char('c'),
            KeyCode::Char('h'),
            KeyCode::Char('t'),
            KeyCode::Char('l'),
            KeyCode::Char('?'),
        ] {
            assert!(
                handle_global_shortcut(&mut app, &KeyEvent::new(code, KeyModifiers::CONTROL)),
                "{code:?} + Ctrl must be consumed"
            );
        }

        assert!(app.should_quit, "Ctrl+C asks to quit");
        assert_ne!(app.theme.name, theme, "Ctrl+T cycles the theme");
        assert_ne!(app.language, language, "Ctrl+L flips the language");
        // Ctrl+H then Ctrl+? — the two panels are mutually exclusive, and the
        // later one wins.
        assert!(app.show_help, "Ctrl+? shows the help panel");
        assert!(!app.show_history, "Ctrl+? hides the history panel");
    }

    #[test]
    fn the_reserved_set_matches_the_dispatcher() {
        // `is_global_shortcut` and `handle_global_shortcut` read one table, so
        // this pins its contents: a key the dispatcher consumes must also be
        // reported as reserved, or a voice binding on it would be accepted and
        // then never fire.
        let (mut app, _user_cmd_rx) = TestApp::new().into_commands();
        let labels = global_shortcut_labels();

        for c in ['c', 'h', 't', 'l', '?'] {
            assert!(is_global_shortcut(c), "Ctrl+{c} must be reserved");
            assert!(
                handle_global_shortcut(
                    &mut app,
                    &KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
                ),
                "Ctrl+{c} must be consumed"
            );
            assert!(
                labels.contains(&format!("Ctrl+{}", c.to_uppercase())),
                "the message must name Ctrl+{c}: {labels}"
            );
        }

        for c in ['g', 'r', ','] {
            assert!(!is_global_shortcut(c), "Ctrl+{c} is not a global shortcut");
            assert!(
                !labels.contains(&format!("Ctrl+{}", c.to_uppercase())),
                "the message must not claim Ctrl+{c}: {labels}"
            );
        }
    }

    #[test]
    fn an_unbound_ctrl_key_is_left_to_the_mode_handler() {
        let (mut app, _user_cmd_rx) = TestApp::new().into_commands();
        let theme = app.theme.name;

        let consumed = handle_global_shortcut(
            &mut app,
            &KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
        );

        assert!(!consumed, "Ctrl+G is bound nowhere and must fall through");
        assert_eq!(app.theme.name, theme);
        assert!(!app.should_quit);
    }

    #[test]
    fn a_plain_letter_is_left_to_the_mode_handler() {
        let (mut app, _user_cmd_rx) = TestApp::new().into_commands();
        let theme = app.theme.name;

        let consumed = handle_global_shortcut(
            &mut app,
            &KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
        );

        assert!(!consumed, "a bare `t` is text, not a shortcut");
        assert_eq!(app.theme.name, theme);
    }

    #[test]
    fn cancel_command_leaves_queued_messages_alone() {
        let (mut app, mut user_cmd_rx) = TestApp::new().into_commands();
        app.status = Status::Executing {
            current_step: 0,
            total: 1,
        };
        app.queue_pending_message("one".into(), "one".into());
        app.queue_pending_message("two".into(), "two".into());
        assert_eq!(app.pending_messages.len(), 2);

        let outcome = execute_palette_command(&mut app, "cancel");

        assert!(outcome.handled);
        assert_eq!(
            app.pending_messages.len(),
            2,
            "/cancel must NOT clear the queue — the [Cancel] button does that"
        );
        assert!(matches!(
            user_cmd_rx.try_recv().expect("expected Cancel"),
            UserCommand::Cancel
        ));
    }

    #[test]
    fn cancel_command_idle_keeps_noop_flash() {
        let (mut app, mut user_cmd_rx) = TestApp::new().into_commands();
        app.status = Status::Idle;
        app.queue_pending_message("stale".into(), "stale".into());

        let outcome = execute_palette_command(&mut app, "cancel");

        assert!(outcome.handled);
        assert_eq!(
            app.pending_messages.len(),
            1,
            "idle cancel must not touch the queue"
        );
        assert!(
            app.flash_msg.is_some(),
            "idle cancel still shows noop flash"
        );
        assert!(
            user_cmd_rx.try_recv().is_err(),
            "idle cancel must not dispatch Cancel"
        );
    }

    #[test]
    fn subagent_cancel_without_args_shows_usage() {
        let (mut app, mut user_cmd_rx) = TestApp::new().into_commands();
        app.input = "/subagent_cancel".into();
        app.input_cursor = app.input.len();

        let outcome = execute_palette_command(&mut app, "subagent_cancel");

        assert!(outcome.handled);
        assert!(app.flash_msg.is_some(), "missing args must flash usage");
        assert!(
            user_cmd_rx.try_recv().is_err(),
            "no command must be sent without a child id"
        );
    }

    #[test]
    fn subagent_cancel_with_id_sends_command() {
        let (mut app, mut user_cmd_rx) = TestApp::new().into_commands();
        app.input = "/subagent_cancel child-123".into();
        app.input_cursor = app.input.len();

        let outcome = execute_palette_command(&mut app, "subagent_cancel");

        assert!(outcome.handled);
        assert!(app.flash_msg.is_none());
        assert!(matches!(
            user_cmd_rx.try_recv().expect("expected CancelSubagent"),
            UserCommand::CancelSubagent { ref child_id } if child_id == "child-123"
        ));
    }

    #[test]
    fn skills_list_markdown_from_entries() {
        let skills = vec![
            crate::widgets::state::SkillEntry {
                name: "code-reviewer".into(),
                description: "代码审查专家".into(),
                body: String::new(),
            },
            crate::widgets::state::SkillEntry {
                name: "demo-test".into(),
                description: "测试 skill 加载功能".into(),
                body: String::new(),
            },
        ];
        let md = skills_list_markdown(&skills);
        assert!(
            md.contains("## 📋 Available skills"),
            "missing heading:\n{md}"
        );
        assert!(
            md.contains("| Skill | Description |"),
            "missing table header:\n{md}"
        );
        assert!(
            md.contains("| `code-reviewer` | 代码审查专家 |"),
            "missing first skill row:\n{md}"
        );
        assert!(
            md.contains("| `demo-test` | 测试 skill 加载功能 |"),
            "missing second skill row:\n{md}"
        );
        // Between the two data rows there must be a row separator.
        assert!(
            md.contains("| `code-reviewer` | 代码审查专家 |\n| --- | --- |\n| `demo-test` | 测试 skill 加载功能 |"),
            "missing row separator between skills:\n{md}"
        );
    }

    #[test]
    fn skills_list_markdown_preserves_namespaced_skill_name() {
        let skills = vec![crate::widgets::state::SkillEntry {
            name: "plugin:skill".into(),
            description: "Plugin-provided skill".into(),
            body: String::new(),
        }];
        let md = skills_list_markdown(&skills);
        assert!(
            md.contains("| `plugin:skill` | Plugin-provided skill |"),
            "namespaced name broken:\n{md}"
        );
    }

    #[test]
    fn skills_list_markdown_empty_is_empty() {
        let md = skills_list_markdown(&[]);
        assert!(md.is_empty(), "expected empty markdown, got:\n{md}");
    }

    /// Expands a subcommand tree into one sample input per leaf: `/hooks trust
    /// --source <label>` becomes `/hooks trust --source sample`.
    fn sample_subcommand_inputs(
        command: &str,
        subs: &[Subcommand],
        prefix: String,
        out: &mut Vec<String>,
    ) {
        for sub in subs {
            let path = if prefix.is_empty() {
                sub.name.to_string()
            } else {
                format!("{prefix} {}", sub.name)
            };
            if sub.children.is_empty() {
                let value = if sub.takes_value { " sample" } else { "" };
                out.push(format!("/{command} {path}{value}"));
            } else {
                assert!(
                    !sub.takes_value,
                    "`{}` cannot both take a value and have subcommands",
                    sub.name
                );
                sample_subcommand_inputs(command, sub.children, path, out);
            }
        }
    }

    /// Every subcommand the popup completes must actually run.
    ///
    /// The declaration in `SlashCommand::subcommands()` drives the completion;
    /// the handlers still parse the tokens themselves. This walks the
    /// declaration and dispatches a sample input for each leaf, so a subcommand
    /// that is offered but not implemented — completing straight into the usage
    /// hint — fails here instead of at the user's fingertips.
    #[test]
    fn every_declared_subcommand_has_a_handler() {
        for command in SlashCommand::iter() {
            let mut inputs = Vec::new();
            sample_subcommand_inputs(
                command.name(),
                command.subcommands(),
                String::new(),
                &mut inputs,
            );
            assert_eq!(
                inputs.is_empty(),
                command.subcommands().is_empty(),
                "{command:?} declares subcommands but expands to no sample input"
            );
            for input in inputs {
                let (mut app, _rx) = TestApp::new().into_commands();
                app.input = input.clone();

                let _ = execute_palette_command(&mut app, command.name());

                let hinted = app
                    .flash_msg
                    .as_ref()
                    .is_some_and(|(message, _)| is_usage_text(message))
                    || app.log.items.iter().any(|item| is_usage_text(&item.raw));
                assert!(
                    !hinted,
                    "`{input}` is completable but reaches the usage hint"
                );
            }
        }
    }

    /// The usage hints are the only localized strings every handler shares.
    fn is_usage_text(text: &str) -> bool {
        text.contains("Usage") || text.contains("用法")
    }

    #[test]
    fn skill_command_lists_skills_and_clears_the_input() {
        let (mut app, _rx) = TestApp::new().into_commands();
        app.skills_data = vec![crate::widgets::state::SkillEntry {
            name: "code-reviewer".into(),
            description: "代码审查专家".into(),
            body: String::new(),
        }];

        let outcome = run_skill_command(&mut app, "list");

        assert!(outcome.handled);
        assert!(outcome.clear_input, "the subcommand was consumed");
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("code-reviewer")),
            "expected the skill table, got: {:?}",
            app.log.items
        );
    }

    #[test]
    fn skill_command_reload_reports_the_rescan() {
        let (mut app, _rx) = TestApp::new().into_commands();

        let outcome = run_skill_command(&mut app, "reload");

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        // No reactor in a sync test, so the reload runs inline; the command's
        // own report is what the user sees.
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("Reloaded")),
            "expected a reload report, got: {:?}",
            app.log.items
        );
    }

    #[test]
    fn bare_skill_command_offers_the_usage_and_keeps_the_input() {
        // Same contract as bare `/mcp`: hand the user a runnable command
        // instead of silently doing nothing.
        for input in ["/skill", "/skill nonsense"] {
            let (mut app, _rx) = TestApp::new().into_commands();
            app.input = input.to_string();

            let outcome = execute_palette_command(&mut app, "skill");

            assert!(outcome.handled, "{input}");
            assert!(!outcome.clear_input, "{input}");
            assert_eq!(app.input, "/skill ", "{input}");
            let (flash, _) = app.flash_msg.as_ref().expect("usage flash");
            assert!(flash.contains("/skill list"), "{input}: {flash}");
        }
    }

    #[test]
    fn skills_command_adds_separators_around_list() {
        let (mut app, _rx) = TestApp::new().into_commands();
        app.skills_data = vec![
            crate::widgets::state::SkillEntry {
                name: "code-reviewer".into(),
                description: "代码审查专家".into(),
                body: String::new(),
            },
            crate::widgets::state::SkillEntry {
                name: "demo-test".into(),
                description: "测试".into(),
                body: String::new(),
            },
        ];
        let before = app.log.items.len();
        run_skill_command(&mut app, "list");
        let after_first = app.log.items.len();
        assert!(after_first > before);
        assert!(
            app.log.items.iter().any(|item| {
                item.raw.contains("code-reviewer") || item.raw.contains("Available skills")
            }),
            "expected skills content, got: {:?}",
            app.log.items
        );
        // Second invocation must not glue flush to the previous block.
        run_skill_command(&mut app, "list");
        let joined = app.log.items[after_first.saturating_sub(1)..]
            .iter()
            .map(|item| item.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            app.log.items[after_first - 1].raw.is_empty()
                || app
                    .log
                    .items
                    .get(after_first)
                    .is_some_and(|item| item.raw.is_empty()),
            "expected blank separator between skills blocks, around: {joined}"
        );
    }

    #[test]
    fn skills_command_paginates_long_lists() {
        let (mut app, _rx) = TestApp::new().into_commands();
        app.skills_data = (0..40)
            .map(|i| crate::widgets::state::SkillEntry {
                name: format!("skill-{i:02}"),
                description: "d".into(),
                body: String::new(),
            })
            .collect();
        let before = app.log.items.len();
        run_skill_command(&mut app, "list");
        let raw = &app.log.items[before..];

        // 40 skills / 15 per page → 3 pages with numbered headings.
        for page in ["(1/3)", "(2/3)", "(3/3)"] {
            assert!(
                raw.iter()
                    .any(|item| item.raw.contains(&format!("Available skills {page}"))),
                "missing page heading {page}, got: {:?}",
                raw
            );
        }
        // Page boundaries: 0-14, 15-29, 30-39.
        for name in [
            "skill-00", "skill-14", "skill-15", "skill-29", "skill-30", "skill-39",
        ] {
            assert!(
                raw.iter().any(|item| item.raw.contains(name)),
                "missing skill {name}, got: {:?}",
                raw
            );
        }
    }

    #[test]
    fn skills_command_output_reaches_middle_skills_when_scrolling() {
        // Regression for the reported symptom: with ~60 skills the old
        // logical-row scrolling never showed alphabetically-middle entries
        // (lark-*) of the `/skill list` table. Paginated pages + visual stepping
        // must make every row reachable.
        let (mut app, _rx) = TestApp::new().into_commands();
        app.skills_data = (0..58)
            .map(|i| crate::widgets::state::SkillEntry {
                name: if (20..=47).contains(&i) {
                    format!("lark-skill-{i:02}")
                } else {
                    format!("skill-{i:02}")
                },
                description: format!("desc {i}"),
                body: String::new(),
            })
            .collect();
        if let Some(entry) = app
            .skills_data
            .iter_mut()
            .find(|e| e.name == "lark-skill-30")
        {
            entry.name = "lark-doc".into();
        }
        run_skill_command(&mut app, "list");
        app.scroll_log_to_top();

        let viewport_height = 8usize;
        let step = crate::widgets::state::app::scroll::key_cell_step(viewport_height);
        let mut seen = false;
        for _ in 0..300 {
            let text = crate::render::test_harness::render_log_panel_text(
                &mut app,
                60,
                viewport_height as u16 + 2,
            );
            if text.contains("lark-doc") {
                seen = true;
                break;
            }
            app.scroll_log_down(step);
        }
        assert!(
            seen,
            "lark-doc never visible while traversing /skill list output"
        );
    }

    #[test]
    fn cancel_while_done_is_noop() {
        let (mut app, mut user_cmd_rx) = TestApp::new().into_commands();
        app.status = Status::Done;
        let outcome = execute_palette_command(&mut app, "cancel");
        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(app.flash_msg.is_some());
        assert!(
            user_cmd_rx.try_recv().is_err(),
            "Done must not dispatch Cancel"
        );
    }

    #[test]
    fn background_command_dispatches_list_all() {
        let (mut app, mut user_cmd_rx) = TestApp::new().into_commands();
        app.input = "/background".into();
        let outcome = execute_palette_command(&mut app, "background");
        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(matches!(
            user_cmd_rx.try_recv().expect("expected QueryBackground"),
            UserCommand::QueryBackground(None)
        ));
    }

    #[test]
    fn background_command_forwards_task_id() {
        let (mut app, mut user_cmd_rx) = TestApp::new().into_commands();
        app.input = "/background 018f3a2c".into();
        let outcome = execute_palette_command(&mut app, "background");
        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(matches!(
            user_cmd_rx.try_recv().expect("expected QueryBackground"),
            UserCommand::QueryBackground(Some(id)) if id == "018f3a2c"
        ));
    }

    #[test]
    fn cancel_while_executing_dispatches() {
        let (mut app, mut user_cmd_rx) = TestApp::new().into_commands();
        app.status = Status::Executing {
            current_step: 0,
            total: 1,
        };
        let outcome = execute_palette_command(&mut app, "cancel");
        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(matches!(
            user_cmd_rx.try_recv().expect("expected Cancel"),
            UserCommand::Cancel
        ));
    }

    /// `/theme` opens the picker (it used to cycle): twelve themes meant up to
    /// eleven presses to reach the one you want.
    #[test]
    fn theme_command_opens_a_picker_marked_at_the_current_theme() {
        use crate::theme::ThemeName;
        use crate::widgets::state::SelectKind;

        let (mut app, _user_cmd_rx) = TestApp::new().into_commands();
        assert_eq!(app.theme.name, ThemeName::Retro);

        let outcome = execute_palette_command(&mut app, "theme");

        assert!(outcome.handled);
        assert!(matches!(app.select_kind, SelectKind::ThemePick));
        assert!(matches!(app.input_mode, InputMode::Select));
        assert_eq!(app.select.options.len(), ThemeName::all().len());
        let labelled: Vec<(usize, &str)> = app
            .select
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| (i, o.as_str()))
            .collect();
        let marked: Vec<&(usize, &str)> = labelled
            .iter()
            .filter(|(_, label)| label.ends_with(" *"))
            .collect();
        assert_eq!(marked.len(), 1, "exactly one row is the current theme");
        assert_eq!(
            marked[0].0, app.select.selected,
            "the picker opens on the current theme, not at the top"
        );
        assert_eq!(
            marked[0].1.strip_suffix(" *").unwrap(),
            crate::widgets::state::app::config::theme_label(&app.msgs(), ThemeName::Retro)
        );
    }

    #[test]
    fn confirming_the_theme_picker_applies_the_chosen_theme() {
        use crate::theme::ThemeName;

        let (mut app, _user_cmd_rx) = TestApp::new().into_commands();
        execute_palette_command(&mut app, "theme");
        let target = ThemeName::all()
            .iter()
            .position(|name| *name == ThemeName::Nord)
            .expect("nord is a built-in theme");
        app.select.selected = target;

        crate::handlers::select::handle_select_mode(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::empty(),
            ),
        );

        assert_eq!(app.theme.name, ThemeName::Nord);
        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("Nord") || item.raw.contains("nord")),
            "the switch is announced: {:?}",
            app.log.items
        );
    }

    #[test]
    fn cancelling_the_theme_picker_keeps_the_theme() {
        use crate::theme::ThemeName;

        let (mut app, _user_cmd_rx) = TestApp::new().into_commands();
        let before = app.theme.name;
        execute_palette_command(&mut app, "theme");
        app.select.selected = 0;

        crate::handlers::select::handle_select_mode(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Esc,
                crossterm::event::KeyModifiers::empty(),
            ),
        );

        assert_eq!(app.theme.name, before, "Esc must not switch the theme");
        assert!(matches!(app.input_mode, InputMode::Normal));
        assert_ne!(before, ThemeName::Dark, "the test needs a non-first theme");
    }

    /// `Ctrl+T` keeps the cheap "next one" path the picker replaces.
    #[test]
    fn ctrl_t_still_cycles_themes() {
        let (mut app, _user_cmd_rx) = TestApp::new().into_commands();
        let before = app.theme.name;

        app.toggle_theme();

        assert_ne!(app.theme.name, before);
    }

    #[test]
    fn builtin_command_wins_over_same_named_skill() {
        use crate::widgets::state::SkillEntry;

        let (mut app, mut user_cmd_rx) = TestApp::new().into_commands();
        app.skills_data = vec![SkillEntry {
            name: "cancel".into(),
            description: "fake".into(),
            body: "should not run".into(),
        }];
        app.status = Status::Executing {
            current_step: 0,
            total: 1,
        };
        let outcome = execute_palette_command(&mut app, "cancel");
        assert!(outcome.handled);
        assert!(matches!(
            user_cmd_rx.try_recv().expect("Cancel"),
            UserCommand::Cancel
        ));
        assert!(
            user_cmd_rx.try_recv().is_err(),
            "must not SubmitTask skill body"
        );
    }

    #[test]
    fn colliding_skill_omitted_from_palette_list() {
        use crate::widgets::state::{SkillEntry, SlashCommand};

        let (mut app, _rx) = TestApp::new().into_commands();
        app.skills_data = vec![SkillEntry {
            name: "help".into(),
            description: "skill help".into(),
            body: "x".into(),
        }];
        let help_rows: Vec<_> = app
            .palette_commands()
            .into_iter()
            .filter(|(c, _)| c == "help")
            .collect();
        assert_eq!(help_rows.len(), 1, "builtin help only once: {help_rows:?}");
        assert_eq!(help_rows[0].1, SlashCommand::Help.desc(app.msgs()));
    }
}
