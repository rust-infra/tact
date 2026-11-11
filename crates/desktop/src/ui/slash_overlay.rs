use tact_catalog::{CatalogLocale, CommandArguments, CommandDescriptor, CommandId, fuzzy_score};
use tact_desktop_core::{CommandAvailability, is_enabled};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCandidate {
    pub id: CommandId,
    pub title: String,
    pub detail: String,
    pub insertion: String,
    pub has_children: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashPath {
    pub path: Vec<String>,
    pub tokens: CommandArguments,
}

pub fn parse(text: &str) -> Option<SlashPath> {
    let text = text.trim().strip_prefix('/')?;
    let tokens = text
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let (path, rest) = tokens.split_first()?;
    let catalog = tact_catalog::builtins();
    let mut resolved = vec![path.clone()];
    let mut index = 0;
    while let Some(descriptor) =
        catalog.resolve_path(&resolved.iter().map(String::as_str).collect::<Vec<_>>())
    {
        if descriptor.children.is_empty() || rest.get(index).is_none() {
            return Some(SlashPath {
                path: resolved,
                tokens: CommandArguments {
                    tokens: rest.get(index..).unwrap_or_default().to_vec(),
                },
            });
        }
        resolved.push(rest[index].clone());
        index += 1;
    }
    Some(SlashPath {
        path: resolved,
        tokens: CommandArguments {
            tokens: rest.get(index..).unwrap_or_default().to_vec(),
        },
    })
}

pub fn children(path: &[String]) -> Vec<&'static CommandDescriptor> {
    let catalog = tact_catalog::builtins();
    let Some(descriptor) =
        catalog.resolve_path(&path.iter().map(String::as_str).collect::<Vec<_>>())
    else {
        return Vec::new();
    };
    catalog.children(&descriptor.id).collect()
}

pub fn execute_id(path: &[String]) -> Option<CommandId> {
    tact_catalog::builtins()
        .resolve_path(&path.iter().map(String::as_str).collect::<Vec<_>>())
        .map(|descriptor| descriptor.id.clone())
}

/// Return the catalog children that match the path fragment at the caret.
pub fn command_candidates(
    text: &str,
    locale: CatalogLocale,
    availability: CommandAvailability,
    skills: &[(String, String)],
) -> Vec<CommandCandidate> {
    let Some(body) = text.strip_prefix('/') else {
        return Vec::new();
    };
    if body.contains(['\n', '\r']) {
        return Vec::new();
    }
    let tokens = body.split_whitespace().collect::<Vec<_>>();
    let ends_with_whitespace = body.chars().last().is_some_and(char::is_whitespace);
    let (base, fragment) = if ends_with_whitespace {
        (tokens.as_slice(), "")
    } else if let Some((fragment, base)) = tokens.split_last() {
        (base, *fragment)
    } else {
        (&[][..], "")
    };

    let catalog = tact_catalog::builtins();
    let mut entries = if base.is_empty() {
        catalog
            .entries()
            .filter(|entry| entry.parent.is_none() && matches!(entry.id, CommandId::Builtin(_)))
            .collect::<Vec<_>>()
    } else {
        let Some(parent) = catalog.resolve_path(base) else {
            return Vec::new();
        };
        catalog.children(&parent.id).collect()
    };
    let mut candidates = entries
        .drain(..)
        .filter(|entry| is_enabled(entry, availability))
        .filter_map(|entry| {
            let path = entry.path.join(" ");
            let label = entry.label.get(locale);
            let score = fuzzy_score(fragment, label)
                .or_else(|| fuzzy_score(fragment, &path))
                .or_else(|| fuzzy_score(fragment, entry.description.get(locale)))
                .or_else(|| fuzzy_score(fragment, entry.group.label().get(locale)))
                .or_else(|| {
                    entry
                        .aliases
                        .iter()
                        .find_map(|alias| fuzzy_score(fragment, alias))
                })?;
            let needs_more = !entry.children.is_empty() || !entry.args.is_empty();
            Some((
                score,
                CommandCandidate {
                    id: entry.id.clone(),
                    title: label.to_string(),
                    detail: entry.description.get(locale).to_string(),
                    insertion: format!("/{path}{}", if needs_more { " " } else { "" }),
                    has_children: !entry.children.is_empty(),
                },
            ))
        })
        .collect::<Vec<_>>();
    let skill_base = if base == ["skill"] {
        Some(true)
    } else if base.is_empty() && !fragment.is_empty() {
        Some(false)
    } else {
        None
    };
    // `CommandAvailability` is what gates every builtin entry, so the dynamic
    // skill entries ask the same question instead of trusting the caller to
    // hand over an empty list.
    if let Some(prefixed) = skill_base.filter(|_| availability.has_installed_skill) {
        for (name, description) in skills {
            if name.chars().any(char::is_whitespace) {
                continue;
            }
            if catalog.resolve_path(&["skill", name]).is_some() {
                continue;
            }
            if !prefixed && catalog.resolve_path(&[name]).is_some() {
                continue;
            }
            let Some(score) = fuzzy_score(fragment, name) else {
                continue;
            };
            candidates.push((
                score,
                CommandCandidate {
                    id: CommandId::DynamicSkill(name.clone()),
                    title: name.clone(),
                    detail: description.clone(),
                    insertion: format!("/skill {name} "),
                    has_children: false,
                },
            ));
        }
    }
    candidates.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    candidates.into_iter().map(|(_, entry)| entry).collect()
}

/// Resolve the longest command path in a typed `/` command.
pub fn resolve(text: &str) -> Option<(&'static CommandDescriptor, CommandArguments)> {
    let body = text.trim().strip_prefix('/')?;
    let tokens = body.split_whitespace().collect::<Vec<_>>();
    let catalog = tact_catalog::builtins();
    for end in (1..=tokens.len()).rev() {
        if let Some(descriptor) = catalog.resolve_path(&tokens[..end]) {
            return Some((
                descriptor,
                CommandArguments {
                    tokens: tokens[end..]
                        .iter()
                        .map(|token| (*token).to_string())
                        .collect(),
                },
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_candidates_follow_the_catalog_tree() {
        let candidates = command_candidates(
            "/plugin mar",
            CatalogLocale::En,
            CommandAvailability {
                has_project: true,
                ..Default::default()
            },
            &[],
        );
        let marketplace = candidates
            .iter()
            .find(|candidate| candidate.insertion == "/plugin marketplace ")
            .expect("marketplace candidate");
        assert!(marketplace.has_children);
    }

    #[test]
    fn installed_skills_are_first_class_slash_candidates() {
        let skills = vec![(
            "test-driven-development".to_string(),
            "Write tests first".to_string(),
        )];
        let candidates = command_candidates(
            "/skill td",
            CatalogLocale::En,
            CommandAvailability {
                has_project: true,
                has_installed_skill: true,
                ..Default::default()
            },
            &skills,
        );
        assert!(candidates.iter().any(|candidate| {
            candidate.id == CommandId::DynamicSkill("test-driven-development".into())
                && candidate.insertion == "/skill test-driven-development "
        }));
    }

    #[test]
    fn root_fragment_finds_installed_skills() {
        let skills = vec![(
            "rust-skills".to_string(),
            "Comprehensive Rust coding guidelines".to_string(),
        )];
        let candidates = command_candidates(
            "/rust",
            CatalogLocale::En,
            CommandAvailability {
                has_project: true,
                has_installed_skill: true,
                ..Default::default()
            },
            &skills,
        );
        assert!(candidates.iter().any(|candidate| {
            candidate.id == CommandId::DynamicSkill("rust-skills".into())
                && candidate.insertion == "/skill rust-skills "
        }));
    }

    #[test]
    fn no_installed_skills_means_no_dynamic_skill_entries() {
        let skills = vec![("rust-skills".to_string(), "Rust guidelines".to_string())];
        for query in ["/rust", "/skill rust"] {
            let candidates = command_candidates(
                query,
                CatalogLocale::En,
                CommandAvailability {
                    has_project: true,
                    has_installed_skill: false,
                    ..Default::default()
                },
                &skills,
            );
            assert!(
                candidates
                    .iter()
                    .all(|candidate| !matches!(candidate.id, CommandId::DynamicSkill(_))),
                "{query} offered a skill entry without an installed skill"
            );
        }
    }

    #[test]
    fn empty_root_query_keeps_the_slash_menu_to_builtins() {
        let skills = vec![("rust-skills".to_string(), "Rust guidelines".to_string())];
        let candidates = command_candidates(
            "/",
            CatalogLocale::En,
            CommandAvailability {
                has_project: true,
                has_installed_skill: true,
                ..Default::default()
            },
            &skills,
        );
        assert!(
            candidates
                .iter()
                .all(|candidate| matches!(candidate.id, CommandId::Builtin(_)))
        );
    }

    #[test]
    fn a_builtin_name_wins_over_an_installed_skill_with_the_same_name() {
        let skills = vec![("help".to_string(), "Colliding skill".to_string())];
        let candidates = command_candidates(
            "/help",
            CatalogLocale::En,
            CommandAvailability {
                has_project: true,
                has_installed_skill: true,
                ..Default::default()
            },
            &skills,
        );
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.id != CommandId::DynamicSkill("help".into()))
        );
    }

    #[test]
    fn slash_root_only_lists_builtin_slash_commands() {
        let candidates = command_candidates(
            "/",
            CatalogLocale::En,
            CommandAvailability {
                has_project: true,
                has_session: true,
                has_account: true,
                has_installed_skill: true,
            },
            &[],
        );
        assert!(
            candidates
                .iter()
                .all(|candidate| matches!(candidate.id, CommandId::Builtin(_)))
        );
    }

    #[test]
    fn longest_path_keeps_arguments_separate() {
        let (descriptor, args) = resolve("/goal show goal-1").unwrap();
        assert_eq!(descriptor.path, ["goal", "show"]);
        assert_eq!(args.tokens, ["goal-1"]);
    }
}
