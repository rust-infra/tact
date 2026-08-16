// Input handlers — split by mode.
mod file_picker;
mod insert;
mod mouse;
mod normal;
mod overlay;
mod palette;
mod plugin;
mod select;
mod skills;

use chrono::Local;
pub(crate) use file_picker::handle_file_picker_mode;
pub(crate) use insert::{handle_insert_mode, insert_transcript};
pub(crate) use mouse::handle_mouse_event;
pub(crate) use normal::handle_normal_mode;
pub(crate) use overlay::handle_overlay_key;
pub(crate) use palette::handle_palette_mode;
pub(crate) use select::handle_select_mode;
use tact_protocol::UserCommand;

use crate::widgets::state::{App, InputMode, SelectKind, Status};

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

/// True when `cmd` is a built-in palette entry (wins over same-named skills).
pub(crate) fn is_builtin_palette_command(cmd: &str) -> bool {
    crate::widgets::state::PALETTE_COMMANDS
        .iter()
        .any(|(name, _)| *name == cmd)
}

/// Built-ins that take a subcommand / arguments: Enter should autocomplete
/// `/{cmd} ` into the insert box instead of executing immediately.
pub(crate) fn command_needs_args(cmd: &str) -> bool {
    matches!(cmd, "plugin")
}

pub(crate) fn execute_palette_command(app: &mut App, cmd: &str) -> CommandExecOutcome {
    // Built-ins always win so a skill named `cancel`/`help`/… cannot shadow them.
    if !is_builtin_palette_command(cmd)
        && let Some(outcome) = skills::handle_skill_command(app, cmd)
    {
        return outcome;
    }

    match cmd {
        "theme" => {
            app.toggle_theme();
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "model" => {
            crate::handlers::select::start_model_picker(app);
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "model-subagent" => {
            crate::handlers::select::start_subagent_model_picker(app);
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "permission" => {
            start_permission_picker(app);
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "view-system-prompt" => {
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
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "save" => {
            let timestamp = Local::now().format("%Y%m%d_%H%M%S");
            let path = std::env::temp_dir().join(format!("agent_log_{timestamp}.txt"));
            if let Ok(mut file) = std::fs::File::create(&path) {
                use std::io::Write;
                for msg in &app.raw_messages {
                    writeln!(file, "{}", msg).ok();
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
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "quit" => {
            app.should_quit = true;
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "help" => {
            app.show_help = !app.show_help;
            app.show_history = false;
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "history" => {
            app.show_history = !app.show_history;
            app.show_help = false;
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "skills" => {
            show_skills_command(app);
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "skill-reload" => {
            match refresh_skills(app) {
                Ok(count) => {
                    let msg = app
                        .msgs()
                        .skill_reloaded_tmpl
                        .replace("{}", &count.to_string());
                    app.add_system_message(msg);
                }
                Err(err) => {
                    let msg = app.msgs().skill_reload_failed_tmpl.replace("{}", &err);
                    app.add_system_message(msg);
                }
            }
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "plugin" => plugin::handle_plugin_command(app),
        "cancel" => {
            // Only cancel an in-flight task; Idle and Done have nothing to abort.
            if matches!(app.status, Status::Planning | Status::Executing { .. }) {
                let _ = app.user_cmd_tx.send(UserCommand::Cancel);
            } else {
                app.flash_msg = Some((
                    app.msgs().cancel_noop_msg.to_string(),
                    std::time::Instant::now(),
                ));
            }
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "compact" => {
            // Only compact when idle; active tasks cannot be compacted
            // from slash since compact_history synchronously rewrites
            // the session context.
            if matches!(app.status, Status::Planning | Status::Executing { .. }) {
                let msg = app.msgs().input_busy_msg.to_string();
                app.flash_msg = Some((msg, std::time::Instant::now()));
            } else {
                let _ = app.user_cmd_tx.send(UserCommand::Compact);
            }
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "balance" => {
            if app.account_rx.is_none() {
                return CommandExecOutcome {
                    handled: true,
                    clear_input: true,
                };
            }
            let _ = app.user_cmd_tx.send(UserCommand::QueryBalance);
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "stats" => {
            let _ = app.user_cmd_tx.send(UserCommand::QueryStats);
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "tasks-dag" => {
            app.open_task_dag_popup();
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "background" => {
            // `/background` lists all background tasks; `/background <id>`
            // shows one task (pretty JSON). The optional id comes from the
            // remaining input after the command token.
            let task_id = app.input.split_whitespace().nth(1).map(str::to_string);
            let _ = app.user_cmd_tx.send(UserCommand::QueryBackground(task_id));
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "lang" => {
            app.toggle_language();
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        _ => CommandExecOutcome {
            handled: false,
            clear_input: false,
        },
    }
}

/// `/skills` output is plain markdown (heading + pipe table) appended as
/// whole-Markdown messages and rendered by `MarkdownCell`. Long lists are
/// split into pages of [`SKILLS_PER_PAGE`] rows so each message stays
/// readable and scrollable instead of one giant table.
const SKILLS_PER_PAGE: usize = 15;

fn show_skills_command(app: &mut App) {
    app.add_new_line();

    let chunks = skills_list_markdown_pages(&app.skills_data, SKILLS_PER_PAGE);
    if chunks.is_empty() {
        app.append_markdown("(no skills available)".to_string());
    } else {
        for (i, chunk) in chunks.into_iter().enumerate() {
            if i > 0 {
                app.add_new_line();
            }
            app.append_markdown(chunk);
        }
    }

    // Trailing blank so the next `/skills` (or other system block) is not flush.
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
pub(crate) fn refresh_skills(app: &mut App) -> Result<usize, String> {
    let mut reg = tact::skill::lock_skills(&app.skill_registry);
    // Keep search roots in sync with the current workdir (tests may set work_dir late).
    *reg = tact::skill::get_skill_registry(&app.work_dir).map_err(|e| e.to_string())?;
    app.skills_description = reg.describe_available();
    app.skills_data = reg
        .skills()
        .values()
        .map(|doc| crate::widgets::state::SkillEntry {
            name: doc.manifest.name.clone(),
            description: doc.manifest.description.clone(),
            body: doc.body.clone(),
        })
        .collect();
    // Skill list affects log highlighting; force visual-cache rebuild.
    app.log_scroll.visual_cache_ver = 0;
    Ok(app.skills_data.len())
}

/// Open the `/permission` SelectPopup from palette / slash command.
pub(crate) fn start_permission_picker(app: &mut App) {
    let msgs = app.msgs();
    let options = vec![
        msgs.permission_option_default.to_string(),
        msgs.permission_option_plan.to_string(),
        msgs.permission_option_auto.to_string(),
    ];
    let current = match app.status_bar.permission_mode.as_str() {
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
    use std::path::PathBuf;

    use tact_protocol::{AgentUpdate, UserCommand};
    use tokio::sync::mpsc::unbounded_channel;

    use super::{execute_palette_command, skills_list_markdown};
    use crate::widgets::state::{App, Status};

    fn make_app() -> (App, tokio::sync::mpsc::UnboundedReceiver<UserCommand>) {
        let (agent_tx, agent_rx) = unbounded_channel::<AgentUpdate>();
        let (user_cmd_tx, user_cmd_rx) = unbounded_channel::<UserCommand>();
        let (plugin_tx, _plugin_request_rx) = unbounded_channel();
        let (_plugin_event_tx, plugin_rx) = unbounded_channel();
        let (history_tx, _history_rx) = unbounded_channel::<(String, String)>();
        drop(agent_tx);
        let app = App::new(
            agent_rx,
            None,
            plugin_rx,
            plugin_tx,
            user_cmd_tx.clone(),
            PathBuf::from("."),
            Vec::new(),
            "test-session".to_string(),
            history_tx,
            "retro".to_string(),
            String::new(),
            Vec::new(),
        );
        (app, user_cmd_rx)
    }

    #[test]
    fn palette_commands_are_all_handled() {
        let (mut app, _user_cmd_rx) = make_app();
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
        let (mut app, _user_cmd_rx) = make_app();
        let outcome = execute_palette_command(&mut app, "nonexistent");
        assert!(!outcome.handled);
        assert!(!outcome.clear_input);
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

    #[test]
    fn skills_command_adds_separators_around_list() {
        let (mut app, _rx) = make_app();
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
        let before = app.raw_messages.len();
        execute_palette_command(&mut app, "skills");
        let after_first = app.raw_messages.len();
        assert!(after_first > before);
        assert!(
            app.raw_messages
                .iter()
                .any(|m| m.contains("code-reviewer") || m.contains("Available skills")),
            "expected skills content, got: {:?}",
            app.raw_messages
        );
        // Second invocation must not glue flush to the previous block.
        execute_palette_command(&mut app, "skills");
        let joined = app.raw_messages[after_first.saturating_sub(1)..].join("\n");
        assert!(
            app.raw_messages[after_first - 1].is_empty()
                || app
                    .raw_messages
                    .get(after_first)
                    .is_some_and(|s| s.is_empty()),
            "expected blank separator between skills blocks, around: {joined}"
        );
    }

    #[test]
    fn skills_command_paginates_long_lists() {
        let (mut app, _rx) = make_app();
        app.skills_data = (0..40)
            .map(|i| crate::widgets::state::SkillEntry {
                name: format!("skill-{i:02}"),
                description: "d".into(),
                body: String::new(),
            })
            .collect();
        let before = app.raw_messages.len();
        execute_palette_command(&mut app, "skills");
        let raw = &app.raw_messages[before..];

        // 40 skills / 15 per page → 3 pages with numbered headings.
        for page in ["(1/3)", "(2/3)", "(3/3)"] {
            assert!(
                raw.iter()
                    .any(|m| m.contains(&format!("Available skills {page}"))),
                "missing page heading {page}, got: {:?}",
                raw
            );
        }
        // Page boundaries: 0-14, 15-29, 30-39.
        for name in [
            "skill-00", "skill-14", "skill-15", "skill-29", "skill-30", "skill-39",
        ] {
            assert!(
                raw.iter().any(|m| m.contains(name)),
                "missing skill {name}, got: {:?}",
                raw
            );
        }
    }

    #[test]
    fn skills_command_output_reaches_middle_skills_when_scrolling() {
        // Regression for the reported symptom: with ~60 skills the old
        // logical-row scrolling never showed alphabetically-middle entries
        // (lark-*) of the `/skills` table. Paginated pages + visual stepping
        // must make every row reachable.
        let (mut app, _rx) = make_app();
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
        execute_palette_command(&mut app, "skills");
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
            "lark-doc never visible while traversing /skills output"
        );
    }

    #[test]
    fn cancel_while_done_is_noop() {
        let (mut app, mut user_cmd_rx) = make_app();
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
        let (mut app, mut user_cmd_rx) = make_app();
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
        let (mut app, mut user_cmd_rx) = make_app();
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
        let (mut app, mut user_cmd_rx) = make_app();
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

    #[test]
    fn theme_command_toggles_theme() {
        use crate::theme::ThemeName;

        let (mut app, _user_cmd_rx) = make_app();
        assert_eq!(app.theme.name, ThemeName::Retro);
        let outcome = execute_palette_command(&mut app, "theme");
        assert!(outcome.handled);
        assert_ne!(app.theme.name, ThemeName::Retro);
    }

    #[test]
    fn builtin_command_wins_over_same_named_skill() {
        use crate::widgets::state::SkillEntry;

        let (mut app, mut user_cmd_rx) = make_app();
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
        use crate::widgets::state::SkillEntry;

        let (mut app, _rx) = make_app();
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
        assert_eq!(help_rows[0].1, app.localize_cmd_desc("help"));
    }
}
