//! The view's ports, implemented over extension services.
//!
//! `tui` declares what it needs (see `tui::ports`); this module is the one place
//! that knows how those questions are answered by the runtime. Keeping the
//! adapter here — not in `tui` — is what lets the view compile without the
//! extension crate: the view names `SkillCatalog`, and only this file mentions
//! `tact::skill`.

use std::sync::Arc;

use tui::ports::SkillCatalog;

/// The skill catalog the runtime owns, seen through the view's port.
///
/// Holds the same `SharedSkillRegistry` the agent's `ToolContext` holds, so a
/// reload or a disable reaches both sides at once — the view must never show a
/// catalog the agent does not have.
pub struct RuntimeSkillCatalog {
    registry: tact::skill::SharedSkillRegistry,
    work_dir: std::path::PathBuf,
}

impl RuntimeSkillCatalog {
    /// Adapt the shared registry for one workspace.
    ///
    /// `work_dir` is kept because a reload re-derives the search roots from it:
    /// tests set the workspace late, and a reload must see the current one.
    pub fn new(registry: tact::skill::SharedSkillRegistry, work_dir: std::path::PathBuf) -> Self {
        Self { registry, work_dir }
    }
}

fn project(reg: &tact::skill::SkillRegistry) -> Vec<tui::SkillEntry> {
    reg.skills()
        .values()
        .map(|doc| tui::SkillEntry {
            name: doc.manifest.name.clone(),
            description: doc.manifest.description.clone(),
            body: doc.body.clone(),
            dir: doc.dir.display().to_string(),
            disabled: reg.is_disabled(&doc.manifest.name),
        })
        .collect()
}

impl SkillCatalog for RuntimeSkillCatalog {
    fn entries(&self) -> Vec<tui::SkillEntry> {
        project(&tact::skill::lock_skills(&self.registry))
    }

    fn reload(&self) -> anyhow::Result<Vec<tui::SkillEntry>> {
        let mut reg = tact::skill::lock_skills(&self.registry);
        // Keep the search roots in sync with the current workspace: a test (or a
        // session that changed directory) must reload from where it is now.
        *reg = tact::skill::get_skill_registry(&self.work_dir)?;
        Ok(project(&reg))
    }

    fn disabled(&self) -> Vec<String> {
        let reg = tact::skill::lock_skills(&self.registry);
        let mut names: Vec<String> = reg.disabled().iter().cloned().collect();
        names.sort();
        names
    }

    fn set_disabled(&self, names: Vec<String>) -> anyhow::Result<Vec<tui::SkillEntry>> {
        let mut reg = tact::skill::lock_skills(&self.registry);
        reg.set_disabled(names);
        Ok(project(&reg))
    }

    fn description(&self) -> String {
        tact::skill::lock_skills(&self.registry).describe_available()
    }
}

/// Wrap a registry for the view.
pub fn skill_catalog(
    registry: tact::skill::SharedSkillRegistry,
    work_dir: std::path::PathBuf,
) -> Arc<dyn SkillCatalog> {
    Arc::new(RuntimeSkillCatalog::new(registry, work_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_with(skills: &[(&str, bool)]) -> (tempfile::TempDir, Arc<dyn SkillCatalog>) {
        let dir = tempfile::tempdir().unwrap();
        for (name, _disabled) in skills {
            let skill_dir = dir.path().join(".tact/skills").join(name);
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {name} desc\n---\n{name} body"),
            )
            .unwrap();
        }
        let registry = tact::skill::shared_skill_registry(dir.path()).unwrap();
        if skills.iter().any(|(_, disabled)| *disabled) {
            let disabled: Vec<String> = skills
                .iter()
                .filter(|(_, disabled)| *disabled)
                .map(|(name, _)| (*name).to_string())
                .collect();
            tact::skill::lock_skills(&registry).set_disabled(disabled);
        }
        let catalog = skill_catalog(registry, dir.path().to_path_buf());
        (dir, catalog)
    }

    /// The port reports what the registry holds, disabled rows included.
    ///
    /// Asserted by presence, not by count: the registry also scans the user's
    /// global skill directory, so a developer machine has far more entries than
    /// this fixture creates. Pinning the count would make the test pass only in
    /// a clean environment.
    #[test]
    fn entries_mirror_the_registry() {
        let (_dir, catalog) = catalog_with(&[("alpha", false), ("beta", true)]);
        let entries = catalog.entries();
        assert!(
            entries
                .iter()
                .any(|entry| entry.name == "alpha" && !entry.disabled),
            "the enabled skill is listed: {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.name == "beta" && entry.disabled),
            "a disabled skill stays listed and is marked: {entries:?}"
        );
        assert_eq!(
            catalog.disabled(),
            vec!["beta".to_string()],
            "only the fixture's skill is disabled"
        );
    }

    /// Disabling replaces the whole set rather than appending.
    ///
    /// The list is what gets written to config, so a partial write would
    /// silently re-enable everything it left out.
    #[test]
    fn set_disabled_replaces_the_whole_set() {
        let (_dir, catalog) = catalog_with(&[("alpha", false), ("beta", false)]);
        catalog
            .set_disabled(vec!["alpha".to_string()])
            .expect("disable alpha");
        assert_eq!(catalog.disabled(), vec!["alpha".to_string()]);

        catalog
            .set_disabled(vec!["beta".to_string()])
            .expect("disable beta instead");
        assert_eq!(
            catalog.disabled(),
            vec!["beta".to_string()],
            "alpha must be re-enabled, not left behind"
        );
    }

    /// A reload sees a skill created after the registry was built.
    #[test]
    fn reload_picks_up_a_new_skill() {
        let (dir, catalog) = catalog_with(&[("alpha", false)]);
        assert!(
            catalog.entries().iter().any(|entry| entry.name == "alpha"),
            "the fixture skill is there before the reload"
        );

        let new_dir = dir.path().join(".tact/skills/gamma");
        std::fs::create_dir_all(&new_dir).unwrap();
        std::fs::write(
            new_dir.join("SKILL.md"),
            "---\nname: gamma\ndescription: gamma desc\n---\nbody",
        )
        .unwrap();

        let entries = catalog.reload().expect("reload");
        assert!(
            entries.iter().any(|entry| entry.name == "gamma"),
            "reload must rescan the workspace: {entries:?}"
        );
    }
}
