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
    root: PathBuf,
}

impl EvalSuite {
    /// Load every `*.toml` under `dir`, sorted by id so a run is reproducible.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let mut cases = Vec::new();
        for entry in std::fs::read_dir(dir)
            .with_context(|| format!("cannot read suite dir {}", dir.display()))?
        {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "toml") {
                continue;
            }
            cases.push(EvalCase::load(&path)?);
        }
        if cases.is_empty() {
            anyhow::bail!("no *.toml cases in {}", dir.display());
        }
        cases.sort_by(|a, b| a.id.cmp(&b.id));

        // Duplicate ids would make the report ambiguous, so refuse at load.
        for pair in cases.windows(2) {
            if pair[0].id == pair[1].id {
                anyhow::bail!("duplicate case id {}", pair[0].id);
            }
        }

        Ok(Self {
            cases,
            root: dir.to_path_buf(),
        })
    }

    pub fn cases(&self) -> &[EvalCase] {
        &self.cases
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Make this attempt's working directory.
///
/// Relative fixture paths resolve against the suite root, so a case file can
/// say `fixture = "fixtures/broken-rust"` without knowing where it is loaded
/// from at runtime.
pub fn prepare_work_dir(case: &EvalCase) -> Result<PathBuf> {
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
