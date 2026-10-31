/// Catalog locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogLocale {
    En,
    Zh,
}

/// Localized command text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalizedText {
    pub en: &'static str,
    pub zh: &'static str,
}

impl LocalizedText {
    pub const fn new(en: &'static str, zh: &'static str) -> Self {
        Self { en, zh }
    }

    pub fn get(self, locale: CatalogLocale) -> &'static str {
        match locale {
            CatalogLocale::En => self.en,
            CatalogLocale::Zh => self.zh,
        }
    }
}
