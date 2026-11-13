//! A directory of cases, and the per-attempt work directory both runners share.
//!
//! Work directories are separated by attempt *and* case id, because two cases
//! running at once would otherwise write the same `.tact/tact.db` and trip the
//! per-workdir session lock.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::case::{EvalCase, Isolation};

#[derive(Debug, Clone)]
pub struct EvalSuite {
    cases: Vec<EvalCase>,
    /// The cases exactly as written, before a relative fixture was resolved to
    /// an absolute path. The fingerprint hashes this form so the same suite
    /// fingerprints the same from any working directory.
    declared: Vec<EvalCase>,
}

impl EvalSuite {
    /// Load every `*.toml` under `dir`, sorted by id so a run is reproducible.
    ///
    /// A relative `fixture` is resolved against `dir` here, at the boundary
    /// where the suite's location is known, and then validated. Resolving it
    /// later would make a case depend on the process's working directory.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let mut cases = Vec::new();
        let mut declared_cases = Vec::new();
        for entry in std::fs::read_dir(dir)
            .with_context(|| format!("cannot read suite dir {}", dir.display()))?
        {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "toml") {
                continue;
            }
            let mut case = EvalCase::load(&path)?;
            let declared = case.clone();
            resolve_fixture(&mut case, dir, &path)?;
            cases.push(case);
            declared_cases.push(declared);
        }
        if cases.is_empty() {
            anyhow::bail!("no *.toml cases in {}", dir.display());
        }
        cases.sort_by(|a, b| a.id.cmp(&b.id));
        declared_cases.sort_by(|a, b| a.id.cmp(&b.id));

        // Duplicate ids would make the report ambiguous, so refuse at load.
        for pair in cases.windows(2) {
            if pair[0].id == pair[1].id {
                anyhow::bail!("duplicate case id {}", pair[0].id);
            }
        }

        Ok(Self {
            cases,
            declared: declared_cases,
        })
    }

    pub fn cases(&self) -> &[EvalCase] {
        &self.cases
    }

    /// The cases as written, for the fingerprint.
    pub fn declared_cases(&self) -> &[EvalCase] {
        &self.declared
    }
}

/// Resolve a relative fixture against the suite directory and check it exists.
///
/// This is where the fixture contract lives, because it is the only place that
/// knows both the case and the directory the case was loaded from.
fn resolve_fixture(case: &mut EvalCase, suite_dir: &Path, case_path: &Path) -> Result<()> {
    if let Isolation::Copy { fixture } = &mut case.isolation {
        if fixture.is_relative() {
            *fixture = suite_dir.join(&*fixture);
        }
        if !fixture.is_dir() {
            anyhow::bail!(
                "{}: fixture {} is not a directory",
                case_path.display(),
                fixture.display()
            );
        }
    }
    Ok(())
}

/// Make this attempt's working directory.
///
/// Fixtures arrive absolute: `EvalSuite::load` resolves them, so this does not
/// depend on the process's working directory.
pub(crate) fn prepare_work_dir(case: &EvalCase) -> Result<PathBuf> {
    let dir = std::env::temp_dir()
        .join("tact-eval")
        .join(&case.id)
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cannot create work dir {}", dir.display()))?;

    if let Isolation::Copy { fixture } = &case.isolation {
        copy_tree(fixture, &dir)?;
    }
    Ok(dir)
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(from).follow_links(false) {
        let entry = entry?;
        let relative = entry.path().strip_prefix(from)?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = to.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cases_load_sorted_and_reject_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.toml"), "id = \"b\"\nprompt = \"p\"\n").unwrap();
        std::fs::write(dir.path().join("a.toml"), "id = \"a\"\nprompt = \"p\"\n").unwrap();

        let suite = EvalSuite::load(dir.path()).unwrap();

        let ids: Vec<&str> = suite.cases().iter().map(|case| case.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn a_duplicate_id_is_refused_rather_than_reported_twice() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.toml"), "id = \"same\"\nprompt = \"p\"\n").unwrap();
        std::fs::write(dir.path().join("b.toml"), "id = \"same\"\nprompt = \"p\"\n").unwrap();

        let error = EvalSuite::load(dir.path()).unwrap_err().to_string();

        assert!(error.contains("duplicate case id"), "{error}");
    }

    #[test]
    fn a_relative_fixture_resolves_against_the_suite_dir() {
        let suite_dir = tempfile::tempdir().unwrap();
        let fixture = suite_dir.path().join("fixtures/seed");
        std::fs::create_dir_all(&fixture).unwrap();
        std::fs::write(fixture.join("seed.txt"), "seed").unwrap();
        std::fs::write(
            suite_dir.path().join("copied.toml"),
            "prompt = \"p\"\n[isolation]\nkind = \"copy\"\nfixture = \"fixtures/seed\"\n",
        )
        .unwrap();

        let suite = EvalSuite::load(suite_dir.path()).unwrap();

        let case = &suite.cases()[0];
        match &case.isolation {
            Isolation::Copy { fixture } => {
                assert!(fixture.is_absolute(), "{}", fixture.display());
                assert!(fixture.ends_with("fixtures/seed"));
            }
            other => panic!("expected a copy isolation, got {other:?}"),
        }
    }

    #[test]
    fn the_declared_cases_keep_the_fixture_spelling_and_the_resolved_ones_do_not() {
        let suite_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(suite_dir.path().join("fixtures/seed")).unwrap();
        std::fs::write(
            suite_dir.path().join("copied.toml"),
            "prompt = \"p\"\n[isolation]\nkind = \"copy\"\nfixture = \"fixtures/seed\"\n",
        )
        .unwrap();

        let suite = EvalSuite::load(suite_dir.path()).unwrap();

        // The fingerprint hashes the declared form, so loading the same suite
        // through a relative and an absolute path agrees.
        let declared = &suite.declared_cases()[0];
        assert_eq!(
            declared.isolation,
            Isolation::Copy {
                fixture: PathBuf::from("fixtures/seed")
            }
        );
        assert!(matches!(
            &suite.cases()[0].isolation,
            Isolation::Copy { fixture } if fixture.is_absolute()
        ));
    }

    #[test]
    fn a_missing_relative_fixture_is_rejected_at_load() {
        let suite_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            suite_dir.path().join("broken.toml"),
            "prompt = \"p\"\n[isolation]\nkind = \"copy\"\nfixture = \"fixtures/nope\"\n",
        )
        .unwrap();

        let error = EvalSuite::load(suite_dir.path()).unwrap_err().to_string();

        assert!(error.contains("not a directory"), "{error}");
    }

    #[test]
    fn a_copy_isolation_copies_the_fixture_into_a_fresh_dir() {
        let fixture = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("seed.txt"), "seed").unwrap();

        let case = EvalCase {
            id: format!("copy-{}", uuid::Uuid::new_v4()),
            prompt: "p".into(),
            timeout_secs: 1,
            isolation: Isolation::Copy {
                fixture: fixture.path().to_path_buf(),
            },
            graders: Vec::new(),
        };

        let work = prepare_work_dir(&case).unwrap();

        assert_eq!(
            std::fs::read_to_string(work.join("seed.txt")).unwrap(),
            "seed"
        );
        assert_ne!(work, fixture.path());
        std::fs::remove_dir_all(work).ok();
    }
}
