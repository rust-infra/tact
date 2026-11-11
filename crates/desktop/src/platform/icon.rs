//! The Tact app icon and the identifiers the OS files it under.
//!
//! `assets/app-icon.svg` is the editable source; `scripts/app-icon.py`
//! rasterises it into the formats each platform reads. The rasters are
//! embedded in the binary as well as shipped next to it, so the icon is
//! correct even when the shell runs outside an installed bundle or desktop
//! entry (`cargo run`, a bare tarball).

/// Desktop-entry basename, X11 `WM_CLASS`, and the Wayland `app_id`.
///
/// `assets/linux/tact-desktop.desktop` declares the same value as
/// `StartupWMClass` and `Icon`, which is how X11 and Wayland attach the
/// desktop entry — and therefore the themed icon — to the running window.
pub const APP_ID: &str = "tact-desktop";

/// Reverse-DNS identity for the Windows AppUserModelID and notification
/// identity, matching the packaged `CFBundleIdentifier`.
pub const APP_IDENTIFIER: &str = "dev.tact.desktop";

/// The user-visible application name.
pub const APP_NAME: &str = "Tact Desktop";

/// The 1024px master raster: the macOS Dock asks for the largest size.
const ICON_PNG: &[u8] = include_bytes!("../../assets/app-icon.png");

#[cfg(target_os = "linux")]
/// The 256px raster from the hicolor tree; the largest size a window manager
/// reads from `_NET_WM_ICON`.
const WINDOW_ICON_PNG: &[u8] =
    include_bytes!("../../assets/linux/hicolor/256x256/apps/tact-desktop.png");

/// The X11 window icon, or `None` when the shipped raster cannot be decoded.
///
/// Wayland has no per-window icon protocol: the compositor resolves
/// [`APP_ID`] through the installed desktop entry instead.
#[cfg(target_os = "linux")]
pub fn window_icon() -> Option<std::sync::Arc<image::RgbaImage>> {
    let decoded = image::load_from_memory_with_format(WINDOW_ICON_PNG, image::ImageFormat::Png)
        .inspect_err(|error| {
            eprintln!("Tact Desktop could not decode its window icon: {error}");
        })
        .ok()?;
    Some(std::sync::Arc::new(decoded.to_rgba8()))
}

/// Sets the Dock tile's image.
///
/// A packaged `Tact Desktop.app` already carries `Tact.icns`; this covers
/// development runs and anything launched straight from a binary, which
/// would otherwise show the generic application icon.
#[cfg(target_os = "macos")]
#[allow(deprecated, unexpected_cfgs)]
pub fn install_dock_icon() {
    use cocoa::base::{id, nil};
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let data: id = msg_send![
            class!(NSData),
            dataWithBytes: ICON_PNG.as_ptr().cast::<std::ffi::c_void>()
            length: ICON_PNG.len()
        ];
        if data == nil {
            return;
        }
        let image: id = msg_send![class!(NSImage), alloc];
        let image: id = msg_send![image, initWithData: data];
        if image == nil {
            return;
        }
        // `NSApplication` outlives the image: AppKit retains it.
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        if app == nil {
            return;
        }
        let _: () = msg_send![app, setApplicationIconImage: image];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the PNG header without pulling an image decoder into the test
    /// build (the `image` dependency is Linux-only).
    fn png_header(bytes: &[u8]) -> (u32, u32, u8, u8) {
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
        assert_eq!(&bytes[12..16], b"IHDR", "missing IHDR");
        let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        (width, height, bytes[24], bytes[25])
    }

    #[test]
    fn the_embedded_icon_is_a_1024_square_rgba_png() {
        let (width, height, depth, colour) = png_header(ICON_PNG);
        assert_eq!(
            (width, height),
            (1024, 1024),
            "the Dock wants a 1024px tile"
        );
        assert_eq!(depth, 8);
        assert_eq!(colour, 6, "expected RGBA, not an opaque colour type");
    }

    /// The Linux desktop entry and the window's `app_id` must agree, or the
    /// compositor never finds the themed icon.
    #[test]
    fn the_linux_desktop_entry_matches_the_window_app_id() {
        let entry = include_str!("../../assets/linux/tact-desktop.desktop");
        for (key, value) in [
            ("Icon", APP_ID),
            ("StartupWMClass", APP_ID),
            ("Name", APP_NAME),
        ] {
            let expected = format!("{key}={value}");
            assert!(
                entry.lines().any(|line| line == expected),
                "tact-desktop.desktop needs `{expected}`"
            );
        }
    }

    #[test]
    fn the_hicolor_theme_covers_the_sizes_a_desktop_reads() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("linux")
            .join("hicolor");
        for size in [16, 24, 32, 48, 64, 128, 256, 512] {
            let path = root
                .join(format!("{size}x{size}"))
                .join("apps")
                .join("tact-desktop.png");
            assert!(
                path.is_file(),
                "missing {}: run `python3 scripts/app-icon.py`",
                path.display()
            );
        }
        assert!(root.join("scalable/apps/tact-desktop.svg").is_file());
    }
}
