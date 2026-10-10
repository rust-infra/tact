//! Fixtures the domain stores' tests share.
//!
//! Test-only: nothing here is compiled into the library.

/// A fresh, empty database path under the system temp directory.
///
/// `prefix` names the store the fixture belongs to, so two stores' tests
/// cannot collide when they run at once. The directory is **wiped first**:
/// a run that panicked half-way leaves a database behind, and a later run
/// reading rows it did not write is the flakiest possible failure — one that
/// only shows up on the second attempt.
///
/// Each store keeps a three-line wrapper naming its own prefix, so the rule
/// above is stated once while the fixture name stays next to the tests that
/// read it.
pub(crate) fn temp_db(prefix: &str, name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("{prefix}-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    dir.join("tact.db")
}
