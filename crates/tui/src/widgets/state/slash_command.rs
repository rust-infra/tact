use agent_tui_kit::state::clamp_step;
use tact_protocol::{GoalCriterionStatusSnapshot, GoalStatusSnapshot};

use super::slash::{SlashCommand, Subcommand};

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
    /// - the walk landed on a verb that takes a value (`/skill disable `,
    ///   `/goal accept `) → the values that verb can act on — skills, or the
    ///   Goal ids from the live panel — so the value is picked rather than
    ///   typed;
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
    /// being first-level entries. Two commands complete a *value* as well —
    /// `/skill disable|enable <name>` and the `/goal <verb> <goal-id>` verbs —
    /// see [`Self::value_candidates`].
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
        // The subcommand the walk landed on, when it expects a value. Only
        // `/skill disable|enable` and the id-taking `/goal` verbs qualify, and
        // they are how a *value* gets completed instead of typed blind.
        let mut value_sub: Option<&Subcommand> = None;
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
            value_sub = next.takes_value.then_some(next);
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
        } else if let Some(sub) = value_sub {
            candidates.extend(self.value_candidates(command, &path, sub.name, partial));
        }
        candidates
    }

    /// The values a verb can act on, when its argument is picked rather than
    /// typed.
    ///
    /// Two commands complete a value and each filters its own list; anything
    /// else answers nothing, which is the same "there is nothing left to
    /// complete" answer `/mcp auth figma` gets.
    fn value_candidates(
        &self,
        command: SlashCommand,
        path: &str,
        verb: &str,
        partial: &str,
    ) -> Vec<Candidate> {
        match command {
            SlashCommand::Skill => self.skill_verb_candidates(path, verb, partial),
            SlashCommand::Goal => self.goal_verb_candidates(path, verb, partial),
            _ => Vec::new(),
        }
    }

    /// The Goals a `/goal <verb>` can act on, as `<path> <goal-id>` rows.
    ///
    /// The id is the one thing about a Goal that is *not* on screen — the
    /// panel row shows the title and hides the id, and the log line that
    /// announced it may be pages up — so every id-taking verb gets the live
    /// panel snapshot as its candidate list, and a row is picked instead of
    /// copied.
    ///
    /// Only the rows the verb can actually act on are offered, and the filter
    /// is the store's transition table (`goal::is_legal_transition`) rather
    /// than a guess:
    ///
    /// - `show` is a read, so every Goal qualifies, terminal ones included;
    /// - `pause` is legal from `queued` / `running` / `waiting_user` /
    ///   `waiting_permission` / `verifying` — `draft` and an already-`paused`
    ///   Goal would only get `InvalidTransition`;
    /// - `cancel` and `steer` take any non-terminal Goal (a terminal one never
    ///   runs again, so there would be nothing left to steer);
    /// - `resume` is the driver's own three waiting states;
    /// - `accept` needs `waiting_user` *and* a pending required human criterion
    ///   — otherwise the driver can only answer "no pending required human
    ///   criterion".
    fn goal_verb_candidates(&self, path: &str, verb: &str, partial: &str) -> Vec<Candidate> {
        use GoalStatusSnapshot::*;

        let actionable: fn(&tact_protocol::GoalSnapshot) -> bool = match verb {
            "show" => |_| true,
            "pause" => |goal| {
                matches!(
                    goal.status,
                    Queued | Running | WaitingUser | WaitingPermission | Verifying
                )
            },
            "cancel" | "steer" => |goal| !goal.status.is_terminal(),
            // The driver also advances `draft` / `queued` and resumes a
            // stranded `running` Goal with no active run; hiding those rows
            // made the documented "continue an idle Running Goal" action
            // undiscoverable.
            "resume" => |goal| {
                matches!(
                    goal.status,
                    Draft | Queued | Paused | WaitingUser | WaitingPermission
                ) || (goal.status == Running && goal.run_budget.is_none())
            },
            // The filing pair: only a finished Goal can be filed away, and only
            // a filed one can come back — a row that would only answer "already
            // archived" is not a candidate (the same rule `disable` / `enable`
            // follow for skills).
            "archive" => |goal| goal.status.is_terminal() && !goal.archived,
            "unarchive" => |goal| goal.archived,
            "accept" => |goal| {
                // `waiting_user` is the normal state. A `paused` Goal (user
                // pause or crash recovery) and a `running` Goal with no active
                // run are both accepted by the driver, so offer them rather
                // than hiding the only actionable row. Automatic criteria are
                // reserved for creation, but legacy rows are closed by the
                // same explicit human acceptance.
                (matches!(goal.status, WaitingUser | Paused)
                    || (goal.status == Running && goal.run_budget.is_none()))
                    && goal.criteria.iter().any(|criterion| {
                        criterion.required
                            && criterion.status != GoalCriterionStatusSnapshot::Passed
                    })
            },
            _ => return Vec::new(),
        };
        // `steer` and `accept` take the instruction after the id, so the row is
        // not runnable until that text is typed; the rest take the id alone.
        let incomplete = matches!(verb, "steer" | "accept");

        self.goal_panel()
            .snapshot
            .iter()
            .filter(|goal| actionable(goal))
            .filter(|goal| {
                partial.is_empty()
                    || fuzzy_score(&goal.id, partial) > 0
                    || fuzzy_score(&goal.title, partial) > 0
            })
            .map(|goal| Candidate {
                path: format!("{path} {}", goal.id),
                // The panel's own marker + title, so a row picked here reads
                // like the row it came from.
                detail: format!("{} {}", goal.status.marker(), goal.title),
                is_skill: false,
                incomplete,
            })
            .collect()
    }

    /// The skills a `/skill disable|enable <name>` verb can act on, as
    /// `<path> <name>` candidates — so the verb is picked from a list instead
    /// of typed blind.
    ///
    /// Only the actionable ones: `disable` lists what is **in** the catalog and
    /// `enable` what is out of it. Offering a skill that is already in the state
    /// being asked for would be a row whose only outcome is the "already hidden"
    /// message.
    fn skill_verb_candidates(&self, path: &str, verb: &str, partial: &str) -> Vec<Candidate> {
        // An unrecognised verb offers nothing rather than the wrong half of the
        // list — the two arms are the declared pair, and a third one added later
        // must decide its own filter here.
        let actionable_disabled = match verb {
            "disable" => false,
            "enable" => true,
            _ => return Vec::new(),
        };

        let mut skills: Vec<&crate::widgets::state::SkillEntry> = self
            .skills_data
            .iter()
            .filter(|skill| skill.disabled == actionable_disabled)
            .filter(|skill| partial.is_empty() || fuzzy_score(&skill.name, partial) > 0)
            .collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        skills
            .into_iter()
            .map(|skill| Candidate {
                path: format!("{path} {}", skill.name),
                detail: skill.description.clone(),
                is_skill: true,
                // The name is the leaf: Enter runs the verb on the highlighted
                // skill, which is the whole point of listing them.
                incomplete: false,
            })
            .collect()
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
    tact_catalog::fuzzy_score(query, target).unwrap_or(0)
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
            [
                "skill list",
                "skill reload",
                "skill disable",
                "skill enable"
            ]
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
            [
                "plugin marketplace list",
                "plugin marketplace add",
                "plugin marketplace update",
                "plugin marketplace remove",
            ]
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

            dir: String::new(),
            disabled: false,
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

    /// `/skill disable ` offers the skills that are **in** the catalog, so the
    /// verb is picked from a list rather than typed blind.
    #[test]
    fn the_disable_verb_offers_the_skills_that_are_in_the_catalog() {
        let mut app = app_typing("/skill disable ");
        app.skills_data = seeded_skills();

        assert_eq!(
            paths(&app),
            ["skill disable code-reviewer", "skill disable demo"]
        );
    }

    /// `/skill enable ` offers the other half. A row for a skill already in the
    /// catalog would only ever answer "already in the catalog".
    #[test]
    fn the_enable_verb_offers_only_what_is_out_of_the_catalog() {
        let mut app = app_typing("/skill enable ");
        let mut skills = seeded_skills();
        skills[1].disabled = true;
        app.skills_data = skills;

        assert_eq!(paths(&app), ["skill enable demo"]);
    }

    /// The token under the cursor filters the list, through the same fuzzy
    /// match every other candidate uses.
    #[test]
    fn the_disable_verb_filters_by_the_token_being_typed() {
        let mut app = app_typing("/skill disable code");
        app.skills_data = seeded_skills();

        assert_eq!(paths(&app), ["skill disable code-reviewer"]);
    }

    /// Nothing left to act on closes the popup rather than showing an empty box
    /// over an argument.
    #[test]
    fn the_enable_verb_offers_nothing_when_the_catalog_is_whole() {
        let mut app = app_typing("/skill enable ");
        app.skills_data = seeded_skills();

        assert!(paths(&app).is_empty());
    }

    /// A Goal id is the one thing about a Goal that is nowhere on screen (the
    /// panel row shows the title), so every id-taking verb offers the ids from
    /// the live panel snapshot.
    #[test]
    fn a_goal_verb_offers_the_ids_from_the_panel() {
        let mut app = app_typing("/goal show ");
        app.goal_panel_mut().apply_snapshot(seeded_goals());

        let candidates = app.slash_candidates();
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.path.as_str())
                .collect::<Vec<_>>(),
            [
                "goal show 216ae6be",
                "goal show 03a4caef",
                "goal show aaaa1111",
                "goal show bbbb2222",
            ],
            "`show` is a read: every Goal in the panel is a candidate"
        );
        assert_eq!(
            candidates[0].detail, "▶ review code",
            "the row carries the panel's own marker and title"
        );
        assert!(
            !candidates[0].incomplete,
            "`/goal show <id>` is complete: Enter runs it"
        );
    }

    /// `steer` and `accept` take an instruction after the id, so their rows
    /// complete rather than run — the same rule `/mcp auth <server>` follows.
    #[test]
    fn the_verbs_that_take_text_after_the_id_only_complete() {
        let mut app = app_typing("/goal accept ");
        app.goal_panel_mut().apply_snapshot(seeded_goals());

        let candidates = app.slash_candidates();
        assert_eq!(
            candidates.len(),
            2,
            "the waiting and the paused Goal (both are accepted by the driver): {candidates:?}"
        );
        assert!(
            candidates.iter().all(|candidate| candidate.incomplete),
            "the instruction still has to be typed"
        );
    }

    /// A message sent while the Goal was `waiting_user` can wake the runner
    /// and leave it `running` with no active run. The driver accepts that
    /// stranded state, so `/goal accept` must offer it instead of hiding the
    /// only actionable row.
    #[test]
    fn accept_offers_a_running_goal_with_no_active_run() {
        use tact_protocol::{GoalCriterionSnapshot, GoalSnapshot, GoalStatusSnapshot};

        let mut app = app_typing("/goal accept ");
        app.goal_panel_mut().apply_snapshot(vec![GoalSnapshot {
            id: "9b630c0e".into(),
            title: "review code".into(),
            status: GoalStatusSnapshot::Running,
            criteria: vec![GoalCriterionSnapshot {
                id: "c1".into(),
                description: "User accepts the result".into(),
                required: true,
                human: true,
                ..Default::default()
            }],
            run_budget: None,
            ..Default::default()
        }]);

        let candidates = app.slash_candidates();
        assert_eq!(candidates.len(), 1, "{candidates:?}");
        assert_eq!(candidates[0].path, "goal accept 9b630c0e");
        assert!(candidates[0].incomplete);
    }

    /// A Goal parked in `paused` by user pause or crash recovery is still a
    /// human decision away from done: the driver accepts it, so completion
    /// must offer it.
    #[test]
    fn accept_offers_a_paused_goal() {
        use tact_protocol::{GoalCriterionSnapshot, GoalSnapshot, GoalStatusSnapshot};

        let mut app = app_typing("/goal accept ");
        app.goal_panel_mut().apply_snapshot(vec![GoalSnapshot {
            id: "9b630c0e".into(),
            title: "review code".into(),
            status: GoalStatusSnapshot::Paused,
            criteria: vec![GoalCriterionSnapshot {
                id: "c1".into(),
                description: "User accepts the result".into(),
                required: true,
                human: true,
                ..Default::default()
            }],
            run_budget: None,
            ..Default::default()
        }]);

        let candidates = app.slash_candidates();
        assert_eq!(candidates.len(), 1, "{candidates:?}");
        assert_eq!(candidates[0].path, "goal accept 9b630c0e");
        assert!(candidates[0].incomplete);
    }

    /// The token under the cursor filters the list — by id *or* title, because
    /// the panel is where the user reads the title, and the id is what the
    /// command takes.
    #[test]
    fn a_goal_row_is_matched_by_id_and_by_title() {
        let mut app = app_typing("/goal show 216a");
        app.goal_panel_mut().apply_snapshot(seeded_goals());
        assert_eq!(paths(&app), ["goal show 216ae6be"]);

        let mut app = app_typing("/goal show review");
        app.goal_panel_mut().apply_snapshot(seeded_goals());
        assert_eq!(paths(&app), ["goal show 216ae6be"]);
    }

    /// Only the statuses the store's transition table accepts are offered: a
    /// row whose only outcome is an error message is not a candidate.
    #[test]
    fn each_goal_verb_offers_only_the_goals_it_can_move() {
        let seeded = |input: &str| {
            let mut app = app_typing(input);
            app.goal_panel_mut().apply_snapshot(seeded_goals());
            app
        };

        // The fixture: `216ae6be` is `running`, `03a4caef` is `waiting_user`,
        // `aaaa1111` is `paused`, `bbbb2222` is `succeeded`.
        assert_eq!(
            paths(&seeded("/goal pause ")),
            ["goal pause 216ae6be", "goal pause 03a4caef"],
            "an already-paused Goal cannot be paused again"
        );
        assert_eq!(
            paths(&seeded("/goal resume ")),
            ["goal resume 03a4caef", "goal resume aaaa1111"],
            "resume only leaves the three paused/waiting states"
        );
        assert_eq!(
            paths(&seeded("/goal cancel ")),
            [
                "goal cancel 216ae6be",
                "goal cancel 03a4caef",
                "goal cancel aaaa1111",
            ],
            "cancel takes any non-terminal Goal"
        );
        assert_eq!(
            paths(&seeded("/goal show ")).len(),
            4,
            "show is a read: terminal and paused Goals included"
        );
    }

    #[test]
    fn resume_offers_a_stranded_running_goal_and_draft_or_queued_work() {
        use tact_protocol::{GoalSnapshot, GoalStatusSnapshot};

        let snapshot =
            |id: &str,
             status: GoalStatusSnapshot,
             run_budget: Option<tact_protocol::GoalRunBudgetSnapshot>| GoalSnapshot {
                id: id.into(),
                title: id.into(),
                status,
                run_budget,
                ..Default::default()
            };
        let mut app = app_typing("/goal resume ");
        app.goal_panel_mut().apply_snapshot(vec![
            snapshot("idle-running", GoalStatusSnapshot::Running, None),
            snapshot("draft", GoalStatusSnapshot::Draft, None),
            snapshot("queued", GoalStatusSnapshot::Queued, None),
            snapshot(
                "active-running",
                GoalStatusSnapshot::Running,
                Some(Default::default()),
            ),
        ]);

        assert_eq!(
            paths(&app),
            [
                "goal resume idle-running",
                "goal resume draft",
                "goal resume queued",
            ],
            "the documented continuation states must be offerable"
        );
    }

    #[test]
    fn pause_offers_a_verifying_goal() {
        use tact_protocol::{GoalSnapshot, GoalStatusSnapshot};

        let mut app = app_typing("/goal pause ");
        app.goal_panel_mut().apply_snapshot(vec![GoalSnapshot {
            id: "verifying".into(),
            title: "verifying".into(),
            status: GoalStatusSnapshot::Verifying,
            ..Default::default()
        }]);
        assert_eq!(paths(&app), ["goal pause verifying"]);
    }

    #[test]
    fn accept_offers_a_legacy_automatic_criterion() {
        use tact_protocol::{GoalCriterionSnapshot, GoalSnapshot, GoalStatusSnapshot};

        let mut app = app_typing("/goal accept ");
        app.goal_panel_mut().apply_snapshot(vec![GoalSnapshot {
            id: "legacy-auto".into(),
            title: "legacy".into(),
            status: GoalStatusSnapshot::WaitingUser,
            criteria: vec![GoalCriterionSnapshot {
                id: "c1".into(),
                description: "legacy automatic check".into(),
                required: true,
                human: false,
                ..Default::default()
            }],
            ..Default::default()
        }]);
        assert_eq!(paths(&app), ["goal accept legacy-auto"]);
    }

    /// `accept` is the one verb with a second gate: a Goal whose required
    /// human criterion already carries evidence has nothing left to accept
    /// (`accept_goal` would only answer "no pending required human
    /// criterion"). `waiting_user` and `paused` both pass the status gate here,
    /// so both are marked as satisfied.
    #[test]
    fn accept_skips_a_goal_that_has_nothing_pending() {
        use tact_protocol::{GoalCriterionStatusSnapshot, GoalStatusSnapshot};

        let mut goals = seeded_goals();
        for goal in goals.iter_mut().filter(|goal| {
            matches!(
                goal.status,
                GoalStatusSnapshot::WaitingUser | GoalStatusSnapshot::Paused
            )
        }) {
            goal.criteria
                .iter_mut()
                .for_each(|criterion| criterion.status = GoalCriterionStatusSnapshot::Passed);
        }

        let mut app = app_typing("/goal accept ");
        app.goal_panel_mut().apply_snapshot(goals);

        assert!(paths(&app).is_empty());
    }

    /// The filing pair is offered the other way round from every other verb:
    /// `archive` lists the finished Goals that are still on the panel, and
    /// `unarchive` lists the ones that already left it. A row on the wrong side
    /// would only ever answer "already archived" / "not archived".
    #[test]
    fn the_filing_verbs_offer_the_finished_goals_and_only_those() {
        use tact_protocol::{GoalSnapshot, GoalStatusSnapshot};

        let mut goals = seeded_goals();
        goals.push(GoalSnapshot {
            id: "cccc3333".into(),
            title: "filed away".into(),
            status: GoalStatusSnapshot::Succeeded,
            archived: true,
            ..Default::default()
        });

        let seeded = |input: &str| {
            let mut app = app_typing(input);
            app.goal_panel_mut().apply_snapshot(goals.clone());
            app
        };

        // The fixture's only finished Goal is `bbbb2222` (`succeeded`); the
        // archived one is already filed, and the live ones are not candidates.
        assert_eq!(paths(&seeded("/goal archive ")), ["goal archive bbbb2222"]);
        assert_eq!(
            paths(&seeded("/goal unarchive ")),
            ["goal unarchive cccc3333"]
        );
        assert_eq!(
            paths(&seeded("/goal archive bbb")),
            ["goal archive bbbb2222"],
            "the token under the cursor still filters"
        );
        assert_eq!(
            paths(&seeded("/goal archive cccc")),
            Vec::<String>::new(),
            "an already-filed Goal is not an archive candidate"
        );
        // `show` is the read that reaches both.
        assert_eq!(paths(&seeded("/goal show ")).len(), 5);
    }

    /// Once the id is complete there is nothing left under the verb, so the
    /// popup closes instead of re-offering the same Goal.
    #[test]
    fn a_completed_goal_id_offers_nothing() {
        let mut app = app_typing("/goal show 216ae6be ");
        app.goal_panel_mut().apply_snapshot(seeded_goals());

        assert!(paths(&app).is_empty());
    }

    /// The subcommands are still what `/goal ` offers: the id list belongs to
    /// the verbs, not to the command.
    #[test]
    fn the_goal_command_still_offers_its_subcommands() {
        let mut app = app_typing("/goal ");
        app.goal_panel_mut().apply_snapshot(seeded_goals());

        assert_eq!(
            paths(&app),
            [
                "goal create",
                "goal list",
                "goal show",
                "goal steer",
                "goal pause",
                "goal resume",
                "goal cancel",
                "goal accept",
                "goal archive",
                "goal unarchive",
            ]
        );
    }

    /// Four Goals, one per interesting status bucket: a running one and a
    /// waiting one that the control verbs can act on, plus a paused one and a
    /// finished one that most of them cannot.
    fn seeded_goals() -> Vec<tact_protocol::GoalSnapshot> {
        use tact_protocol::{GoalCriterionSnapshot, GoalSnapshot, GoalStatusSnapshot};

        let criterion = || GoalCriterionSnapshot {
            id: "c1".into(),
            description: "User accepts the result".into(),
            required: true,
            human: true,
            ..Default::default()
        };
        let goal = |id: &str, title: &str, status: GoalStatusSnapshot| GoalSnapshot {
            id: id.into(),
            title: title.into(),
            status,
            criteria: vec![criterion()],
            ..Default::default()
        };

        // The running Goal has an active run (budget snapshot present), so
        // `accept` must not offer it; a stranded running Goal is covered by its
        // own test below.
        let mut running = goal("216ae6be", "review code", GoalStatusSnapshot::Running);
        running.run_budget = Some(Default::default());
        vec![
            running,
            goal("03a4caef", "ship notes", GoalStatusSnapshot::WaitingUser),
            goal("aaaa1111", "paused work", GoalStatusSnapshot::Paused),
            goal("bbbb2222", "old work", GoalStatusSnapshot::Succeeded),
        ]
    }

    #[test]
    fn the_first_level_is_builtins_only() {
        let mut app = app_typing("/");
        app.skills_data = (0..30)
            .map(|i| SkillEntry {
                name: format!("skill-{i:02}"),
                description: format!("Skill {i}"),
                body: String::new(),

                dir: String::new(),
                disabled: false,
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

                dir: String::new(),
                disabled: false,
            },
            SkillEntry {
                name: "demo".into(),
                description: "Demo skill".into(),
                body: String::new(),

                dir: String::new(),
                disabled: false,
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
                "skill disable",
                "skill enable",
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

                dir: String::new(),
                disabled: false,
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

                dir: String::new(),
                disabled: false,
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
