use agent_tui_kit::state::clamp_step;

use super::slash::SlashCommand;

/// Slash command autocomplete popup state, triggered by '/' at the start of
/// input or after whitespace in Insert mode.
#[derive(Debug, Clone, Default)]
pub(crate) struct SlashCommandState {
    /// Whether the slash command popup is currently active.
    pub(crate) active: bool,
    /// Byte position in `input` where '/' was typed (start of the command).
    pub(crate) start_pos: usize,
    /// Selected index in the filtered command list.
    pub(crate) selected: usize,
}

/// One row of the slash popup: a built-in, a skill, or a subcommand of a
/// built-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    /// What goes after the leading `/`, and what completing inserts
    /// (`skill list`, `plugin marketplace list`).
    pub path: String,
    /// The right-hand column: the built-in's or skill's description, or — for a
    /// subcommand — the syntax that follows it (`<server>`).
    pub detail: String,
    /// A skill rather than a built-in: the popup groups the two.
    pub is_skill: bool,
    /// More input is expected (deeper subcommands, or a value): Enter completes
    /// instead of running, and the popup survives the separating space.
    pub incomplete: bool,
}

impl Candidate {
    /// The built-in (or skill) name to dispatch on — the first path token.
    pub(crate) fn command(&self) -> &str {
        self.path.split_whitespace().next().unwrap_or_default()
    }
}

impl SlashCommandState {
    /// Extract the command query text from `input` (substring from
    /// `start_pos` to `cursor`).
    pub(crate) fn query<'a>(&self, input: &'a str, cursor: usize) -> &'a str {
        let end = cursor.min(input.len());
        if self.start_pos < end {
            &input[self.start_pos..end]
        } else {
            ""
        }
    }
}

impl super::App {
    /// The slash-popup candidates for the current input, in display order.
    ///
    /// Three shapes, decided by how far the input has got:
    ///
    /// - the command name is still being typed (`/sk`) → the built-ins,
    ///   fuzzy-matched, in palette order;
    /// - the name is complete and it is a built-in with subcommands
    ///   (`/skill `, `/plugin mar`) → that command's subcommands, filtered by
    ///   the token under the cursor — plus, for `/skill`, every installed
    ///   skill, which is where skills are offered now that they are not
    ///   first-level entries;
    /// - anything else (`/code-reviewer fix`, `/mcp auth figma`) → nothing: the
    ///   rest is arguments, and there is nothing left to complete.
    pub(crate) fn slash_candidates(&self) -> Vec<Candidate> {
        let query = self.slash_command.query(&self.input, self.input_cursor);
        let Some(body) = query.strip_prefix('/') else {
            return Vec::new();
        };
        // A trailing space means the last token is finished, so the cursor sits
        // at the start of a fresh one.
        let ends_with_space = body.ends_with(char::is_whitespace);
        let tokens: Vec<&str> = body.split_whitespace().collect();
        let Some(&first) = tokens.first() else {
            return self.command_candidates(None);
        };
        if tokens.len() == 1 && !ends_with_space {
            return self.command_candidates(Some(first));
        }
        let Some(command) = SlashCommand::from_name(first) else {
            // A skill (or a typo) with arguments: nothing to complete.
            return Vec::new();
        };
        let (completed, partial) = if ends_with_space {
            (&tokens[1..], "")
        } else {
            (&tokens[1..tokens.len() - 1], tokens[tokens.len() - 1])
        };
        self.subcommand_candidates(command, completed, partial)
    }

    /// Built-ins, already filtered by the account channel exactly as the
    /// palette filters them. `partial` is the command name being typed; `None`
    /// lists everything.
    ///
    /// Skills are not in here — they are offered under `/skill` (see
    /// [`subcommand_candidates`]), so a wall of skill names cannot bury the
    /// first-level commands.
    fn command_candidates(&self, partial: Option<&str>) -> Vec<Candidate> {
        let mut scored: Vec<(i32, usize, Candidate)> = self
            .palette_commands()
            .into_iter()
            .enumerate()
            .filter_map(|(index, (name, desc))| {
                let score = match partial {
                    None => 100,
                    Some(query) => fuzzy_score(&name, query).max(fuzzy_score(&desc, query)),
                };
                (score > 0).then(|| {
                    let incomplete =
                        SlashCommand::from_name(&name).is_some_and(SlashCommand::needs_args);
                    (
                        score,
                        index,
                        Candidate {
                            path: name,
                            detail: desc,
                            is_skill: false,
                            incomplete,
                        },
                    )
                })
            })
            .collect();
        if partial.is_some() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        }
        scored.into_iter().map(|(_, _, c)| c).collect()
    }

    /// The subcommands reachable from `completed` (already-typed whole tokens),
    /// filtered by the token currently being typed.
    ///
    /// Empty when the path runs into a value (`/mcp auth figma`), an unknown
    /// token, or a leaf: in all three cases there is nothing to offer, and the
    /// popup closes rather than showing a "no match" box over an argument.
    ///
    /// `/skill` is the one command whose children are not all static: every
    /// installed skill is one too, which is how skills reach the popup without
    /// being first-level entries.
    fn subcommand_candidates(
        &self,
        command: SlashCommand,
        completed: &[&str],
        partial: &str,
    ) -> Vec<Candidate> {
        let mut node = command.subcommands();
        // The path the user has already typed, echoed back verbatim so
        // completing a nested subcommand replaces the whole span, not just its
        // last token.
        let mut path = command.name().to_string();
        let mut at_root = true;
        for token in completed {
            // A skill is a child of `/skill` and a leaf: `/skill demo fix auth`
            // is an invocation with args, not a deeper path, and an unknown
            // token has nothing under it either — both end the walk.
            let Some(next) = node.iter().find(|sub| sub.name == *token) else {
                return Vec::new();
            };
            node = next.children;
            path.push(' ');
            path.push_str(token);
            at_root = false;
        }

        let mut scored: Vec<(i32, Candidate)> = node
            .iter()
            .filter_map(|sub| {
                let score = if partial.is_empty() {
                    100
                } else {
                    fuzzy_score(sub.name, partial)
                };
                (score > 0).then(|| {
                    (
                        score,
                        Candidate {
                            path: format!("{path} {}", sub.name),
                            detail: sub.hint.to_string(),
                            is_skill: false,
                            incomplete: sub.takes_value || !sub.children.is_empty(),
                        },
                    )
                })
            })
            .collect();
        // Declaration order is the tie-break, which is why this sort is stable.
        if !partial.is_empty() {
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        }
        let mut candidates: Vec<Candidate> =
            scored.into_iter().map(|(_, candidate)| candidate).collect();

        if at_root && command == SlashCommand::Skill {
            candidates.extend(self.skill_candidates(partial));
        }
        candidates
    }

    /// Every installed skill as a `/skill <name>` candidate, name order,
    /// filtered by the token being typed.
    ///
    /// A skill named like one of the command's own subcommands is dropped:
    /// `/skill list` is the listing, and a second row with that same path would
    /// be indistinguishable from it — the built-in keeps its name, the same
    /// rule that always kept a skill named `help` from shadowing `/help`. Such
    /// a skill is still reachable through the direct `/{name}` form.
    fn skill_candidates(&self, partial: &str) -> Vec<Candidate> {
        let declared = SlashCommand::Skill.subcommands();
        let mut skills: Vec<&crate::widgets::state::SkillEntry> = self
            .skills_data
            .iter()
            .filter(|skill| {
                !declared.iter().any(|sub| sub.name == skill.name)
                    && (partial.is_empty() || fuzzy_score(&skill.name, partial) > 0)
            })
            .collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        skills
            .into_iter()
            .map(|skill| Candidate {
                path: format!("skill {}", skill.name),
                detail: skill.description.clone(),
                is_skill: true,
                // A skill runs when picked; optional args are typed, not
                // completed, so Enter does not stop at the name.
                incomplete: false,
            })
            .collect()
    }
}

/// Simple fuzzy match scoring.
///
/// Returns a score > 0 if every character of `query` appears in `target` in
/// order (case-insensitive), with bonuses for:
///   - exact prefix match
///   - consecutive character matches
///   - matches at word boundaries
pub(crate) fn fuzzy_score(target: &str, query: &str) -> i32 {
    let target = target.to_lowercase();
    let query = query.to_lowercase();
    let target_chars: Vec<char> = target.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();

    if query_chars.is_empty() {
        return 100;
    }
    if query_chars.len() > target_chars.len() {
        return 0;
    }

    let mut score: i32 = 0;
    let mut q_idx = 0;
    let mut consecutive: i32 = 0;
    let mut prev_match: Option<usize> = None;

    for (t_idx, &tc) in target_chars.iter().enumerate() {
        if q_idx >= query_chars.len() {
            break;
        }
        if tc == query_chars[q_idx] {
            q_idx += 1;
            // Base score per match
            score += 1;
            // Consecutive bonus
            if let Some(prev) = prev_match {
                if t_idx == prev + 1 {
                    consecutive += 1;
                    score += consecutive;
                } else {
                    consecutive = 0;
                }
            }
            // Prefix bonus
            if t_idx == 0 && q_idx == 1 {
                score += 10;
            }
            // Word boundary bonus (after '_' or uppercase→lowercase transition)
            if t_idx == 0 || {
                let prev_c = target_chars[t_idx - 1];
                prev_c == '_'
                    || prev_c == '-'
                    || prev_c == ' '
                    || (prev_c.is_lowercase() && tc.is_uppercase())
            } {
                score += 3;
            }
            prev_match = Some(t_idx);
        }
    }

    // Must match all query chars
    if q_idx < query_chars.len() { 0 } else { score }
}

impl super::App {
    /// Move the slash popup selection by `delta` items (negative = up),
    /// clamped to the current match list. No-op when the popup is inactive or
    /// nothing matches. Shared by the Up/Down key handler and mouse-wheel
    /// scrolling.
    pub(crate) fn step_slash_selection(&mut self, delta: i32) {
        if !self.slash_command.active {
            return;
        }
        let n = self.slash_candidates().len();
        if n == 0 {
            return;
        }
        self.slash_command.selected = clamp_step(n, self.slash_command.selected, delta);
    }
}

#[cfg(test)]
mod tests {
    use crate::render::test_harness::make_app;
    use crate::widgets::state::{App, InputMode, SkillEntry};

    /// An app mid-typing: `input` is in the box with the popup open and the
    /// cursor at the end, which is the state the completion reads.
    ///
    /// No skills: tests that need them seed `skills_data` themselves, because
    /// what is offered where is exactly what most of these tests assert.
    fn app_typing(input: &str) -> App {
        let mut app = make_app();
        app.input_mode = InputMode::Insert;
        app.input = input.to_string();
        app.input_cursor = input.len();
        app.slash_command.active = true;
        app.slash_command.start_pos = 0;
        app.slash_command.selected = 0;
        app
    }

    fn paths(app: &App) -> Vec<String> {
        app.slash_candidates()
            .into_iter()
            .map(|candidate| candidate.path)
            .collect()
    }

    #[test]
    fn a_trailing_space_offers_the_subcommands_of_a_builtin() {
        assert_eq!(
            paths(&app_typing("/skill ")),
            ["skill list", "skill reload"]
        );
        assert_eq!(
            paths(&app_typing("/mcp ")),
            [
                "mcp auth",
                "mcp login",
                "mcp list",
                "mcp prompts",
                "mcp prompt",
            ]
        );
    }

    #[test]
    fn a_partial_subcommand_is_the_only_match() {
        assert_eq!(paths(&app_typing("/skill li")), ["skill list"]);
        assert_eq!(paths(&app_typing("/plugin mar")), ["plugin marketplace"]);
        // Flags complete too: hooks takes `--all` / `--source <label>`.
        assert_eq!(
            paths(&app_typing("/hooks trust --")),
            ["hooks trust --all", "hooks trust --source"]
        );
    }

    #[test]
    fn subcommands_nest() {
        // `/plugin marketplace` has its own child, so the walk continues.
        assert_eq!(
            paths(&app_typing("/plugin marketplace ")),
            ["plugin marketplace list"]
        );
        assert_eq!(
            paths(&app_typing("/plugin marketplace l")),
            ["plugin marketplace list"]
        );
    }

    #[test]
    fn a_subcommand_row_carries_its_syntax_and_whether_more_is_expected() {
        let candidates = app_typing("/mcp ").slash_candidates();
        let auth = candidates
            .iter()
            .find(|candidate| candidate.path == "mcp auth")
            .expect("auth is offered");
        assert_eq!(auth.detail, "<server>", "the popup shows what to type next");
        assert!(auth.incomplete, "`/mcp auth` still needs a value");
        assert!(!auth.is_skill);

        let list = candidates
            .iter()
            .find(|candidate| candidate.path == "mcp list")
            .expect("list is offered");
        assert!(list.detail.is_empty());
        assert!(!list.incomplete, "`/mcp list` is complete and runnable");
        assert_eq!(list.command(), "mcp");
    }

    #[test]
    fn nothing_is_offered_once_the_input_is_an_argument() {
        // A value (`<server>`), a skill's args, and an unknown subcommand all
        // stop the completion: none of them has a token list to walk.
        for input in ["/mcp auth figma", "/code-reviewer fix auth", "/mcp nope "] {
            assert!(
                paths(&app_typing(input)).is_empty(),
                "{input} must offer nothing"
            );
        }
    }

    #[test]
    fn a_command_without_subcommands_stops_at_the_command() {
        // `/cancel` (and any skill) takes no subcommands: the space closes the
        // popup rather than offering something invented.
        assert!(paths(&app_typing("/cancel ")).is_empty());
    }

    #[test]
    fn still_typing_the_name_matches_commands() {
        let mut app = app_typing("/sk");
        app.skills_data = vec![SkillEntry {
            name: "skill-notes".into(),
            description: "Notes skill".into(),
            body: String::new(),
        }];

        let candidates = app.slash_candidates();

        assert!(
            candidates.iter().any(|candidate| candidate.path == "skill"),
            "{candidates:?}"
        );
        // The skill must NOT be here: it used to be a first-level entry, and a
        // wall of them buried exactly this kind of match.
        assert!(
            !candidates
                .iter()
                .any(|candidate| candidate.path == "skill-notes"),
            "{candidates:?}"
        );
    }

    #[test]
    fn the_first_level_is_builtins_only() {
        let mut app = app_typing("/");
        app.skills_data = (0..30)
            .map(|i| SkillEntry {
                name: format!("skill-{i:02}"),
                description: format!("Skill {i}"),
                body: String::new(),
            })
            .collect();

        let candidates = app.slash_candidates();

        assert_eq!(
            candidates.len(),
            app.palette_commands().len(),
            "the first level lists the built-ins and nothing else: {candidates:?}"
        );
        assert!(candidates.iter().all(|candidate| !candidate.is_skill));
    }

    fn seeded_skills() -> Vec<SkillEntry> {
        vec![
            SkillEntry {
                name: "code-reviewer".into(),
                description: "代码审查专家".into(),
                body: String::new(),
            },
            SkillEntry {
                name: "demo".into(),
                description: "Demo skill".into(),
                body: String::new(),
            },
        ]
    }

    #[test]
    fn skills_are_offered_under_the_skill_command() {
        let mut app = app_typing("/skill ");
        app.skills_data = seeded_skills();

        let candidates = app.slash_candidates();

        // Built-in subcommands first, then the skills by name.
        let paths: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.path.as_str())
            .collect();
        assert_eq!(
            paths,
            [
                "skill list",
                "skill reload",
                "skill code-reviewer",
                "skill demo"
            ]
        );
        let reviewer = candidates
            .iter()
            .find(|candidate| candidate.path == "skill code-reviewer")
            .expect("skill offered");
        assert!(reviewer.is_skill);
        assert_eq!(reviewer.detail, "代码审查专家");
        assert!(
            !reviewer.incomplete,
            "picking a skill runs it; args are typed, not completed"
        );

        // Filtering works on the skill name, and `list`/`reload` win a
        // collision (a skill literally named `list` is never reachable here).
        let mut partial = app_typing("/skill c");
        partial.skills_data = seeded_skills();
        let filtered: Vec<String> = partial
            .slash_candidates()
            .into_iter()
            .map(|candidate| candidate.path)
            .collect();
        assert_eq!(filtered, ["skill code-reviewer"]);

        let colliding = {
            let mut app = app_typing("/skill li");
            app.skills_data = vec![SkillEntry {
                name: "list".into(),
                description: "shadowed".into(),
                body: String::new(),
            }];
            app.slash_candidates()
        };
        assert_eq!(
            colliding
                .iter()
                .map(|candidate| candidate.path.as_str())
                .collect::<Vec<_>>(),
            ["skill list"],
            "the built-in subcommand keeps its name"
        );
    }

    /// An app with `count` skills, on `/skill ` — the level that carries the
    /// long list, now that skills are not first-level entries.
    fn open_app_with_skills(count: usize) -> App {
        let mut app = make_app();
        app.input_mode = InputMode::Insert;
        app.input = "/skill ".into();
        app.input_cursor = app.input.len();
        app.slash_command.active = true;
        app.slash_command.start_pos = 0;
        app.slash_command.selected = 0;
        app.skills_data = (0..count)
            .map(|i| SkillEntry {
                name: format!("skill-{i:02}"),
                description: format!("Skill number {i} description"),
                body: String::new(),
            })
            .collect();
        app
    }

    #[test]
    fn step_slash_selection_moves_and_clamps() {
        let mut app = open_app_with_skills(40);
        // The `/skill` subcommands plus 40 skills. Derived, not hardcoded: the
        // declaration grows every time a subcommand is added, and a magic index
        // turns that into a confusing off-by-one failure here.
        let last = app.slash_candidates().len() - 1;
        assert!(
            last > 40,
            "40 skills plus the subcommands must be selectable"
        );

        app.step_slash_selection(1);
        assert_eq!(app.slash_command.selected, 1);
        app.step_slash_selection(-1);
        assert_eq!(app.slash_command.selected, 0);

        // Clamp at the bottom.
        app.slash_command.selected = last;
        app.step_slash_selection(1);
        assert_eq!(app.slash_command.selected, last);
        app.step_slash_selection(-1);
        assert_eq!(app.slash_command.selected, last - 1);
    }

    #[test]
    fn step_slash_selection_noop_when_inactive_or_no_match() {
        let mut app = open_app_with_skills(40);
        app.slash_command.active = false;
        app.slash_command.selected = 5;
        app.step_slash_selection(1);
        assert_eq!(
            app.slash_command.selected, 5,
            "inactive popup must not move"
        );

        let mut app = open_app_with_skills(40);
        app.input = "/zzz-no-match".into();
        app.input_cursor = app.input.len();
        app.slash_command.selected = 5;
        app.step_slash_selection(1);
        assert_eq!(app.slash_command.selected, 5, "no match must not move");
    }
}
