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

    /// Compute the list of matching slash commands with fuzzy scores.
    /// Returns `(index_into_commands, (cmd, desc), score)` sorted by descending
    /// score, then built-ins before skills, then original index.
    pub(crate) fn matched_commands<'a>(
        &self,
        input: &str,
        cursor: usize,
        commands: &[(&'a str, &'a str)],
        skill_names: &std::collections::HashSet<&str>,
    ) -> Vec<(usize, (&'a str, &'a str), i32)> {
        let query = self.query(input, cursor);
        let query_lower = query.to_lowercase();
        if query_lower.is_empty() || query_lower == "/" {
            // Show all commands when only '/' is typed (palette order: cmds then skills)
            commands
                .iter()
                .enumerate()
                .map(|(i, c)| (i, *c, 100))
                .collect()
        } else {
            let query = &query_lower[1..]; // strip leading '/'
            let mut scored: Vec<_> = commands
                .iter()
                .enumerate()
                .filter_map(|(i, &(cmd, desc))| {
                    let score = fuzzy_score(cmd, query);
                    // Also check description for matches
                    let desc_score = fuzzy_score(desc, query);
                    let best = score.max(desc_score);
                    if best > 0 {
                        Some((i, (cmd, desc), best))
                    } else {
                        None
                    }
                })
                .collect();
            scored.sort_by(|a, b| {
                b.2.cmp(&a.2)
                    .then_with(|| {
                        let a_skill = skill_names.contains(a.1.0);
                        let b_skill = skill_names.contains(b.1.0);
                        a_skill.cmp(&b_skill) // commands (false) before skills (true)
                    })
                    .then_with(|| a.0.cmp(&b.0))
            });
            scored
        }
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
