//! The font families the machine offers, for the Appearance font picker.
//!
//! The shell's default family comes from the theme, and the theme's fonts are
//! the ones Tact ships or assumes. A user who wants something else — a system
//! UI font that matches the rest of their desktop, a face with the glyphs their
//! language needs — should be able to pick it from what is actually installed,
//! so this asks the platform's font database rather than offering a fixed list.

use std::sync::OnceLock;

/// Every family the system font database knows, sorted and de-duplicated.
///
/// The database walk is a directory scan plus a parse per face, so the answer
/// is cached: the settings dialog renders this on every frame it is open, and
/// the set of installed fonts does not change while the app runs.
pub(crate) fn system_families() -> &'static [String] {
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();
    FAMILIES.get_or_init(|| {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        let mut families: Vec<String> = database
            .faces()
            .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .collect();
        families.sort_unstable();
        families.dedup();
        families
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_font_list_is_sorted_and_unique() {
        let families = system_families();
        // A container or a minimal CI image can legitimately have no fonts; an
        // empty list is a real answer, not a failure.
        if families.is_empty() {
            return;
        }
        let mut sorted = families.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            families,
            sorted.as_slice(),
            "families are sorted and unique"
        );
        assert!(
            families.iter().all(|name| !name.trim().is_empty()),
            "every family has a name"
        );
    }
}
