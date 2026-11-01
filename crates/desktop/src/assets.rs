//! The component library's asset bundle, plus the glyphs the shell adds.
//!
//! `gpui_component_assets` pins the default Lucide subset: 101 icons, with no
//! paper plane. The composer's submit control is drawn from the design's
//! `send` glyph, so the shell serves that one path itself and delegates every
//! other lookup to the pinned bundle.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// The component bundle with the shell's extra icons in front of it.
pub struct Assets;

/// Lucide's `send` glyph (ISC), used by the composer's plane submit control.
const SEND_ICON: &[u8] = include_bytes!("../assets/icons/send.svg");
const SEND_PATH: &str = "icons/send.svg";
/// Lucide's `mic` glyph (ISC), used by the composer's voice control.
const MIC_ICON: &[u8] = include_bytes!("../assets/icons/mic.svg");
const MIC_PATH: &str = "icons/mic.svg";
const EXTRA_PATHS: [&str; 2] = [SEND_PATH, MIC_PATH];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == SEND_PATH {
            return Ok(Some(Cow::Borrowed(SEND_ICON)));
        }
        if path == MIC_PATH {
            return Ok(Some(Cow::Borrowed(MIC_ICON)));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_component_assets::Assets.list(path)?;
        for extra in EXTRA_PATHS {
            if extra.starts_with(path) && !paths.iter().any(|known| known.as_ref() == extra) {
                paths.push(extra.into());
            }
        }
        Ok(paths)
    }
}
