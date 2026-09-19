//! Tact desktop client entry point.

use gpui_kit::component::{Root, ThemeRegistry};
use gpui_kit::{
    AppContext as _, Bounds, SharedString, TitlebarOptions, WindowBounds, WindowOptions, point, px,
    size,
};
use tact_gui::shell::TactApp;
use tact_gui::theme;

/// Preferred window size at first launch.
const INITIAL_WINDOW: (f32, f32) = (1440., 900.);
/// Smallest supported window; the three-column shell still resolves at 960×640.
const MINIMUM_WINDOW: (f32, f32) = (960., 640.);

fn main() {
    init_logging();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(|cx| {
            gpui_kit::init(cx);
            tact_gui::commands_init(cx);

            if let Err(err) = ThemeRegistry::watch_dir(theme::theme_dir(), cx, theme::reapply) {
                tracing::error!("Cannot watch the Tact themes: {err:#}");
            }

            let preview = preview_requested();

            cx.spawn(async move |cx| {
                cx.open_window(window_options(), |window, cx| {
                    // The approved prototype is the dark Direction A board, so a
                    // fresh window opens dark unless another mode is requested.
                    theme::activate_default(window, cx);

                    let shell = cx.new(|cx| {
                        if preview {
                            TactApp::preview(window, cx)
                        } else {
                            TactApp::connect(window, cx)
                        }
                    });
                    cx.new(|cx| Root::new(shell, window, cx))
                })
                .expect("failed to open the Tact window");
            })
            .detach();
        });
}

/// Window geometry, title bar, and minimum size.
///
/// These are platform bounds rather than layout values, so they stay in pixels;
/// everything inside the window is sized from the theme's rem scale.
fn window_options() -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(INITIAL_WINDOW.0), px(INITIAL_WINDOW.1)),
        ))),
        window_min_size: Some(size(px(MINIMUM_WINDOW.0), px(MINIMUM_WINDOW.1))),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("Tact")),
            appears_transparent: true,
            traffic_light_position: Some(point(px(9.), px(9.))),
        }),
        app_id: Some("dev.tact.Tact".to_string()),
        ..WindowOptions::default()
    }
}

/// Whether this launch asked for the prototype-shaped design preview.
///
/// `--preview` is the explicit switch; `TACT_GUI_PREVIEW` exists so a preview
/// can be requested without rebuilding the command line.
fn preview_requested() -> bool {
    std::env::args().any(|arg| arg == "--preview")
        || std::env::var_os("TACT_GUI_PREVIEW").is_some_and(|value| value != "0")
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,tact_gui=info,gpui_component=info"));

    tracing_subscriber::fmt().with_env_filter(filter).init();
}
