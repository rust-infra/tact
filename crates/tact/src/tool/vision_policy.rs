//! How an image is prepared before it reaches the model.
//!
//! Three settings — whether to compress, the longest edge and the JPEG quality —
//! were read from the process configuration inside `read_image`, which meant the
//! tool's behaviour depended on a global the caller could not see.
//!
//! They are resolved once at startup and never change, like `ShellPolicy`, so
//! they travel as one value: a tool that has a `VisionPolicy` has answered "how
//! should this image be prepared", and one that does not must say so rather than
//! quietly reading a global.

/// The image pipeline's session-constant settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisionPolicy {
    /// Whether to re-encode the image at all.
    ///
    /// Off means the original bytes are sent: fewer artifacts, but a large
    /// screenshot can exceed the provider's per-image limit.
    pub compress: bool,
    /// Longest edge, in pixels, after compression.
    pub max_edge: u32,
    /// JPEG quality (1–100) when re-encoding.
    pub jpeg_quality: u8,
}

impl Default for VisionPolicy {
    fn default() -> Self {
        Self {
            compress: crate::config::VisionImageSettings::DEFAULT_COMPRESS,
            max_edge: crate::config::VisionImageSettings::DEFAULT_MAX_EDGE,
            jpeg_quality: crate::config::VisionImageSettings::DEFAULT_JPEG_QUALITY,
        }
    }
}

impl VisionPolicy {
    /// The policy a resolved configuration asks for.
    pub fn from_settings(settings: &crate::config::VisionImageSettings) -> Self {
        Self {
            compress: settings.compress,
            max_edge: settings.max_edge,
            jpeg_quality: settings.jpeg_quality,
        }
    }

    /// The edge to fit within when compressing, clamped to something renderable.
    ///
    /// `0` would make the scaler divide by zero, and an absurd ceiling would
    /// defeat the point of compressing; the configured value is clamped rather
    /// than trusted.
    #[must_use]
    pub fn edge(&self) -> u32 {
        self.max_edge.clamp(1, 8192)
    }

    /// Whether re-encoding is worth doing for an image of these dimensions.
    ///
    /// Compression is skipped when the image already fits: re-encoding a small
    /// PNG to JPEG loses quality for no benefit, and the user's `compress = true`
    /// is a ceiling, not an instruction to always re-encode.
    #[must_use]
    pub fn should_reencode(&self, width: u32, height: u32) -> bool {
        self.compress && (width > self.edge() || height > self.edge())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A configured edge of `0` cannot make the scaler divide by zero.
    #[test]
    fn a_zero_edge_is_clamped() {
        let policy = VisionPolicy {
            compress: true,
            max_edge: 0,
            jpeg_quality: 80,
        };
        assert_eq!(policy.edge(), 1);
    }

    /// An absurd ceiling is clamped rather than trusted.
    #[test]
    fn an_absurd_edge_is_clamped() {
        let policy = VisionPolicy {
            compress: true,
            max_edge: u32::MAX,
            jpeg_quality: 80,
        };
        assert_eq!(policy.edge(), 8192);
    }

    /// An image that already fits is not re-encoded.
    ///
    /// `compress = true` is a ceiling, not an instruction: re-encoding a small
    /// image loses quality for nothing.
    #[test]
    fn a_small_image_is_left_alone() {
        let policy = VisionPolicy {
            compress: true,
            max_edge: 1280,
            jpeg_quality: 80,
        };
        assert!(!policy.should_reencode(800, 600));
        assert!(policy.should_reencode(2000, 600), "width over the edge");
        assert!(policy.should_reencode(600, 2000), "height over the edge");
    }

    /// Turning compression off means "never re-encode", whatever the size.
    #[test]
    fn compression_off_never_reencodes() {
        let policy = VisionPolicy {
            compress: false,
            max_edge: 100,
            jpeg_quality: 80,
        };
        assert!(!policy.should_reencode(10_000, 10_000));
    }

    /// The default matches the config defaults, so a host that never resolved a
    /// policy behaves like one that took the documented defaults.
    ///
    /// `VisionImageSettings` has no `Default` impl (its constructor is
    /// `new`), so the values are read from its constants directly.
    #[test]
    fn the_default_matches_the_config_defaults() {
        use crate::config::VisionImageSettings as Settings;
        let settings = Settings {
            compress: Settings::DEFAULT_COMPRESS,
            max_edge: Settings::DEFAULT_MAX_EDGE,
            jpeg_quality: Settings::DEFAULT_JPEG_QUALITY,
        };
        assert_eq!(
            VisionPolicy::default(),
            VisionPolicy::from_settings(&settings)
        );
    }
}
