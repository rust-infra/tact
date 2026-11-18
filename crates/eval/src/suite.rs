//! A directory of cases, and the per-attempt work directory both runners share.
//!
//! Work directories are separated by attempt *and* case id. Each attempt then
//! gets its own `.tact/tact.db`, and its sessions have their own ids, so two
//! cases running at once share neither the database nor a session row.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::case::{EvalCase, Isolation};

#[derive(Debug, Clone)]
pub struct EvalSuite {
    /// The suite directory, canonicalized so a fixture path can be made
    /// relative to it regardless of how the caller spelled the suite path.
    root: PathBuf,
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
        let root = dir
            .canonicalize()
            .with_context(|| format!("cannot read suite dir {}", dir.display()))?;
        let mut cases = Vec::new();
        let mut declared_cases = Vec::new();
        for entry in std::fs::read_dir(&root)
            .with_context(|| format!("cannot read suite dir {}", root.display()))?
        {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "toml") {
                continue;
            }
            let mut case = EvalCase::load(&path)?;
            let declared = case.clone();
            resolve_fixture(&mut case, &root, &path)?;
            cases.push(case);
            declared_cases.push(declared);
        }
        if cases.is_empty() {
            anyhow::bail!("no *.toml cases in {}", root.display());
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
            root,
            cases,
            declared: declared_cases,
        })
    }

    pub fn cases(&self) -> &[EvalCase] {
        &self.cases
    }

    /// Select a subset of cases by id, for iterating on one case.
    ///
    /// Both the run set and the declared set are filtered, so the subset
    /// fingerprints as the subset. Comparing it against a full run therefore
    /// fails the suite-fingerprint check and reads as `incomparable` rather than
    /// as a regression — a filtered run is not evidence about the whole suite,
    /// and the gate says so instead of guessing.
    ///
    /// An id that matches nothing is an error, not an empty run: a typo would
    /// otherwise look like a suite that passed everything at no cost.
    pub fn select(&self, ids: &[String]) -> anyhow::Result<Self> {
        if ids.is_empty() {
            return Ok(self.clone());
        }
        let mut missing: Vec<&str> = ids
            .iter()
            .map(String::as_str)
            .filter(|id| !self.cases.iter().any(|case| case.id == *id))
            .collect();
        if !missing.is_empty() {
            missing.sort_unstable();
            let known: Vec<&str> = self.cases.iter().map(|case| case.id.as_str()).collect();
            anyhow::bail!(
                "no case named {}; this suite has {known:?}",
                missing
                    .iter()
                    .map(|id| format!("{id:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        Ok(Self {
            root: self.root.clone(),
            cases: self
                .cases
                .iter()
                .filter(|case| ids.contains(&case.id))
                .cloned()
                .collect(),
            declared: self
                .declared
                .iter()
                .filter(|case| ids.contains(&case.id))
                .cloned()
                .collect(),
        })
    }

    /// The cases as written, for the fingerprint.
    pub fn declared_cases(&self) -> &[EvalCase] {
        &self.declared
    }

    /// Identity of the suite: its declared cases plus every Copy fixture's
    /// file contents.
    ///
    /// Fixture paths are hashed relative to the suite root, and their contents
    /// by sorted relative path, so the same suite has one identity whether it
    /// was loaded through a relative or an absolute path.
    pub fn fingerprint(&self) -> Result<String> {
        let mut hasher = Sha256::new();
        crate::report::hash_declared_cases(&mut hasher, &self.declared);

        for (declared, loaded) in self.declared.iter().zip(&self.cases) {
            let (Isolation::Copy { .. }, Isolation::Copy { fixture }) =
                (&declared.isolation, &loaded.isolation)
            else {
                continue;
            };

            let fixture_path = fixture
                .canonicalize()
                .with_context(|| format!("cannot resolve fixture {}", fixture.display()))?;
            let relative_fixture = relative_to(&self.root, &fixture_path);
            hash_path(&mut hasher, &relative_fixture);

            let mut files = Vec::new();
            for entry in walkdir::WalkDir::new(&fixture_path).follow_links(false) {
                let entry = entry.with_context(|| {
                    format!("cannot inspect fixture {}", fixture_path.display())
                })?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let relative = entry.path().strip_prefix(&fixture_path).with_context(|| {
                    format!(
                        "fixture entry {} is outside its root",
                        entry.path().display()
                    )
                })?;
                files.push((normalized_path(relative), entry.path().to_path_buf()));
            }
            files.sort_by(|left, right| left.0.cmp(&right.0));

            hasher.update((files.len() as u64).to_le_bytes());
            for (relative, path) in files {
                let bytes = std::fs::read(&path)
                    .with_context(|| format!("cannot read fixture file {}", path.display()))?;
                hasher.update((relative.len() as u64).to_le_bytes());
                hasher.update(relative.as_bytes());
                hasher.update((bytes.len() as u64).to_le_bytes());
                hasher.update(bytes);
            }
        }

        Ok(crate::report::finish_fingerprint(hasher))
    }
}

fn hash_path(hasher: &mut Sha256, path: &Path) {
    let path = normalized_path(path);
    hasher.update((path.len() as u64).to_le_bytes());
    hasher.update(path.as_bytes());
}

fn normalized_path(path: &Path) -> String {
    use std::path::Component;

    let parts: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            Component::CurDir => None,
            Component::ParentDir => Some("..".into()),
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            Component::RootDir | Component::Prefix(_) => None,
        })
        .collect();
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

fn relative_to(root: &Path, target: &Path) -> PathBuf {
    let root: Vec<_> = root.components().collect();
    let target: Vec<_> = target.components().collect();
    let common = root
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();

    let mut relative = PathBuf::new();
    for _ in common..root.len() {
        relative.push("..");
    }
    for component in &target[common..] {
        relative.push(component.as_os_str());
    }
    relative
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
///
/// The case id is a *name*, never a path: it is reduced to one path component
/// here, because the run deletes this directory when grading finishes. An id of
/// `".."` (from a case file named `...toml`, whose stem is `..`) or one built by
/// a library caller with a separator would otherwise aim that deletion outside
/// the scratch root. Sanitizing is the guarantee; `EvalCase::validate_shape`
/// reports the same mistake at load, where it can name the file.
pub(crate) fn prepare_work_dir(case: &EvalCase) -> Result<PathBuf> {
    let dir = std::env::temp_dir()
        .join("tact-eval")
        .join(work_dir_component(&case.id))
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cannot create work dir {}", dir.display()))?;

    if let Isolation::Copy { fixture } = &case.isolation {
        copy_tree(fixture, &dir)?;
    }
    Ok(dir)
}

/// Reduce a case id to one safe directory component.
///
/// Anything that is not an alphanumeric, `-`, `_`, or `.` becomes `_`, and a
/// component that is `.` or `..` becomes `_`.
///
/// When the reduction changed anything, a short hash of the original id is
/// appended. Without it `a/b` and `a_b` would share a directory, and an orphan
/// left behind by a crashed run would no longer say which case it came from.
/// With it the mapping is still not injective in theory, but a collision now
/// needs two ids that agree on both the sanitized form and the hash.
fn work_dir_component(id: &str) -> String {
    let original = id.trim();
    let mut component: String = original
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if component.is_empty() || component == "." || component == ".." {
        component = "_".into();
    }
    if component != original {
        let digest = Sha256::digest(original.as_bytes());
        component.push('-');
        component.push_str(&format!("{digest:x}")[..8]);
    }
    // A bound keeps one long id from pushing the path past a filesystem limit.
    component.truncate(72);
    component
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

    fn write_copy_suite(dir: &Path, prompt: &str) -> PathBuf {
        let fixture = dir.join("fixtures/seed");
        std::fs::create_dir_all(fixture.join("nested")).unwrap();
        std::fs::write(fixture.join("seed.txt"), "seed").unwrap();
        std::fs::write(fixture.join("nested/other.txt"), "other").unwrap();
        std::fs::write(
            dir.join("copied.toml"),
            format!(
                "id = \"copied\"\nprompt = \"{prompt}\"\n\
                 [isolation]\nkind = \"copy\"\nfixture = \"fixtures/seed\"\n"
            ),
        )
        .unwrap();
        fixture
    }

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
    fn a_declared_case_change_changes_the_suite_fingerprint() {
        let suite_dir = tempfile::tempdir().unwrap();
        write_copy_suite(suite_dir.path(), "one");
        let before = EvalSuite::load(suite_dir.path())
            .unwrap()
            .fingerprint()
            .unwrap();

        write_copy_suite(suite_dir.path(), "two");
        let after = EvalSuite::load(suite_dir.path())
            .unwrap()
            .fingerprint()
            .unwrap();

        assert_ne!(before, after);
    }

    #[test]
    fn fixture_bytes_change_the_suite_fingerprint() {
        let suite_dir = tempfile::tempdir().unwrap();
        let fixture = write_copy_suite(suite_dir.path(), "one");
        let suite = EvalSuite::load(suite_dir.path()).unwrap();
        let before = suite.fingerprint().unwrap();

        std::fs::write(fixture.join("nested/other.txt"), "changed").unwrap();
        let after = suite.fingerprint().unwrap();

        assert_ne!(before, after);
    }

    #[test]
    fn a_suite_fingerprints_the_same_through_relative_and_absolute_paths() {
        let cwd = std::env::current_dir().unwrap();
        let suite_dir = tempfile::tempdir_in(&cwd).unwrap();
        write_copy_suite(suite_dir.path(), "one");

        let relative = suite_dir.path().strip_prefix(&cwd).unwrap();
        let from_relative = EvalSuite::load(relative).unwrap().fingerprint().unwrap();
        let from_absolute = EvalSuite::load(suite_dir.path().canonicalize().unwrap())
            .unwrap()
            .fingerprint()
            .unwrap();

        assert_eq!(from_relative, from_absolute);
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
