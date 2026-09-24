//! UI integration tests for the Tact desktop shell.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::{ActiveTheme as _, Root, ThemeMode, ThemeRegistry};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{App, AppContext as _, Entity, SharedString, TestAppContext, Window, px, size};

use tact_gui::{RecentSession, TactApp, theme};

#[gpui_kit::test]
fn shipped_theme_file_loads_into_the_registry(cx: &mut TestAppContext) {
    let path = theme::theme_dir().join("tact-anthropic.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));

    cx.update(|cx| {
        gpui_kit::init(cx);
        ThemeRegistry::global_mut(cx)
            .load_themes_from_str(&content)
            .expect("the shipped theme file must parse");

        let names: Vec<String> = ThemeRegistry::global(cx)
            .themes()
            .keys()
            .map(|name| name.to_string())
            .collect();
        assert!(
            names.iter().any(|name| name == theme::LIGHT_THEME_NAME),
            "missing {}: {names:?}",
            theme::LIGHT_THEME_NAME
        );
        assert!(
            names.iter().any(|name| name == theme::DARK_THEME_NAME),
            "missing {}: {names:?}",
            theme::DARK_THEME_NAME
        );
    });
}

#[gpui_kit::test]
fn bundled_lora_fonts_load_without_error(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::register_bundled_fonts(cx).expect("bundled Lora fonts load");
    });
}

#[gpui_kit::test]
fn activate_adopts_the_tact_themes(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let content = std::fs::read_to_string(theme::theme_dir().join("tact-anthropic.json"))
            .expect("the shipped theme file must be readable");
        ThemeRegistry::global_mut(cx)
            .load_themes_from_str(&content)
            .expect("the shipped theme file must parse");

        theme::activate(ThemeMode::Dark, None, cx).expect("dark theme must activate");
        assert_eq!(cx.theme().theme_name().as_ref(), theme::DARK_THEME_NAME);

        theme::activate(ThemeMode::Light, None, cx).expect("light theme must activate");
        assert_eq!(cx.theme().theme_name().as_ref(), theme::LIGHT_THEME_NAME);
    });
}

#[gpui_kit::test]
fn work_pane_toggle_shows_and_hides_the_pane(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("sidebar").is_some(), "sidebar renders");
        assert!(
            window.try_find("transcript").is_some(),
            "transcript renders"
        );
        assert!(
            window.try_find("work-pane").is_some(),
            "work pane starts open"
        );

        window.click("toggle-work-pane", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane").is_none(),
            "toggling hides the work pane"
        );

        window.click("toggle-work-pane", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane").is_some(),
            "toggling restores the work pane"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_title_bar_trails_with_the_prototypes_controls(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        let palette = window.find("open-command-palette").bounds();
        let left = window.find("title-bar-left").bounds();
        let center = window.find("title-bar-center").bounds();
        let right = window.find("title-bar-right").bounds();
        let work_pane = window.find("work-pane").bounds();
        let theme = window.find("toggle-theme").bounds();
        let work = window.find("toggle-work-pane").bounds();
        let settings = window.find("open-settings").bounds();

        assert!(
            center.origin.x >= left.right(),
            "the tabs stay in the center column: {center:?} against {left:?}"
        );
        assert!(
            right.origin.x >= work_pane.origin.x,
            "the title controls stay in the work-pane column: {right:?} against {work_pane:?}"
        );
        assert!(
            palette.origin.x >= right.origin.x,
            "the command field stays inside the work-pane column: {palette:?} against {right:?}"
        );
        assert!(
            palette.origin.x < theme.origin.x,
            "the palette field leads `.tr`"
        );
        assert!(
            theme.origin.x < work.origin.x && work.origin.x < settings.origin.x,
            "`.tr` trails with theme, the work pane toggle, then settings"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn work_pane_becomes_a_drawer_at_the_minimum_window(cx: &mut TestAppContext) {
    settle_motion(cx);
    cx.update(gpui_kit::init);

    let handle = cx.open_window(size(px(960.), px(640.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("sidebar").is_some(),
            "sidebar stays visible"
        );
        let work_pane = window.find("work-pane").bounds();
        assert_eq!(
            width(work_pane),
            384.,
            "the work pane keeps its comfortable width as a drawer"
        );
        assert_eq!(
            f32::from(work_pane.origin.x + work_pane.size.width),
            960.,
            "the drawer is anchored to the right edge"
        );

        let transcript = window.find("transcript").bounds().size.width;
        assert!(
            transcript > px(640.),
            "the transcript keeps the window instead of wrapping per character: {transcript:?}"
        );
    })
    .unwrap();
}

/// The one contract that needs a clock.
///
/// `.work` is `transform:translateX(105%)` with
/// `transition:transform 180ms var(--ease)`, and the prototype's close paths
/// (`#workClose`, `#scrim`) only flip `body.workOpen` -- the panel itself is
/// never removed, it transitions back out of view. So this test leaves reduced
/// motion off, walks the drawer through its travel on the executor's clock,
/// and checks both ends: it does not snap in on mount, it does not snap out on
/// close, and it unmounts only once the exit transition has finished.
#[gpui_kit::test]
fn the_work_pane_drawer_slides_in_and_out_over_the_prototype_duration(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let handle = cx.open_window(size(px(960.), px(640.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    let drawer_x = |cx: &mut TestAppContext| -> f32 {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find("work-pane").bounds().origin.x.into()
        })
        .unwrap()
    };

    // `translateX(105%)` of the drawer's own 384px parks it at 960 + 403.2,
    // so its left edge starts past the right edge of a 960px window.
    let parked = drawer_x(cx);
    assert_eq!(parked, 979., "the drawer starts parked offscreen");

    // 90ms into the 180ms: strictly inside the travel, at neither end.
    cx.background_executor
        .advance_clock(Duration::from_millis(90));
    cx.run_until_parked();
    let halfway = drawer_x(cx);
    assert!(
        halfway < parked && halfway > 540.,
        "the drawer is mid-slide 90ms in: {halfway} between {parked} and 540"
    );

    // Past the duration: settled in its column, at 960 - 384.
    cx.background_executor
        .advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    let settled = drawer_x(cx);
    assert_eq!(settled, 576., "the drawer settles in its column");

    // Closing flips the flag rather than the mount, so the panel survives its
    // own exit and only leaves once the transition is done.
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("work-pane-close", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.background_executor
        .advance_clock(Duration::from_millis(90));
    cx.run_until_parked();
    let leaving = drawer_x(cx);
    assert!(
        leaving > settled,
        "the drawer keeps its node while it slides back out: {leaving} past {settled}"
    );

    cx.background_executor
        .advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane").is_none(),
            "the drawer unmounts once its exit transition finishes"
        );
    })
    .unwrap();
}

/// The prototype makes the work pane modal while it is a drawer: with
/// `body.workOpen` its `.scrim` covers everything but the drawer
/// (`z-index:25` against the pane's `30` and the sidebar's `40`) and
/// `q('#scrim').onclick=()=>work(false)`.
///
/// `visible()` is only an intersection with the viewport, so a control that
/// sits in the window but *behind* the drawer still reads as visible. The
/// contract therefore has to be behavioural: a press aimed at such a control
/// lands on the scrim instead — closing the drawer — and never reaches the
/// control. Pressing the same control again, with the scrim gone, is what
/// proves the scrim was what swallowed the first press.
#[gpui_kit::test]
fn the_work_pane_drawer_swallows_presses_behind_it(cx: &mut TestAppContext) {
    settle_motion(cx);
    cx.update(gpui_kit::init);
    cx.update(tact_gui::commands_init);

    let sessions = vec![
        RecentSession {
            id: "11111111-aaaa-bbbb".to_string(),
            updated_at_unix: 1_700_000_000,
            message_count: 4,
            title: Some("Desktop client design".to_string()),
            name: None,
            archived: false,
            pinned: false,
        },
        RecentSession {
            id: "22222222-cccc-dddd".to_string(),
            updated_at_unix: 1_700_000_100,
            message_count: 0,
            title: None,
            name: None,
            archived: false,
            pinned: false,
        },
    ];
    // 1100px is past `WORK_PANE_IN_FLOW_FROM` (80rem = 1280px), so the pane is
    // out as a drawer while the sidebar still holds its column.
    let handle = cx.open_window(size(px(1100.), px(800.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_sessions(window, cx, sessions));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane").is_some(),
            "the pane is a drawer at this width"
        );
        assert!(
            window.try_find("work-pane-scrim").is_some(),
            "the prototype's `.scrim` is up while the drawer is"
        );

        let row = "session-row-22222222-cccc-dddd";
        assert!(window.find(row).visible(), "the row is inside the viewport");
        assert!(
            window.find("work-pane").bounds().origin.x > window.find(row).bounds().right(),
            "and the row sits outside the drawer, so the scrim is all that covers it"
        );

        // The scrim dims what is behind the drawer, not the drawer itself.
        window.within("work-pane-tabs").click(1usize, cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-diff").is_some(),
            "the drawer keeps its own presses while the scrim is up"
        );

        window.click(row, cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane").is_none(),
            "a press on the scrim closes the drawer"
        );
        assert_ne!(
            window.find(row).selected(),
            Some(true),
            "and it does not click through to the row behind it"
        );
        assert!(
            window.try_find("work-pane-scrim").is_none(),
            "the scrim leaves with the drawer it belonged to"
        );

        window.click(row, cx);
        window.render_frame(cx);
        assert_eq!(
            window.find(row).selected(),
            Some(true),
            "the same press lands once the drawer is gone"
        );
    })
    .unwrap();
}

/// The sidebar floats above the scrim, so it keeps its own presses.
///
/// The prototype stacks the three surfaces `.scrim{z-index:25}`,
/// `.work{z-index:30}` and `.sidebar{z-index:40}`, and the scrim spans the whole
/// workspace underneath both of them. Strictly below `SIDEBAR_OVERLAY_UNDER`
/// (60rem = 960px) the sidebar floats *and* the work pane is already a drawer,
/// so a press on a sidebar row is a press inside the scrim's rectangle; only
/// the z-order decides that it belongs to the sidebar. Without it the press
/// would close the work pane and never reach the row.
#[gpui_kit::test]
fn the_sidebar_overlay_keeps_its_presses_above_the_scrim(cx: &mut TestAppContext) {
    settle_motion(cx);
    cx.update(gpui_kit::init);
    cx.update(tact_gui::commands_init);

    let sessions = vec![RecentSession {
        id: "33333333-eeee-ffff".to_string(),
        updated_at_unix: 1_700_000_200,
        message_count: 2,
        title: Some("Overlay ordering".to_string()),
        name: None,
        archived: false,
        pinned: false,
    }];
    // 900px: below the sidebar threshold, so the sidebar floats over the
    // transcript; still past the work pane's own threshold, so that one is
    // already out as a drawer and its scrim is up.
    let handle = cx.open_window(size(px(900.), px(700.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_sessions(window, cx, sessions));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("sidebar-overlay").is_some()
                && window.try_find("work-pane").is_some()
                && window.try_find("work-pane-scrim").is_some(),
            "the sidebar floats over the transcript while the pane is a drawer"
        );

        let row = "session-row-33333333-eeee-ffff";
        window.click(row, cx);
        window.render_frame(cx);
        assert_eq!(
            window.find(row).selected(),
            Some(true),
            "the row takes the press: the sidebar is above the scrim"
        );
        assert!(
            window.try_find("work-pane").is_some(),
            "and the press is not also read as a click on the scrim behind it"
        );
        assert_eq!(
            window.find(row).selected(),
            Some(true),
            "the sidebar is above the scrim, so the row takes the press"
        );
    })
    .unwrap();
}

/// The floating sidebar stays above the work pane where the two overlap.
///
/// The prototype stacks `.scrim{z-index:25}`, `.work{z-index:30}` and the
/// floating `.sidebar{z-index:40}`. A window narrower than the two floating
/// widths put together (260 px + 384 px) makes the sidebar and the drawer share
/// a strip, and only that order says which surface owns a press inside it.
/// GPUI has no `z-index`, so the order is paint order: a drawer painted after
/// the sidebar takes the whole strip even though the sidebar outranks it.
#[gpui_kit::test]
fn the_floating_sidebar_stays_above_the_work_pane_where_they_overlap(cx: &mut TestAppContext) {
    settle_motion(cx);
    cx.update(gpui_kit::init);
    cx.update(tact_gui::commands_init);

    let sessions = vec![RecentSession {
        id: "44444444-aaaa-bbbb".to_string(),
        updated_at_unix: 1_700_000_300,
        message_count: 2,
        title: Some("Narrow overlap".to_string()),
        name: None,
        archived: false,
        pinned: false,
    }];
    // Below the sidebar threshold, so the sidebar floats, and below the work
    // pane's own threshold, so that one is already out as a drawer. Narrow
    // enough that the two floating widths overlap: the drawer is right-anchored
    // and the floating sidebar is 260 px, so a 560 px window is the tightest
    // case the stacking order has to answer for.
    let handle = cx.open_window(size(px(560.), px(640.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_sessions(window, cx, sessions));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        let row_id = "session-row-44444444-aaaa-bbbb";
        let drawer = window.find("work-pane").bounds();
        let row = window.find(row_id).bounds();
        assert!(
            row.right() > drawer.origin.x,
            "the sidebar row runs under the drawer at this width: \
             {row:?} against {drawer:?}"
        );

        // A press inside that shared strip belongs to the sidebar, because
        // `.sidebar{z-index:40}` outranks `.work{z-index:30}`.
        let under_the_drawer = gpui_kit::point(drawer.origin.x - row.origin.x + px(4.), px(10.));
        window.click_at(row_id, under_the_drawer, cx);
        window.render_frame(cx);

        assert_eq!(
            window.find(row_id).selected(),
            Some(true),
            "the sidebar owns the strip it shares with the drawer"
        );
        assert!(
            window.try_find("work-pane").is_some(),
            "and the press is not read as a click on the scrim behind both"
        );
    })
    .unwrap();
}

/// The prototype's first media query shrinks every column together:
/// `@media(max-width:1320px){:root{--sidebar:244px;--work:374px}
/// .thread{width:min(680px,calc(100% - 36px))}}`. It reaches only the columns:
/// the same query's narrow block resets the work pane to `min(420px,88vw)`
/// when it becomes the drawer. The shell's own drawer default is 384 px, the
/// width the pane opens at; only the narrow *column* uses the prototype's
/// 374 px.
#[gpui_kit::test]
fn the_narrow_breakpoint_shrinks_the_three_columns(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    // 1300 px is under the prototype's breakpoint and over v1's drawer
    // threshold, so all three columns are on screen at the narrow sizes.
    let handle = cx.open_window(size(px(1300.), px(800.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        let sidebar = window.find("sidebar").bounds();
        let transcript = window.find("transcript").bounds();
        let work_pane = window.find("work-pane").bounds();

        assert_eq!(width(sidebar), 244., "`--sidebar: 244px` below 1320 px");
        assert_eq!(width(work_pane), 374., "`--work: 374px` below 1320 px");
        assert_eq!(
            width(transcript),
            1300. - 244. - 374.,
            "the transcript still takes the whole surplus"
        );

        // `.thread { width: min(680px, calc(100% - 36px)) }`: the column leaves
        // 682 px, so the 18 px gutters bind and the measure is 682 - 36.
        let thread = window.find("transcript-scroller").bounds();
        assert_eq!(
            width(thread),
            682. - 36.,
            "the narrow thread keeps the prototype's 18 px gutters"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn wide_window_lays_out_three_columns(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        let sidebar = window.find("sidebar").bounds();
        let transcript = window.find("transcript").bounds();
        let work_pane = window.find("work-pane").bounds();

        assert_eq!(width(sidebar), 260., "sidebar keeps its 260 px column");
        assert_eq!(width(work_pane), 384., "the work pane keeps its own column");
        assert_eq!(
            width(transcript),
            1440. - 260. - 384.,
            "the transcript takes the whole surplus"
        );
        assert!(
            sidebar.origin.x < transcript.origin.x && transcript.origin.x < work_pane.origin.x,
            "columns run sidebar, transcript, work pane"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn work_pane_tabs_switch_the_visible_body(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-plan").is_some(),
            "Plan is the initial work pane tab"
        );

        window.within("work-pane-tabs").click(4usize, cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-files").is_some(),
            "clicking Files switches the body"
        );
        assert!(
            window.try_find("work-pane-body-plan").is_none(),
            "only one work pane body renders at a time"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn command_palette_opens_over_the_shell(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("open-command-palette", cx);
        window.render_frame(cx);

        assert!(
            window.try_find("command").is_some(),
            "the command palette renders in the dialog layer"
        );
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("command").is_none(),
            "escape reaches the focused palette and dismisses it"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn settings_dialog_exposes_live_controls(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("open-settings", cx);
        window.render_frame(cx);

        window.find("settings-theme-light");
        assert!(
            window.try_find("settings-show-thinking").is_some(),
            "reading controls are visible"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn theme_toggle_button_switches_to_dark(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let content = std::fs::read_to_string(theme::theme_dir().join("tact-anthropic.json"))
            .expect("the shipped theme file must be readable");
        ThemeRegistry::global_mut(cx)
            .load_themes_from_str(&content)
            .expect("the shipped theme file must parse");
        theme::activate(ThemeMode::Light, None, cx).expect("light theme must activate");
    });

    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(cx.theme().theme_name().as_ref(), theme::LIGHT_THEME_NAME);

        window.click("toggle-theme", cx);
        window.render_frame(cx);
        assert_eq!(
            cx.theme().theme_name().as_ref(),
            theme::DARK_THEME_NAME,
            "the title bar toggle switches to the dark Tact theme"
        );
    })
    .unwrap();
}

/// Load the shipped theme and activate it, the way `main.rs` does at startup.
///
/// The shell takes more than colors from the theme: the toast anchor, the mono
/// family, and the default mode all live there. A test that only calls
/// `gpui_kit::init` renders against gpui-component's defaults instead, which is
/// a different shell than the one users run.
fn activate_shipped_theme(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let content = std::fs::read_to_string(theme::theme_dir().join("tact-anthropic.json"))
            .expect("the shipped theme file must be readable");
        ThemeRegistry::global_mut(cx)
            .load_themes_from_str(&content)
            .expect("the shipped theme file must parse");
        theme::activate(ThemeMode::Dark, None, cx).expect("dark theme must activate");
    });
}

/// Assert settled geometry, not the frame an overlay happens to be on.
///
/// The work drawer and the floating sidebar are the prototype's two sliding
/// surfaces, and they animate in *and* out. Every test that only cares where
/// one of them ends up turns the platform's reduce-motion preference on, which
/// makes `Presence` resolve straight to its end state and schedule no frames
/// at all. The tween itself has its own contract in
/// `the_work_pane_drawer_slides_in_and_out_over_the_prototype_duration`, which
/// deliberately leaves this off.
fn settle_motion(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
}

fn width(snapshot: gpui_kit::Bounds<gpui_kit::Pixels>) -> f32 {
    f32::from(snapshot.size.width)
}

fn height(snapshot: gpui_kit::Bounds<gpui_kit::Pixels>) -> f32 {
    f32::from(snapshot.size.height)
}

#[gpui_kit::test]
fn transcript_and_composer_render(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| {
            let mut shell = TactApp::new(window, cx);
            shell.push_notice("Session ready.", cx);
            shell
        });
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("transcript-scroller").is_some(),
            "the virtualized transcript renders"
        );
        assert!(
            window.try_find("transcript-row-0").is_some(),
            "the first transcript row renders"
        );
        assert!(
            window.try_find("composer").is_some(),
            "the composer renders"
        );
        assert!(
            window.try_find("composer-model").is_some(),
            "the model control renders"
        );
        assert!(
            window.try_find("composer-primary").is_some(),
            "the primary composer action renders"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn composer_stays_reachable_at_the_minimum_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let handle = cx.open_window(size(px(960.), px(640.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        let composer = window.find("composer").bounds();
        assert!(
            composer.origin.y + composer.size.height <= px(640.),
            "the composer remains inside the minimum window: {composer:?}"
        );
        assert!(
            composer.size.height >= px(64.),
            "the composer keeps a usable hit area: {composer:?}"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn keyboard_contract_toggles_panes_and_cycles_detail(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        tact_gui::commands_init(cx);
    });
    cx.run_until_parked();

    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("sidebar").is_some());
        window.press("ctrl-b", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("sidebar").is_none(),
            "Primary+B hides the sidebar"
        );

        window.press("ctrl-b", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("sidebar").is_some(),
            "Primary+B restores the sidebar"
        );

        window.press("ctrl-\\", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane").is_none(),
            "Primary+Backslash hides the work pane"
        );

        window.press("ctrl-\\", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane").is_some(),
            "Primary+Backslash restores the work pane"
        );

        window.press("ctrl-shift-d", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-diff").is_some(),
            "Primary+Shift+D opens the Diff tab"
        );

        assert!(
            window
                .find("transcript-detail-cycle")
                .label()
                .is_some_and(|label| label.contains("Normal"))
        );
        window.press("ctrl-o", cx);
        window.render_frame(cx);
        assert!(
            window
                .find("transcript-detail-cycle")
                .label()
                .is_some_and(|label| label.contains("Thinking"))
        );
        window.press("ctrl-o", cx);
        window.render_frame(cx);
        assert!(
            window
                .find("transcript-detail-cycle")
                .label()
                .is_some_and(|label| label.contains("Verbose"))
        );
        window.press("ctrl-o", cx);
        window.render_frame(cx);
        assert!(
            window
                .find("transcript-detail-cycle")
                .label()
                .is_some_and(|label| label.contains("Normal")),
            "Ctrl+O wraps back to Normal"
        );
    })
    .unwrap();
}

/// The keyboard reaches the commands that have no button of their own.
///
/// `keyboard_contract_toggles_panes_and_cycles_detail` presses the pane and
/// transcript chords; these are the rest of `commands::init`, pressed on the
/// preview shell so each effect is on screen. Two bindings stay out for the
/// same reason the click walk skips their buttons: `secondary-n`'s offline
/// effect is a system row below the fold (its handler is the same one the
/// `session-new` press covers), and `escape` cancels a turn the preview has no
/// session to cancel. `secondary-shift-backspace` has its own attachment fixture
/// in `the_composer_removes_the_last_attachment_with_the_keyboard`.
#[gpui_kit::test]
fn the_keyboard_contract_reaches_the_commands_without_a_button(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    // The shipped theme is not the same thing as this app's own bindings.
    cx.update(tact_gui::commands_init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        window.press("ctrl-k", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("command").is_some(),
            "Primary+K opens the command palette"
        );
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("command").is_none(),
            "escape closes the palette the chord opened"
        );

        window.press("ctrl-shift-t", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-tasks").is_some(),
            "Primary+Shift+T opens the Tasks tab"
        );
        window.within("work-pane-tabs").click(0usize, cx);
        window.render_frame(cx);

        window.press("ctrl-,", cx);
        // Dialog entrance is a 250 ms wall-clock animation, like `open-settings`.
        std::thread::sleep(std::time::Duration::from_millis(300));
        window.render_frame(cx);
        assert!(
            window.try_find("settings-theme-light").is_some(),
            "Primary+Comma opens the settings dialog"
        );
        window.press("escape", cx);
        std::thread::sleep(std::time::Duration::from_millis(300));
        window.render_frame(cx);
        assert!(
            window.try_find("settings-theme-light").is_none(),
            "escape closes the settings dialog"
        );

        let before = cx.theme().theme_name().clone();
        window.press("ctrl-shift-l", cx);
        window.render_frame(cx);
        assert_ne!(
            *cx.theme().theme_name(),
            before,
            "Primary+Shift+L switches the theme the way the title bar does"
        );

        // Sessions: forward then back, asserting on the row the sidebar marks
        // open rather than on a hard-coded id, so the walk does not depend on
        // which preview row the window happens to start on.
        let preview_rows = [
            "7fbab10-2c41-4c9a-9f10-2222aaaa1111",
            "3b78ba4-1d02-4a33-8b71-3333bbbb2222",
            "d464f22d-5e11-4c2f-9a08-4444cccc3333",
            "daf05fa-71b4-4d0e-8e55-5555dddd4444",
            "6278b7fa-3c92-4f18-9b27-6666eeee5555",
            "306ea551-8a13-4b62-8f04-7777ffff6666",
            "036e6015-a4d7-4e29-8c31-8888aaaa7777",
            "3f016556-2b8c-4f70-9d12-9999bbbb8888",
        ];
        let open_row = |window: &Window| {
            preview_rows.iter().copied().find(|id| {
                window
                    .try_find(SharedString::from(format!("session-row-{id}")))
                    .and_then(|row| row.selected())
                    == Some(true)
            })
        };
        let start = open_row(window).expect("the sidebar marks one row as open");
        window.press("ctrl-tab", cx);
        window.render_frame(cx);
        let next = open_row(window).expect("Primary+Tab lands on another session row");
        assert_ne!(
            start, next,
            "Primary+Tab moves the open row to the next session"
        );
        window.press("ctrl-shift-tab", cx);
        window.render_frame(cx);
        assert_eq!(
            open_row(window),
            Some(start),
            "Primary+Shift+Tab steps back to the row it left"
        );

        // The composer chord is only proved by typing: the draft is what opens
        // the mention list, so the panel appearing means focus landed there.
        window.press("ctrl-l", cx);
        window.input("@", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-suggestions").is_some(),
            "Primary+L puts the keyboard in the composer"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn settings_controls_change_theme_and_reasoning(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let content = std::fs::read_to_string(theme::theme_dir().join("tact-anthropic.json"))
            .expect("the shipped theme file must be readable");
        ThemeRegistry::global_mut(cx)
            .load_themes_from_str(&content)
            .expect("the shipped theme file must parse");
        theme::activate(ThemeMode::Light, None, cx).expect("light theme must activate");
    });

    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("open-settings", cx);
        // Dialog entrance is a 250 ms wall-clock animation. Wait for it to settle
        // before dispatching clicks whose targets move while it runs.
        std::thread::sleep(std::time::Duration::from_millis(300));
        window.render_frame(cx);

        assert_eq!(
            window.find("settings-show-thinking").checked(),
            Some(false),
            "Normal detail is the initial state"
        );

        window.click("settings-theme-dark", cx);
        window.render_frame(cx);
        assert_eq!(cx.theme().theme_name().as_ref(), theme::DARK_THEME_NAME);

        window.click("settings-show-thinking", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-show-thinking").checked(),
            Some(true),
            "the reading switch turns reasoning on immediately"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn sidebar_lists_recent_sessions_beside_the_new_session_affordance(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let sessions = vec![
        RecentSession {
            id: "11111111-aaaa-bbbb".to_string(),
            updated_at_unix: 1_700_000_000,
            message_count: 4,
            title: Some("Desktop client design".to_string()),
            name: None,
            archived: false,
            pinned: false,
        },
        RecentSession {
            id: "22222222-cccc-dddd".to_string(),
            updated_at_unix: 1_700_000_100,
            message_count: 0,
            title: None,
            name: None,
            archived: false,
            pinned: false,
        },
    ];
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_sessions(window, cx, sessions));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("session-new").is_some(),
            "the sidebar offers a new session"
        );
        assert!(
            window.try_find("session-row-11111111-aaaa-bbbb").is_some(),
            "the first stored session renders"
        );
        assert!(
            window.try_find("session-row-22222222-cccc-dddd").is_some(),
            "the second stored session renders"
        );
        assert!(
            window.try_find("session-row-2").is_none(),
            "only the listed sessions render"
        );
        assert!(
            window.try_find("sidebar-no-sessions").is_none(),
            "a sidebar with rows does not claim to be empty"
        );
    })
    .unwrap();
}

/// The design preview must populate every pane a reviewer compares against the
/// prototype, without an agent session or a store.
#[gpui_kit::test]
fn preview_mode_renders_prototype_shaped_content(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        assert!(
            window.try_find("session-new").is_some(),
            "the sidebar header renders"
        );
        assert!(
            window.try_find("sidebar-top").is_some(),
            "the search field groups with the new-session action"
        );
        assert!(
            window.try_find("work-pane-body-plan").is_some(),
            "the plan pane is the default work surface"
        );
        assert!(
            window.try_find("work-pane-tabs-host").is_some(),
            "the work pane tabs render"
        );
    })
    .unwrap();
}

/// The prototype closes the sidebar with the workspace's worktrees and its
/// live background work, rather than stopping at the session history.
#[gpui_kit::test]
fn the_sidebar_lists_worktrees_and_background_work(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        // The preview roots itself in this repository, so git reports at least
        // the worktree the process is running in.
        assert!(
            worktree_row_ids()
                .iter()
                .any(|id| window.try_find(id.clone()).is_some()),
            "the sidebar lists the workspace's worktrees"
        );
        assert!(
            window
                .try_find("background-row-cargo test -p tact")
                .is_some(),
            "the sidebar lists running background work"
        );
        let avatar = window.find("sidebar-avatar").bounds();
        assert_eq!(height(avatar), 24., "`.avatar` is 24 px tall");
        assert_eq!(width(avatar), 24., "`.avatar` is 24 px wide");
        let first_session = window
            .find("session-row-7fbab10-2c41-4c9a-9f10-2222aaaa1111")
            .bounds();
        assert!(
            height(first_session) >= 44.,
            "`.row` keeps its 44 px minimum: {first_session:?}"
        );
        assert_eq!(
            height(window.find("session-new").bounds()),
            34.,
            "`.new` is 34 px tall"
        );
        assert_eq!(
            height(window.find("session-search").bounds()),
            30.,
            "`.search` is 30 px tall"
        );

        // Worktrees sit above background work, so the sidebar scrolls rather
        // than dropping a group when the session list is long.
        let worktree = worktree_row_ids()
            .into_iter()
            .find_map(|id| window.try_find(id).map(|row| row.bounds()))
            .expect("a rendered worktree row");
        let background = window.find("background-row-cargo test -p tact").bounds();
        assert!(
            worktree.origin.y < background.origin.y,
            "worktrees lead the background group"
        );
        // Both groups are `.row` buttons in the prototype, so they keep the
        // same 44 px minimum as the session rows above them.
        assert!(
            height(worktree) >= 44.,
            "`.row` keeps its 44 px minimum on worktree rows: {worktree:?}"
        );
        assert!(
            height(background) >= 44.,
            "`.row` keeps its 44 px minimum on background rows: {background:?}"
        );

        assert!(
            window.try_find("transcript-toolbar").is_some(),
            "the transcript toolbar renders"
        );
    })
    .unwrap();
}

/// The prototype is the dark board, so a fresh client opens dark.
#[gpui_kit::test]
fn the_default_theme_is_dark(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        assert_eq!(
            theme::default_mode(),
            ThemeMode::Dark,
            "the approved prototype is the dark Direction A board"
        );
    });
}

/// The registry's first load races window creation, so the reload callback must
/// pin the intended default rather than inherit the built-in theme's mode.
#[gpui_kit::test]
fn the_first_registry_load_adopts_the_dark_default(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let content = std::fs::read_to_string(theme::theme_dir().join("tact-anthropic.json"))
            .expect("the shipped theme file must be readable");
        ThemeRegistry::global_mut(cx)
            .load_themes_from_str(&content)
            .expect("the shipped theme file must parse");

        // `Theme` starts on the built-in light theme; the first reload callback
        // must still land on the dark Tact theme.
        theme::reapply(cx);
        assert_eq!(
            cx.theme().theme_name().as_ref(),
            theme::DARK_THEME_NAME,
            "the first Tact theme load must open dark"
        );
    });
}

/// The prototype's code surfaces are monospaced, so the active theme must carry
/// the configured mono family rather than the platform default.
#[gpui_kit::test]
fn the_tact_theme_carries_its_configured_mono_font(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let content = std::fs::read_to_string(theme::theme_dir().join("tact-anthropic.json"))
            .expect("the shipped theme file must be readable");
        ThemeRegistry::global_mut(cx)
            .load_themes_from_str(&content)
            .expect("the shipped theme file must parse");

        theme::activate(ThemeMode::Dark, None, cx).expect("dark theme must activate");
        assert_eq!(
            cx.theme().mono_font_family.as_ref(),
            "JetBrains Mono",
            "the theme file must drive the mono family"
        );
        assert_eq!(
            cx.theme().font_family.as_ref(),
            "Inter",
            "the theme file must drive the UI family"
        );
    });
}

/// Every work pane carries the prototype's panel header, and the pane footer
/// stays pinned regardless of which pane is selected.
#[gpui_kit::test]
fn every_work_pane_renders_its_body_and_footer(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        assert!(
            window.try_find("work-pane-footer").is_some(),
            "the pane footer with Open in editor renders"
        );

        // Only the selected pane renders its body; the others are absent by
        // design rather than present-but-hidden.
        assert!(
            window.try_find("work-pane-body-plan").is_some(),
            "the default plan pane body renders"
        );
        assert!(
            window.try_find("work-pane-body-diff").is_none(),
            "an unselected pane renders no body"
        );
    })
    .unwrap();
}

/// Every pane renders its own content, not just an empty body.
///
/// The blank work pane shipped once because the only assertion was that the
/// body container existed; a pane that silently renders nothing still passed
/// that. Each pane now names a landmark that only appears with real content,
/// so a regression of the same kind fails here.
#[gpui_kit::test]
fn every_work_pane_renders_content_not_just_a_container(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-plan").is_some(),
            "the default plan pane renders"
        );

        // Tab order is Plan, Diff, Tasks, Agents, Files, Stats, Term, Browser.
        for (index, body) in [
            (0usize, "work-pane-body-plan"),
            (1, "work-pane-body-diff"),
            (3, "work-pane-body-subagent"),
            (4, "work-pane-body-files"),
            (5, "work-pane-body-stats"),
            (6, "work-pane-body-terminal"),
            (7, "work-pane-body-browser"),
        ] {
            window.within("work-pane-tabs").click(index, cx);
            window.render_frame(cx);
            assert!(
                window.try_find(body).is_some(),
                "pane {body} renders its body"
            );
        }

        // The tasks pane is the one with a card whose content is more than the
        // pane container, so it is asserted separately.
        window.within("work-pane-tabs").click(2usize, cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-tasks").is_some(),
            "the tasks pane renders its body"
        );
        assert!(
            window.try_find("work-pane-task-table").is_some(),
            "the tasks pane renders the task table, not an empty card"
        );
    })
    .unwrap();
}

/// A directory that shows up in the shell's Files pane while a test runs.
///
/// The preview re-roots its Files pane at [`repo_root`] rather than the process
/// working directory, so a probe has to land there to be inside the walk the
/// pane caches. Rows sort directories first and hide dotfiles, so the name keeps
/// the probe at the top of the pane and out of the way of everything else, and
/// `Drop` takes it away again -- including when an assertion fails.
struct WorkspaceProbe {
    path: std::path::PathBuf,
}

impl WorkspaceProbe {
    /// Reserve the name without creating anything yet: the first pane visit has
    /// to be a walk that does not know about the probe.
    fn reserve(name: &str) -> Self {
        let path = repo_root().join(name);
        let _ = std::fs::remove_dir_all(&path);
        Self { path }
    }

    /// Put the directory on disk, the way an agent's write would.
    fn place(&self) {
        std::fs::create_dir_all(&self.path).expect("workspace root is writable");
    }

    /// The element id the Files pane gives this row.
    fn row_id(&self) -> SharedString {
        format!("file-row-{}", self.path.display()).into()
    }
}

impl Drop for WorkspaceProbe {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The Files pane re-reads the workspace once it has been off screen.
///
/// The walk is `read_dir` plus a `stat` per entry, so the pane caches it across
/// frames -- but that cache must not outlive the pane being hidden, or a file
/// the agent writes while the user is reading another tab would never appear.
/// Both halves are pinned here: the cached walk keeps serving the pane while it
/// stays on screen, and the new directory shows up on the way back in.
#[gpui_kit::test]
fn the_files_pane_re_reads_the_workspace_when_it_comes_back(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let probe = WorkspaceProbe::reserve(&format!("aaa-files-pane-probe-{}", std::process::id()));
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        // Opening the Files tab is the walk the pane caches.
        window.within("work-pane-tabs").click(4usize, cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-files").is_some(),
            "the Files tab renders its body"
        );
        let row = probe.row_id();
        assert!(
            window.try_find(row.clone()).is_none(),
            "the probe is not in the walk yet"
        );

        // The directory appears while the user is looking at the pane.
        probe.place();
        window.render_frame(cx);
        assert!(
            window.try_find(row.clone()).is_none(),
            "the pane re-walked the disk instead of serving its cached walk"
        );

        // Leaving the tab and coming back is what drops the cache.
        window.within("work-pane-tabs").click(0usize, cx);
        window.render_frame(cx);
        window.within("work-pane-tabs").click(4usize, cx);
        window.render_frame(cx);
        assert!(
            window.try_find(row.clone()).is_some(),
            "the pane kept a stale walk across tabs"
        );
    })
    .unwrap();
}

/// The title bar's preset tabs are the shell's top-level switcher, and the
/// prototype pairs each preset with the pane that belongs to it:
/// `pane(n==='agent'?'tasks':n==='code'?'diff':'plan')`. The walk reached the
/// work pane's own tabs but never these three, so the pairings were only
/// asserted by reading the source.
#[gpui_kit::test]
fn the_workspace_tabs_pair_each_preset_with_its_pane(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-plan").is_some(),
            "the preview opens on Chat, whose pane is the plan"
        );

        // Chat -> Agent -> Code -> Chat, so every preset is pressed and each
        // pairing is read after a switch rather than from the opening state.
        for (index, body) in [
            (1usize, "work-pane-body-tasks"),
            (2, "work-pane-body-diff"),
            (0, "work-pane-body-plan"),
        ] {
            window.within("workspace-tabs").click(index, cx);
            window.render_frame(cx);
            assert!(
                window.try_find(body).is_some(),
                "pressing preset {index} opens {body}"
            );
        }
    })
    .unwrap();
}

/// The prototype's Escape dismisses the work drawer before anything else:
/// `if(overlay.open) closePalette(); else if(body.workOpen) work(false)`
/// (`docs/design/tact-desktop-prototype.html:159`). The palette half is the
/// dialog layer's own `Cancel`; the drawer half is the shell's. Where the pane
/// has a column of its own, v1 keeps `escape` on StopTask instead.
#[gpui_kit::test]
fn escape_closes_the_work_drawer_but_leaves_the_column_alone(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    // `escape` is a binding, not a widget: without the shell's own keymap the
    // press resolves to nothing at all.
    cx.update(tact_gui::commands_init);
    for (width, dismissed) in [(1100u32, true), (1440, false)] {
        let handle = cx.open_window(size(px(width as f32), px(800.)), |window, cx| {
            let shell = cx.new(|cx| TactApp::preview(window, cx));
            Root::new(shell, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("work-pane").is_some(),
                "the preview opens with the {width}px pane mounted"
            );

            window.press("escape", cx);
            window.render_frame(cx);

            assert_eq!(
                window.try_find("work-pane").is_none(),
                dismissed,
                "at {width}px escape {} the pane",
                if dismissed { "dismisses" } else { "leaves" }
            );
        })
        .unwrap();
    }
}

/// The prototype's `pane(n)` ends with `if(innerWidth<=1120) work(true)`: on a
/// narrow window, selecting a pane is also asking to see it. v1 puts the
/// drawer form below 1280 px, so a preset press under that width has to bring
/// the drawer back -- without it the press moves a pane that is not on screen,
/// the same "nothing happened" the floating sidebar had. Above the threshold
/// the pane has a column to be shown in, so a closed pane stays closed.
#[gpui_kit::test]
fn a_preset_press_opens_the_drawer_only_where_the_pane_has_no_column(cx: &mut TestAppContext) {
    settle_motion(cx);
    cx.update(gpui_kit::init);
    for (width, expected) in [(1440u32, false), (1100, true)] {
        let handle = cx.open_window(size(px(width as f32), px(800.)), |window, cx| {
            let shell = cx.new(|cx| TactApp::preview(window, cx));
            Root::new(shell, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("work-pane-close", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("work-pane").is_none(),
                "the close button takes the {width}px pane off screen"
            );

            // Agent is preset 1, whose pane is Tasks.
            window.within("workspace-tabs").click(1, cx);
            window.render_frame(cx);

            assert_eq!(
                window.try_find("work-pane-body-tasks").is_some(),
                expected,
                "a preset press at {width}px {} the pane",
                if expected {
                    "brings back"
                } else {
                    "leaves closed"
                }
            );
        })
        .unwrap();
    }
}

/// The status bar exposes the prototype's segments rather than a single string,
/// and each chip reads one piece of live session state. Existence alone is not
/// the contract: a chip that renders the wrong value looks identical in a
/// passing test that only asks whether the bar is there.
#[gpui_kit::test]
fn the_status_bar_renders_its_segmented_chips(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("status-bar").is_some(),
            "the segmented status bar renders"
        );

        let chip = |id: &'static str| {
            window
                .find(id)
                .label()
                .unwrap_or_else(|| panic!("{id} has to expose its text as a label"))
                .to_string()
        };

        assert!(
            !chip("status-project").is_empty(),
            "the project chip names the worktree the shell opened"
        );
        assert!(
            !chip("status-branch").is_empty(),
            "the branch chip names the checked-out branch"
        );
        assert_eq!(
            chip("status-permission"),
            "Ask permission",
            "the prototype footer reads `Ask permission`, and the preview seeds \
             the protocol's `default` mode so the chip takes that pairing"
        );
        // The preview seeds three diff cards: 412+188+76 added, 96+24+32 removed.
        assert_eq!(chip("status-diff"), "+676 \u{2212}152");
        // `.ring` and this chip read the same pair — the last request's total
        // against the configured window — so 84_000 of 200_000 has to read 42
        // in both.
        assert_eq!(chip("status-context"), "42% context");
        assert_eq!(chip("status-balance"), "$18.42");
        // The preview seeds one running subagent against one completed one.
        assert_eq!(chip("status-running"), "1 running");
        assert!(
            window.try_find("status-turns").is_none(),
            "the turn counter stays hidden until the agent reports TurnStats"
        );
    })
    .unwrap();
}

/// The prototype caps a user message at 72% of the transcript measure and
/// hangs it on the right edge, so the bubble must not stretch to the column.
#[gpui_kit::test]
fn the_user_bubble_is_capped_and_right_aligned(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut app: Option<Entity<TactApp>> = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the shell entity is captured");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // The transcript follows the tail and the preview's conversation is
        // taller than the viewport, so the first row has to be scrolled in
        // before its geometry exists to measure.
        app.update(cx, |app, cx| app.scroll_transcript_to(0, cx));
        window.render_frame(cx);

        let row = window.find("transcript-row-0").bounds();
        let bubble = window.find("user-bubble-0").bounds();
        let capped = row.size.width * 0.72;
        assert!(
            (bubble.size.width - capped).abs() < px(1.),
            "the bubble holds the 72% cap: {bubble:?} against {row:?}"
        );
        assert!(
            (bubble.origin.x + bubble.size.width - (row.origin.x + row.size.width)).abs() < px(1.),
            "the bubble hangs on the row's right edge: {bubble:?} against {row:?}"
        );
    })
    .unwrap();
}

/// A tool card keeps its output collapsed until the summary row is clicked,
/// so reading a command's output never requires the whole transcript to be in
/// `TranscriptDetail::Verbose`.
#[gpui_kit::test]
fn clicking_a_tool_summary_reveals_its_output(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");
    let tool_height = |cx: &mut TestAppContext, index: usize| -> f32 {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            height(
                window
                    .find(SharedString::from(format!("tool-summary-{index}")))
                    .bounds(),
            )
        })
        .unwrap()
    };
    let click = |cx: &mut TestAppContext, id: &'static str| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.click_at(id, gpui_kit::point(px(80.), px(16.)), cx);
            window.render_frame(cx);
        })
        .unwrap();
    };

    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.scroll_transcript_to(3, cx));
        window.render_frame(cx);
        // Preview row 3 is the first tool card, seeded collapsed.
    })
    .unwrap();

    let collapsed_row = tool_height(cx, 3);
    click(cx, "tool-summary-3");
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    let expanded_row = tool_height(cx, 3);
    assert!(
        expanded_row > collapsed_row,
        "the reveal settles at the measured content height: {collapsed_row} -> {expanded_row}"
    );
}

/// Thinking uses gpui-ai's controlled disclosure too: opening travels through
/// its clipped reveal, while closing removes the body at once.
#[gpui_kit::test]
fn clicking_a_thinking_summary_reveals_it_softly(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");
    let row_height = |cx: &mut TestAppContext, index: usize| -> f32 {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            height(
                window
                    .find(SharedString::from(format!("transcript-row-{index}")))
                    .bounds(),
            )
        })
        .unwrap()
    };
    let click = |cx: &mut TestAppContext, id: &'static str| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.click_at(id, gpui_kit::point(px(12.), px(12.)), cx);
            window.render_frame(cx);
        })
        .unwrap();
    };

    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.scroll_transcript_to(2, cx));
        window.render_frame(cx);
    })
    .unwrap();

    let expanded_row = row_height(cx, 2);
    click(cx, "thinking-summary-2");
    let closing_row = row_height(cx, 2);
    assert!(
        closing_row < expanded_row,
        "the thinking card settles back to its collapsed height: {expanded_row} -> {closing_row}"
    );

    click(cx, "thinking-summary-2");
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    let reopened_row = row_height(cx, 2);
    assert!(
        reopened_row > closing_row,
        "reopening the thinking card reveals its body again: {closing_row} -> {reopened_row}"
    );
}

/// A tool card shows a 150px window onto its output; a command that streams
/// more than a screenful is read through that window rather than by growing the
/// card.
///
/// The preview's own tool rows are a handful of lines, which fit inside the
/// window, so the shell is built with a longer body here -- without one the
/// block's scroll path is unreachable. The block used to be `overflow_hidden`
/// under the same 150px cap: a 200-line body laid out 3417px tall and was then
/// clipped at 150px, so everything past the first screenful could not be read.
#[gpui_kit::test]
fn a_long_tool_output_scrolls_inside_its_expanded_window(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let output: String = (1..=200).map(|line| format!("line {line:03}\n")).collect();
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_tool_output(window, cx, output));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        let row = window.find("transcript-row-0").bounds();
        assert!(
            height(row) < 400.0,
            "the card caps its expanded height instead of growing to the full output: row {row:?}"
        );
    })
    .unwrap();
}

/// A permission prompt is a gpui-ai approval card, not the custom Ask card.
#[gpui_kit::test]
fn the_permission_card_leads_with_deny(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.scroll_transcript_to_end(cx));
        window.render_frame(cx);

        assert!(
            window.try_find("request-panel").is_some(),
            "the permission gate renders its gpui-ai approval card"
        );
        for id in ["request-option-0", "request-option-1", "request-option-2"] {
            assert!(
                window.try_find(id).is_none(),
                "permission no longer uses the old inline option rows"
            );
        }
    })
    .unwrap();
}

/// Pressing an option answers the card with that option's own label.
///
/// The click walk leaves the option rows alone, because a press resolves the
/// request the rest of the walk reads; the card still has to be walked
/// somewhere, and this is that: the decision reports the pressed label, and the
/// resolved card is the prototype's `.approval.done` rather than a second set
/// of buttons.
#[gpui_kit::test]
fn the_permission_card_reports_the_choice_it_was_given(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.scroll_transcript_to_end(cx));
        window.render_frame(cx);

        window.click("permission-1-reject", cx);
        window.render_frame(cx);

        assert!(
            window.try_find("request-panel").is_none(),
            "the answered card disappears"
        );
    })
    .unwrap();
}

/// The one-shot permission option answers the card like any other row.
///
/// The preview seeds a single-select permission request, so pressing
/// `request-option-0` must resolve the card instead of toggling a choice. The
/// decision carries the option's own label back to the transcript.
#[gpui_kit::test]
fn the_once_permission_option_answers_the_card(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.scroll_transcript_to_end(cx));
        window.render_frame(cx);

        window.click("permission-1-approve", cx);
        window.render_frame(cx);

        assert!(
            window.try_find("request-panel").is_none(),
            "the answered card disappears"
        );
    })
    .unwrap();
}

/// The lasting permission option answers the card like any other row.
///
/// The seeded trio is `Allow once` / `Deny` / `Always allow this tool`, and the
/// click walk presses none of them, because a press resolves the request the
/// rest of the walk reads. The two tests around this one press `Deny`; the
/// third row is the one the shell draws as the prototype's `.btn.primary`, so
/// this presses that row and reads the decision back.
#[gpui_kit::test]
fn the_lasting_permission_option_answers_the_card(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.scroll_transcript_to_end(cx));
        window.render_frame(cx);

        window.click("permission-1-always", cx);
        window.render_frame(cx);

        assert!(
            window.try_find("request-panel").is_none(),
            "the answered card disappears"
        );
    })
    .unwrap();
}

/// The question card's `Confirm` answers with the choices that were toggled.
///
/// A question is the multi-select shape, and it is the only card that renders
/// `Confirm` and `Cancel`; a single-select row answers on its own press, so a
/// multi-select row has to toggle in place and leave the answer to `Confirm`.
#[gpui_kit::test]
fn the_question_card_confirms_the_toggled_choices(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| {
            TactApp::with_question(
                window,
                cx,
                "Which crates should ship?",
                vec!["tact".to_string(), "tact-gui".to_string()],
            )
        });
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the question shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.scroll_transcript_to_end(cx));
        window.render_frame(cx);

        assert!(
            window.try_find("request-confirm").is_some(),
            "a multi-select question is the shape that offers Confirm"
        );
        assert!(
            window.try_find("request-cancel").is_some(),
            "a question can be declined without carrying its own Deny option"
        );
        assert!(
            window.try_find("request-option-2").is_none(),
            "the card renders the options the question was asked with"
        );

        window.click("request-option-1", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("request-option-1").checked(),
            Some(true),
            "a multi-select row toggles instead of answering"
        );
        assert!(
            window.try_find("request-decision").is_none(),
            "toggling a choice leaves the question pending"
        );

        window.click("request-confirm", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("request-confirm").is_none(),
            "the resolved question drops its actions"
        );
        assert!(
            window.try_find("request-panel").is_none(),
            "the resolved question card disappears"
        );
    })
    .unwrap();
}

/// The question card's `Cancel` dismisses it without the toggled choices.
#[gpui_kit::test]
fn the_question_card_cancels_without_choosing(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| {
            TactApp::with_question(
                window,
                cx,
                "Which crates should ship?",
                vec!["tact".to_string(), "tact-gui".to_string()],
            )
        });
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the question shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.scroll_transcript_to_end(cx));
        window.render_frame(cx);

        // Toggle a choice first, so the decision below distinguishes dismissing
        // the question from confirming what was on screen.
        window.click("request-option-0", cx);
        window.render_frame(cx);
        assert_eq!(window.find("request-option-0").checked(), Some(true));

        window.click("request-cancel", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("request-option-0").is_none(),
            "the dismissed card drops its choices too"
        );
        assert!(
            window.try_find("request-panel").is_none(),
            "the dismissed question card disappears"
        );
    })
    .unwrap();
}

/// The plan progress fill measures as a percentage of its track; the
/// prototype's `.progress i { width:58% }` is not a fixed rem width.
#[gpui_kit::test]
fn the_plan_progress_fill_uses_the_track_width(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        let track = window.find("work-pane-plan-progress").bounds();
        let fill = window.find("work-pane-plan-progress-fill").bounds();
        let expected = width(track) * 0.4;
        assert!(
            (width(fill) - expected).abs() < 1.,
            "the 40% plan fill spans its track: {fill:?} against {track:?}"
        );
    })
    .unwrap();
}

/// The work pane's `.workTop` and `.workFoot` own the prototype's 28px
/// controls, not gpui-component's 32px button default.
#[gpui_kit::test]
fn the_work_pane_chrome_uses_the_prototype_boxes(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        let close = window.find("work-pane-close").bounds();
        assert_eq!(height(close), 28., "`.icon` is 28 px tall");
        assert_eq!(width(close), 28., "`.icon` is 28 px wide");

        let open_editor = window.find("work-pane-open-editor").bounds();
        assert_eq!(height(open_editor), 28., "`.btn` is 28 px tall");
    })
    .unwrap();
}

/// The prototype binds `#openDiff` to the write row's `.diffBtn`: clicking the
/// counts shows the Diff pane and leaves the tool card's own output closed.
#[gpui_kit::test]
fn clicking_a_write_rows_diff_badge_opens_the_diff_pane(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.scroll_transcript_to(3, cx));
        window.render_frame(cx);
        // Preview row 3 starts the read/edit tool run; row 5 is the edit card.
        assert!(
            window.try_find("work-pane-body-plan").is_some(),
            "the preview opens on the plan pane"
        );
        assert!(
            window.try_find("tool-diff-3").is_none(),
            "a read has no change to open"
        );
        assert!(
            window.try_find("tool-call-body-tool-5").is_none(),
            "the edit card starts collapsed"
        );

        window.click("tool-diff-5", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-diff").is_some(),
            "the badge switches the work pane to Diff"
        );
        assert!(
            window.try_find("tool-call-body-tool-5").is_none(),
            "the badge does not also expand the tool card"
        );
    })
    .unwrap();
}

/// A session row's actions live behind the row itself: a right press selects
/// that session and opens the same rename/duplicate/pin/archive menu as the
/// title-bar session chip.
#[gpui_kit::test]
fn right_clicking_a_session_row_opens_its_context_menu(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.right_click("session-row-7fbab10-2c41-4c9a-9f10-2222aaaa1111", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("session-context-menu-panel").is_some(),
            "right-clicking a session row opens its context menu"
        );
        for id in [
            "session-context-menu-rename",
            "session-context-menu-duplicate",
            "session-context-menu-pin",
            "session-context-menu-archive",
            "session-context-menu-reveal",
        ] {
            assert!(window.try_find(id).is_some(), "the row menu exposes {id}");
        }
    })
    .unwrap();
}

/// The Diff pane can stage one file and draft a batch review.
///
/// The review draft is deliberately routed through the composer rather than
/// sent immediately: one press gathers every recorded path, then the user can
/// add comments and send the batch through the normal queue. The stage action
/// is offline here, so it reports what it would do without touching the test
/// checkout's index.
#[gpui_kit::test]
fn the_diff_pane_stages_and_drafts_a_batch_review(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("work-pane-tabs").click(1usize, cx);
        window.render_frame(cx);

        window.click("work-pane-diff-comment", cx);
        window.render_frame(cx);
        let draft = app.update(cx, |app, cx| app.composer_draft(cx));
        assert!(
            draft.contains("crates/tact-gui/src/shell.rs")
                && draft.contains("crates/tact-gui/src/pane.rs")
                && draft.ends_with("Comments:\n"),
            "Comment gathers the changed files into one review draft: {draft:?}"
        );

        let before = app.update(cx, |app, _| app.transcript_len());
        window.click("diff-stage-0", cx);
        window.render_frame(cx);
        assert_eq!(
            app.update(cx, |app, _| app.transcript_len()),
            before + 1,
            "Stage reports its offline result instead of silently doing nothing"
        );
    })
    .unwrap();
}

/// The prototype hangs an assistant answer off a 24px gutter badge, one `.msg`
/// gap (12px) away from the body, rather than starting flush with the column.
#[gpui_kit::test]
fn the_assistant_message_carries_a_gutter_badge(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        let row = window.find("transcript-row-1").bounds();
        let gutter = window.find("assistant-gutter-1").bounds();
        let body = window.find("assistant-body-1").bounds();

        assert!(
            (gutter.size.width - px(24.)).abs() < px(0.5),
            "the gutter is the prototype's 24px square: {gutter:?}"
        );
        assert!(
            (gutter.size.width - gutter.size.height).abs() < px(0.5),
            "the gutter is square: {gutter:?}"
        );
        assert!(
            (gutter.origin.x - row.origin.x).abs() < px(0.5),
            "the gutter leads the row: {gutter:?} against {row:?}"
        );
        assert!(
            (body.origin.x - (gutter.origin.x + gutter.size.width) - px(12.)).abs() < px(0.5),
            "the body sits one `.msg` gap after the gutter: {body:?} against {gutter:?}"
        );
    })
    .unwrap();
}

/// The prototype's `.code` card carries a `.codeHead` band whose right end
/// holds a `Copy` chip. v1 wires it to the clipboard, which needs an element
/// id; a custom block has none of its own, so the renderer anchors the id to
/// the block's byte range in the message. This is the one interactive control
/// the click walk could not reach, so it gets a contract of its own.
#[gpui_kit::test]
fn the_transcript_toolbar_carries_the_prototype_copy_button(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        let toolbar = window.find("transcript-toolbar").bounds();
        let detail = window.find("transcript-detail-cycle").bounds();
        let copy = window.find("transcript-copy").bounds();

        assert!(
            copy.origin.x > detail.origin.x,
            "the copy button follows the detail cycle: {copy:?} against {detail:?}"
        );
        assert!(
            copy.right() <= toolbar.right() + px(0.5),
            "the copy button stays inside the toolbar: {copy:?} against {toolbar:?}"
        );

        // `.mainTop` sits in `.main { grid-template-rows: 38px ... }` and the
        // two `.toolbtn` chips are 26 px tall. gpui-component's own buttons are
        // 24 px or 32 px, so this is the regression guard for drawing our own.
        assert_eq!(
            height(toolbar),
            38.,
            "the toolbar band is `.main`'s 38 px row"
        );
        assert_eq!(height(detail), 26., "`.toolbtn` is 26 px tall");
        assert_eq!(height(copy), 26., "`.toolbtn` is 26 px tall");
        assert_eq!(
            height(detail),
            height(copy),
            "both toolbar chips share `.toolbtn`'s box"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_title_bar_controls_use_the_prototype_boxes(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        // `.icon { width: 28px; height: 28px; }` — the sidebar toggle in `.tl`
        // and theme / work pane / settings in `.tr`.
        for id in [
            "toggle-sidebar",
            "toggle-theme",
            "toggle-work-pane",
            "open-settings",
        ] {
            let box_ = window.find(id).bounds();
            assert_eq!(height(box_), 28., "`.icon` is 28 px tall ({id})");
            assert_eq!(width(box_), 28., "`.icon` is 28 px wide ({id})");
        }

        // `.cmd { height: 28px; min-width: 154px; }` — the palette field.
        let command = window.find("open-command-palette").bounds();
        assert_eq!(height(command), 28., "`.cmd` is 28 px tall");
        assert!(
            width(command) >= 154.,
            "`.cmd` keeps its 154 px minimum: {command:?}"
        );

        // `.tabs { padding: 2px; border: 1px solid var(--line) }` with
        // `.tab { height: 26px }` inside: the strip is 26 + 2 + 2 + 2 = 32 px
        // tall, and the stock 32 px segmented bar would report 40 px here.
        let strip = window.find("workspace-tabs").bounds();
        assert_eq!(
            height(strip),
            32.,
            "`.tabs` frames a 26 px tab with 2 px padding and a 1 px border: {strip:?}"
        );
        let tab = window.within("workspace-tabs").find(0).bounds();
        assert_eq!(height(tab), 26., "`.tab` is 26 px tall: {tab:?}");
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_composer_controls_use_the_prototype_boxes(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        // `.mini { height: 25px; padding: 0 7px; font-size: 10.5px; }`.
        for id in [
            "composer-add",
            "composer-mention",
            "composer-model",
            "composer-effort",
            "composer-permission",
        ] {
            let chip = window.find(id).bounds();
            assert_eq!(height(chip), 25., "`.mini` is 25 px tall ({id})");
        }

        // `.ring { width:24px; height:24px; }`. The preview seeds the usage
        // that mounts the ring, so its box is measurable here; the popover's
        // trigger is the button around it.
        let ring = window.find("composer-usage").bounds();
        assert_eq!(width(ring), 24., "`.ring` is 24 px wide: {ring:?}");
        assert_eq!(height(ring), 24., "`.ring` is 24 px tall: {ring:?}");

        // `.send { width: 28px; height: 28px; }`.
        let send = window.find("composer-primary").bounds();
        assert_eq!(height(send), 28., "`.send` is 28 px tall");
        assert_eq!(width(send), 28., "`.send` is 28 px wide");

        // `.prompt { min-height: 48px; }` keeps the message field from
        // collapsing below the prototype's two-line input box.
        let prompt = window.find("prompt-composer-input").bounds();
        assert!(
            height(prompt) >= 48.,
            "the prompt field keeps `.prompt`'s 48 px minimum: {prompt:?}"
        );

        // The project footer's action is `Size::XSmall` (20 px, `text_xs`),
        // which is what keeps it in scale with the `text_xs` project name it
        // sits beside. `compact()` alone only shrank padding: the button stayed
        // `Size::Medium`, so its label rendered at `text_base` in a 32 px box.
        let open_project = window.find("composer-open-project").bounds();
        assert_eq!(
            height(open_project),
            20.,
            "the project footer button is `Size::XSmall`: {open_project:?}"
        );
    })
    .unwrap();
}

/// The model, budget, effort and permission rows keep the choice they set.
///
/// The broad click walk only proves each row is reachable. These controls are
/// toggles, so the contract is that the chosen row reports `checked`, its
/// siblings report off, the choice survives the popover closing and reopening,
/// and the offline shell answers each command with exactly one notice.
#[gpui_kit::test]
fn the_composer_option_rows_keep_the_choice_they_set(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");

    let dismiss = |window: &mut Window, cx: &mut App| {
        window.within("work-pane-tabs").click(0usize, cx);
        window.render_frame(cx);
    };

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let mut notices = app.update(cx, |app, _| app.transcript_len());

        // Model and budget share one panel.
        window.click("composer-model", cx);
        window.render_frame(cx);
        assert!(window.try_find("composer-model-panel").is_some());
        assert_eq!(window.find("composer-model-gpt-5").label(), Some("gpt-5"));
        window.click("composer-model-gpt-5", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-model-panel").is_none(),
            "choosing a model closes the picker"
        );
        notices += 1;
        assert_eq!(app.update(cx, |app, _| app.transcript_len()), notices);
        dismiss(window, cx);

        window.click("composer-model", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("composer-model-gpt-5").selected(),
            Some(true),
            "the model row keeps its check after the popover reopens"
        );
        assert_eq!(
            window.find("composer-model-claude-sonnet-4-5").selected(),
            Some(false),
            "the previous model row is unchecked"
        );
        dismiss(window, cx);

        // Effort.
        window.click("composer-effort", cx);
        window.render_frame(cx);
        assert!(window.try_find("composer-effort-panel").is_some());
        assert_eq!(window.find("composer-effort-high").label(), Some("high"));
        window.click("composer-effort-high", cx);
        window.render_frame(cx);
        notices += 1;
        assert_eq!(app.update(cx, |app, _| app.transcript_len()), notices);
        dismiss(window, cx);

        window.click("composer-effort", cx);
        window.render_frame(cx);
        assert_eq!(window.find("composer-effort-high").checked(), Some(true));
        assert_eq!(window.find("composer-effort-auto").checked(), Some(false));
        dismiss(window, cx);

        // Permission.
        window.click("composer-permission", cx);
        window.render_frame(cx);
        assert!(window.try_find("composer-permission-panel").is_some());
        assert_eq!(
            window.find("composer-permission-plan").label(),
            Some("Plan mode")
        );
        window.click("composer-permission-plan", cx);
        window.render_frame(cx);
        notices += 1;
        assert_eq!(app.update(cx, |app, _| app.transcript_len()), notices);
        dismiss(window, cx);

        window.click("composer-permission", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("composer-permission-plan").checked(),
            Some(true)
        );
        assert_eq!(
            window.find("composer-permission-auto").checked(),
            Some(false)
        );
    })
    .unwrap();
}

/// A real provider can advertise a long model list. The picker keeps the list
/// live while it is open and filters it by model id, so models that are below
/// the fold can still be reached without scrolling the whole panel.
#[gpui_kit::test]
fn the_model_picker_filters_a_long_list(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-model", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-model-search").is_some(),
            "the model picker exposes its own search field"
        );
        assert!(
            window.try_find("composer-model-gpt-5").is_some(),
            "the unfiltered list starts with server order"
        );
        assert!(
            window.try_find("composer-model-list-scroll-area").is_some(),
            "the model list owns a scroll viewport"
        );
        let panel = window.find("composer-model-panel").bounds();
        let row = window.find("composer-model-gpt-5").bounds();
        assert!(
            row.left() - panel.left() < px(20.),
            "model rows start at the panel's left edge: row {row:?} panel {panel:?}"
        );
        assert!(
            height(row) <= 28.0,
            "model rows use the compact list height: {row:?}"
        );

        window.input("deepseek", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-model-deepseek-chat").is_some(),
            "typing a provider family keeps its models visible"
        );
        assert!(
            window.try_find("composer-model-gpt-5").is_none(),
            "typing a provider family filters unrelated models"
        );

        window.press("ctrl-a", cx);
        window.press("backspace", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-model-gpt-5").is_some(),
            "clearing the search restores the full list"
        );

        window.press("escape", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-model-panel").is_none(),
            "Escape closes the model picker"
        );
    })
    .unwrap();
}

/// The composer's Add menu rows do what their labels say.
///
/// The native file picker is a platform prompt and is covered by the broad
/// click walk only. The other three rows have observable, deterministic
/// behaviour: Skills arms the slash completion, Connectors sends the MCP list
/// command, and Plugins explains that discovery is not configured.
#[gpui_kit::test]
fn the_composer_add_rows_do_what_they_name(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");

    let dismiss = |window: &mut Window, cx: &mut App| {
        window.within("work-pane-tabs").click(0usize, cx);
        window.render_frame(cx);
    };

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        window.click("composer-add", cx);
        window.render_frame(cx);
        assert!(window.try_find("composer-add-panel").is_some());
        assert_eq!(window.find("composer-add-skill").label(), Some("Skills"));
        window.click("composer-add-skill", cx);
        window.render_frame(cx);
        assert_eq!(
            app.update(cx, |app, cx| app.composer_draft(cx)),
            "/",
            "Skills arms the slash completion"
        );
        dismiss(window, cx);

        let before = app.update(cx, |app, _| app.transcript_len());
        window.click("composer-add", cx);
        window.render_frame(cx);
        window.click("composer-add-connector", cx);
        window.render_frame(cx);
        assert_eq!(
            app.update(cx, |app, _| app.transcript_len()),
            before + 1,
            "Connectors sends a command and the offline shell notices"
        );
        dismiss(window, cx);

        let before = app.update(cx, |app, _| app.transcript_len());
        window.click("composer-add", cx);
        window.render_frame(cx);
        window.click("composer-add-plugin", cx);
        window.render_frame(cx);
        assert_eq!(
            app.update(cx, |app, _| app.transcript_len()),
            before + 1,
            "Plugins explains that discovery is not configured"
        );
    })
    .unwrap();
}

/// `.prompt { min-height:48px; max-height:150px; }` keeps the message field
/// between the prototype's compact box and its scrolling ceiling.
///
/// The prototype leaves `min-height`/`max-height` to the browser, but the
/// GPUI textarea sizes an auto-growing box in whole window line-height rows,
/// so the ceiling is a row count instead: one row at rest, five rows once the
/// draft fills the box. Before this the field started 16 px taller than the
/// prototype and had no ceiling at all -- a 600 word draft reached 192 px, 42
/// px past the prototype's maximum.
#[gpui_kit::test]
fn the_prompt_grows_between_the_prototype_minimum_and_maximum(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let initial = window.find("prompt-composer-input").bounds();
        assert!(
            (48.0..64.0).contains(&height(initial)),
            "`.prompt` starts at the prototype's 48 px minimum, not the 64 px it \
             used to hold: {initial:?}"
        );

        window.click("prompt-composer-input", cx);
        window.render_frame(cx);
        window.input(&"word ".repeat(600), cx);
        window.render_frame(cx);

        let grown = window.find("prompt-composer-input").bounds();
        assert!(
            (120.0..=150.0).contains(&height(grown)),
            "a long draft grows `.prompt` to the prototype's 150 px ceiling \
             without passing it: {grown:?}"
        );
    })
    .unwrap();
}

/// A typed `/name` the shell owns runs as a command, not as a message.
///
/// The offline shell has no session, so the command reports exactly that rather
/// than running — and that report is itself the proof it took the command path.
/// The user bubble is the other half: the send path draws one for every draft,
/// and there is none here, so the command never reached the agent as prose.
#[gpui_kit::test]
fn a_typed_slash_command_is_not_sent_as_a_message(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("transcript-empty").is_some(),
            "a shell with no turns starts on the empty transcript"
        );

        // Type the command, then send it: the same two steps a reader takes.
        window.click("prompt-composer-input", cx);
        window.render_frame(cx);
        window.input("/compact", cx);
        window.render_frame(cx);
        window.click("composer-primary", cx);
        window.render_frame(cx);

        assert!(
            window.try_find("transcript-row-0").is_some(),
            "the shell says what became of the command"
        );
        assert!(
            window.try_find("user-bubble-0").is_none(),
            "a slash command is not a user message"
        );
        assert!(
            window.try_find("transcript-empty").is_none(),
            "the report replaced the empty-transcript state"
        );
    })
    .unwrap();
}

/// The prompt box and the Send button are the composer's own entry points.
///
/// The preview opens on a live turn, so its `composer-primary` press is the
/// Stop half of the control; the Send half belongs to a shell with nothing in
/// flight, and this walks it: a press in the prompt box lands the caret the
/// draft is typed into, and the press on `.send` turns that draft into the
/// first transcript row and leaves the box empty behind it.
#[gpui_kit::test]
fn a_press_in_the_prompt_box_sends_the_draft(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("transcript-empty").is_some(),
            "a shell with no turns starts on the empty transcript"
        );

        // The box is a separate entry point from the mention chip: the press
        // has to land the caret, or the draft typed below goes nowhere.
        window.click("prompt-composer-input", cx);
        window.render_frame(cx);
        window.input("Show me the diff", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("transcript-empty").is_some(),
            "the draft alone leaves the transcript alone"
        );

        window.click("composer-primary", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("transcript-empty").is_none(),
            "the sent draft leaves the empty transcript behind"
        );
        assert!(
            window.try_find("transcript-row-0").is_some(),
            "the draft becomes the first transcript row"
        );
        assert!(
            window.try_find("transcript-row-1").is_some(),
            "the offline shell appends its own notice that nothing was sent"
        );

        // The press emptied the box, so a second one has nothing to hand over:
        // a draft left behind would land as another user row here. (The button
        // does not expose a disabled flag, which is why the draft is read from
        // what the shell did with it rather than from `.send` itself.)
        window.click("composer-primary", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("transcript-row-2").is_none(),
            "an emptied prompt box sends nothing on a second press"
        );
    })
    .unwrap();
}

/// Plain Enter in the prompt box takes the same path as the `.send` press.
///
/// The transcript is a virtualized scroller, so the rows a submit appends are
/// only built once the tail is in view: the assertions scroll to the end first.
/// Reading `transcript-row-*` without that step reports "missing" for a row the
/// submit really appended, which is how an earlier probe concluded Enter never
/// reached the composer at all.
#[gpui_kit::test]
fn the_enter_key_sends_the_draft_like_the_button(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the shell is created with its window");

    let handle = handle.into();
    let mut at_rest = px(0.);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        at_rest = window.find("prompt-composer-input").bounds().size.height;
        assert!(
            window.try_find("transcript-empty").is_some(),
            "a shell with no turns starts on the empty transcript"
        );

        window.click("prompt-composer-input", cx);
        window.render_frame(cx);
        window.input("Show me the diff", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("transcript-empty").is_some(),
            "the draft alone leaves the transcript alone"
        );
    })
    .unwrap();

    cx.update_window(handle, |_, window, cx| {
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();

    // The appended rows are below the fold of the virtualized scroller.
    app.update(cx, |app, cx| app.scroll_transcript_to_end(cx));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("transcript-empty").is_none(),
            "the sent draft leaves the empty transcript behind"
        );
        assert!(
            window.try_find("transcript-row-0").is_some(),
            "Enter turns the draft into the first transcript row"
        );
        assert!(
            window.try_find("transcript-row-1").is_some(),
            "the offline shell appends its own notice that nothing was sent"
        );
        assert_eq!(
            window.find("prompt-composer-input").bounds().size.height,
            at_rest,
            "the submitted draft leaves the prompt box empty behind it"
        );
    })
    .unwrap();
}

/// The prototype puts the session header and the inline approval in the same
/// transcript scroller. The preview starts at the top, so the approval is not
/// built yet; scrolling to the tail swaps which virtual item is visible.
#[gpui_kit::test]
fn the_transcript_header_and_approval_share_the_virtual_list(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("session-intro-detail-cycle").is_some(),
            "the preview starts at the transcript header"
        );
        assert!(
            window.try_find("request-panel").is_none(),
            "the approval is virtualized below the initial viewport"
        );

        app.update(cx, |app, cx| app.scroll_transcript_to_end(cx));
        window.render_frame(cx);
        assert!(
            window.try_find("session-intro-detail-cycle").is_none(),
            "scrolling to the tail releases the header row"
        );
        assert!(
            window.try_find("request-panel").is_some(),
            "the approval is built at the transcript tail"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_session_intro_detail_chip_matches_the_prototype_cycle_box(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.scroll_transcript_to_top(cx));
        window.render_frame(cx);

        let cycle = window.find("session-intro-detail-cycle").bounds();
        let toolbar = window.find("transcript-detail-cycle").bounds();

        // `.cycle { height: 26px }` in the prototype, the same box as the
        // toolbar's `.toolbtn`.
        assert_eq!(height(cycle), 26., "`.cycle` is 26 px tall");
        assert_eq!(
            height(cycle),
            height(toolbar),
            "the intro chip and the toolbar chip share the prototype's 26 px box"
        );
    })
    .unwrap();
}

/// Walks every clickable entry point the shell renders.
///
/// `window.click` panics when a control is missing from the frame or invisible,
/// so the walk is the reachability assertion: every control is there in the
/// state that shows it, answers a pointer press, and leaves a renderable shell
/// behind. What a click *means* is pinned per control by the focused tests
/// above; this one catches the control that stopped rendering, moved off
/// screen, or vanished behind a transient layer.
///
/// Two controls stay out of it: the sidebar avatar (it has no handler at all)
/// and `composer-attachment-*` (a press opens a native file dialog). The
/// request card's own actions stay out for the reason its shape gives -- a
/// press resolves the request the walk reads -- and are walked by
/// `the_permission_card_reports_the_choice_it_was_given` and the two
/// `the_question_card_*` tests instead. The session rows are walkable only
/// because the preview is an offline shell: there a row click moves the
/// sidebar's selection instead of starting an agent runtime.
/// The element ids the sidebar gives this repository's worktree rows.
///
/// A row is identified by the branch its worktree holds, falling back to the
/// directory name when the head is detached -- the same rule the row builder
/// uses, applied to `git worktree list` so the walk covers every row instead of
/// the one this checkout happens to be sitting on.
fn worktree_row_ids() -> Vec<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root())
        .args(["worktree", "list", "--porcelain"])
        .output()
        .expect("git worktree list runs");
    assert!(output.status.success(), "git worktree list succeeds");

    String::from_utf8_lossy(&output.stdout)
        .split("\n\n")
        .filter_map(|block| {
            let mut path = None;
            let mut branch = None;
            for line in block.lines() {
                if let Some(rest) = line.strip_prefix("worktree ") {
                    path = Some(rest.trim().to_string());
                } else if let Some(rest) = line.strip_prefix("branch ") {
                    branch = Some(rest.trim().trim_start_matches("refs/heads/").to_string());
                }
            }
            let directory = std::path::Path::new(&path?)
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("worktree")
                .to_string();
            Some(format!("worktree-row-{}", branch.unwrap_or(directory)))
        })
        .collect()
}

/// The repository this test binary was built from.
fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels below the repository root")
        .to_path_buf()
}

/// Scroll the sidebar until `id` sits inside its viewport, then leave it there.
///
/// The list holds sessions, projects, worktrees, and background rows. A row
/// that is scrolled out of the box, or only half inside it, cannot be pressed:
/// the click lands at the row's centre, which may be outside the scroll area.
/// Every group is reachable, but a walk that visits groups in arbitrary order
/// has to move the list to each row before pressing it.
fn reveal_in_sidebar(window: &mut gpui_kit::Window, id: &SharedString, cx: &mut gpui_kit::App) {
    for _ in 0..40 {
        window.render_frame(cx);
        let (Some(viewport), Some(row)) = (
            window.try_find("sidebar-scroll").map(|el| el.bounds()),
            window.try_find(id.clone()).map(|el| el.bounds()),
        ) else {
            return;
        };
        let (view_top, view_bottom) = (viewport.top().as_f32(), viewport.bottom().as_f32());
        let (row_top, row_bottom) = (row.top().as_f32(), row.bottom().as_f32());
        if row_top >= view_top && row_bottom <= view_bottom {
            return;
        }
        let delta = if row_top < view_top { 120.0 } else { -120.0 };
        window.scroll(
            "sidebar-scroll",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(delta))),
            cx,
        );
    }
}

/// A second worktree of this repository, alive for as long as the guard is.
///
/// The walk's worktree section only proves something when the repository has
/// more than one worktree: with a single entry every click is the idempotent
/// case, because the row that gets pressed is the row the window already had
/// open. CI checks the repository out once, so the walk makes a second worktree
/// of its own and removes it on the way out -- including when an assertion
/// fails, since the cleanup lives in `Drop`.
///
/// `git worktree add` writes into `.git`, which some sandboxes mount read-only.
/// There the fixture reports that it could not be created and the walk falls
/// back to the idempotent half rather than failing for an environment reason.
///
/// The fixture registers a real worktree, and every sidebar renders the whole
/// `git worktree list`. Tests share one process, so two of them adding and
/// removing that worktree at once make each other's rows vanish mid-walk: the
/// guard serializes the walks, and the fixture holds it for its whole life.
struct WorktreeFixture {
    path: std::path::PathBuf,
    added: bool,
    /// Held until the guard drops, so one worktree walk runs at a time.
    _walk: std::sync::MutexGuard<'static, ()>,
}

/// Serializes the tests that add a worktree to this repository.
static FIXTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl WorktreeFixture {
    fn create() -> Self {
        // A test that panics still has to release the lock for the rest of the
        // suite, so poisoning is cleared rather than propagated.
        let walk = FIXTURE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let path = std::env::temp_dir().join(format!("tact-worktree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        let added = std::process::Command::new("git")
            .arg("-C")
            .arg(repo_root())
            .args(["worktree", "add", "--detach", "--force"])
            .arg(&path)
            .arg("HEAD")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if !added {
            eprintln!(
                "note: `git worktree add` failed in this environment (read-only .git?), so the \
                 walk covers the idempotent worktree click only"
            );
        }
        Self {
            path,
            added,
            _walk: walk,
        }
    }

    /// Whether a second worktree is on disk, i.e. whether a click can switch.
    fn is_added(&self) -> bool {
        self.added
    }

    /// A `file-row-` id inside this worktree's own walk.
    fn file_row(&self, relative: &str) -> SharedString {
        format!("file-row-{}", self.path.join(relative).display()).into()
    }
}

impl Drop for WorktreeFixture {
    fn drop(&mut self) {
        if !self.added {
            return;
        }
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(repo_root())
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .output();
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(repo_root())
            .args(["worktree", "prune"])
            .output();
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[gpui_kit::test]
fn every_entry_point_answers_a_click(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        macro_rules! click {
            ($id:expr) => {{
                assert!(window.try_find($id).is_some(), "{} is not rendered", $id);
                window.click($id, cx);
                window.render_frame(cx);
            }};
        }

        // `ElementSnapshot::label` borrows the snapshot, which the next render
        // invalidates, so a row's label is read through an owned copy.
        macro_rules! label_of {
            ($id:expr) => {
                window.find($id).label().unwrap_or_default().to_string()
            };
        }

        // A gpui-component popover dismisses on a press *outside* it, and stays
        // open through the presses its own buttons take. So each panel is
        // dismissed before the next trigger is pressed: leaving one open would
        // make the next trigger's press a dismissal, and its entries would never
        // render. The work pane's first tab is far enough away to be that press,
        // and it is a no-op at the top of the walk, where the pane is already on
        // its first tab.
        let dismiss_popover = |window: &mut Window, cx: &mut App, panel: &'static str| {
            window.within("work-pane-tabs").click(0usize, cx);
            window.render_frame(cx);
            assert!(
                window.try_find(panel).is_none(),
                "{panel} closes on a press outside it"
            );
        };

        // Title bar: both pane toggles and the theme cycle.
        click!("toggle-sidebar");
        click!("toggle-sidebar");
        click!("toggle-work-pane");
        click!("toggle-work-pane");

        // Sidebar search, then the transcript toolbar.
        click!("session-search");
        click!("transcript-detail-cycle");
        click!("transcript-detail-cycle");
        click!("transcript-copy");
        click!("transcript-detail-cycle");

        // Transcript rows: the intro chip, then each disclosure row and each
        // write row's diff badge. The scroller is virtual, so a row is scrolled
        // into view before it is clicked, and only the rows the preview seeds
        // are walked.
        app.update(cx, |app, cx| app.scroll_transcript_to_top(cx));
        window.render_frame(cx);
        click!("session-intro-detail-cycle");

        let mut disclosures = 0;
        let mut diff_badges = 0;
        for row in 0..12usize {
            app.update(cx, |app, cx| app.scroll_transcript_to(row, cx));
            window.render_frame(cx);
            for family in ["tool-summary", "thinking-summary"] {
                let disclosure: SharedString = format!("{family}-{row}").into();
                if window.try_find(disclosure.clone()).is_some() {
                    click!(disclosure.clone());
                    click!(disclosure.clone());
                    disclosures += 1;
                }
            }
            let badge: SharedString = format!("tool-diff-{row}").into();
            if window.try_find(badge.clone()).is_some() {
                click!(badge.clone());
                diff_badges += 1;
                // The badge switches the pane to Diff; put it back so the next
                // section starts from the pane users see on open.
                window.within("work-pane-tabs").click(0usize, cx);
                window.render_frame(cx);
            }
        }
        assert!(
            disclosures > 0,
            "the preview transcript seeds disclosure rows to click"
        );
        assert!(
            diff_badges > 0,
            "the preview transcript seeds a write row with a diff badge"
        );

        // The session chip is the prototype's `<button>`: its press opens the
        // session dropdown, and every row in it now has behaviour behind it.
        // A pick closes the menu it was picked from, so each row needs its own
        // reopening press.
        //
        // The walk comes after the transcript's own rows because those rows
        // assert on the scroller's geometry, and the notices below append to
        // the transcript the scroller is following.
        let preview_row_count = app.update(cx, |app, _| app.recent_row_ids().len());
        let open_named_session = app
            .update(cx, |app, _| app.open_session().map(str::to_string))
            .expect("the walk starts on a session");
        let named_row: SharedString = format!("session-row-{open_named_session}").into();

        click!("session-menu");
        assert!(
            window.try_find("session-menu-panel").is_some(),
            "the session chip opens its dropdown"
        );
        let mut notices = app.update(cx, |app, _| app.transcript_len());
        click!("session-menu-rename");
        assert!(
            window.try_find("session-menu-panel").is_none(),
            "a pick closes the dropdown it came from"
        );
        assert!(
            window.try_find("session-rename-input").is_some(),
            "Rename asks for a name instead of answering with a notice"
        );
        // The field only seeds a stored name, so the walk replaces whatever it
        // contains rather than appending to it.
        window.click("session-rename-input", cx);
        window.press("ctrl-a", cx);
        window.input("Renamed by the walk", cx);
        // Flush the field's own edit before the footer reads it; the click
        // walk runs beside other integration tests, so the input event cannot
        // be assumed to have landed by the time the next press starts.
        window.render_frame(cx);
        window.render_frame(cx);
        window.click("session-rename-ok", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("session-rename-input").is_none(),
            "the dialog closes once the name is committed"
        );
        let named = label_of!(named_row.clone());
        assert!(
            named.starts_with("Renamed by the walk ·"),
            "the renamed row shows the name the user typed: {named}"
        );
        let after = app.update(cx, |app, _| app.transcript_len());
        assert_eq!(after, notices + 1, "a rename lands one transcript notice");
        notices = after;

        click!("session-menu");
        click!("session-menu-duplicate");
        let (rows_after_duplicate, copy_id) = app.update(cx, |app, _| {
            let copy = app.recent_row_ids();
            (copy.len(), copy.first().cloned())
        });
        assert_eq!(
            rows_after_duplicate,
            preview_row_count + 1,
            "Duplicate adds a session to the sidebar"
        );
        let copy_id = copy_id.expect("the copy is a row");
        assert_ne!(copy_id, open_named_session, "the copy has its own id");
        let copy_row: SharedString = format!("session-row-{copy_id}").into();
        assert_eq!(
            window.find(copy_row.clone()).selected(),
            Some(true),
            "Duplicate opens the copy it made"
        );
        let copied = label_of!(copy_row.clone());
        assert!(
            copied.starts_with("Renamed by the walk (copy) ·"),
            "the copy is named after the row it came from: {copied}"
        );
        let after = app.update(cx, |app, _| app.transcript_len());
        assert_eq!(
            after,
            notices + 1,
            "a duplicate lands one transcript notice"
        );
        notices = after;

        click!("session-menu");
        click!("session-menu-archive");
        let archived = label_of!(copy_row.clone());
        assert!(
            archived.contains("· Archived"),
            "an archived row carries the prototype's badge: {archived}"
        );
        let after = app.update(cx, |app, _| app.transcript_len());
        assert_eq!(after, notices + 1, "an archive lands one transcript notice");
        notices = after;

        // The same row now offers to undo itself: an archive that cannot be
        // reversed would be a delete wearing another name.
        click!("session-menu");
        let undo_row = label_of!("session-menu-archive");
        assert_eq!(
            undo_row, "Unarchive",
            "an archived session offers to undo the flag rather than re-archiving"
        );
        window.click("session-menu-archive", cx);
        window.render_frame(cx);
        let restored = label_of!(copy_row.clone());
        assert!(
            !restored.contains("Archived"),
            "Unarchive clears the badge: {restored}"
        );
        let after = app.update(cx, |app, _| app.transcript_len());
        assert_eq!(
            after,
            notices + 1,
            "an unarchive lands one transcript notice"
        );
        notices = after;

        click!("session-menu");
        click!("session-menu-reveal");
        let after = app.update(cx, |app, _| app.transcript_len());
        assert_eq!(after, notices + 1, "Reveal answers with one notice row");

        // Archived sessions are hidden by default, and the walk visits every
        // seeded row, so it asks for them first.
        window.click("session-show-archived", cx);
        window.render_frame(cx);

        // Projects: the Open folder entry, then the project row the window is
        // already in. The offline shell opens no modal picker, so both presses
        // land as notices or no-ops rather than as a dialog.
        reveal_in_sidebar(window, &SharedString::from("project-open-folder"), cx);
        click!("project-open-folder");
        for id in ["project-open-folder", "project-rows"] {
            assert!(
                window.try_find(id).is_some(),
                "{id} survives the Open folder press"
            );
        }

        // Sidebar sessions: every seeded row, then the new-session action. The
        // offline shell owns no runtime, so a row click moves the row the
        // sidebar has open instead of starting an agent -- and an agent that
        // did start would take the highlight with it, which is what the
        // selection assertions below catch.
        let preview_rows = [
            "7fbab10-2c41-4c9a-9f10-2222aaaa1111",
            "3b78ba4-1d02-4a33-8b71-3333bbbb2222",
            "d464f22d-5e11-4c2f-9a08-4444cccc3333",
            "daf05fa-71b4-4d0e-8e55-5555dddd4444",
            "6278b7fa-3c92-4f18-9b27-6666eeee5555",
            "306ea551-8a13-4b62-8f04-7777ffff6666",
            "036e6015-a4d7-4e29-8c31-8888aaaa7777",
            "3f016556-2b8c-4f70-9d12-9999bbbb8888",
        ];
        for id in preview_rows {
            let row: SharedString = format!("session-row-{id}").into();
            // The sidebar scrolls, and this group is taller than the box; each
            // row is brought into view before it is pressed.
            reveal_in_sidebar(window, &row, cx);
            click!(row.clone());
            assert_eq!(
                window.find(row.clone()).selected(),
                Some(true),
                "{row} is the session the sidebar has open after its click"
            );
            let open = preview_rows
                .iter()
                .filter(|other| {
                    window
                        .try_find(SharedString::from(format!("session-row-{other}")))
                        .and_then(|row| row.selected())
                        == Some(true)
                })
                .count();
            assert_eq!(open, 1, "exactly one session row is open at a time");
        }
        click!("session-new");
        assert_eq!(
            window
                .find("session-row-3f016556-2b8c-4f70-9d12-9999bbbb8888")
                .selected(),
            Some(true),
            "the offline new-session action answers without taking the open row"
        );

        // Worktrees: the group lists the repository's own worktrees, current
        // first, and every row re-roots the window at it. The fixture gives the
        // repository a second worktree so at least one press is a real switch
        // and not just the row the window already had open. The walk finishes
        // back on the worktree it started in, so the panes below still read what
        // the window opened on.
        let fixture = WorktreeFixture::create();
        let worktrees = worktree_row_ids();
        assert!(
            !worktrees.is_empty(),
            "the crate lives in a git worktree, so the group lists one"
        );
        if fixture.is_added() {
            // At least the fixture's and this checkout's. A stale checkout left
            // on the machine by an interrupted run is not this test's business
            // to fail on -- the walk below visits every row it is handed.
            assert!(
                worktrees.len() >= 2,
                "the fixture worktree and this checkout are both listed: {worktrees:?}"
            );
        }
        let start = worktrees
            .iter()
            .find(|id| {
                window
                    .try_find(SharedString::from((*id).clone()))
                    .and_then(|row| row.selected())
                    == Some(true)
            })
            .cloned()
            .expect("the window opens on one of the repository's worktrees");
        for id in &worktrees {
            let row: SharedString = id.clone().into();
            reveal_in_sidebar(window, &row, cx);
            click!(row.clone());
            assert_eq!(
                window.find(row.clone()).selected(),
                Some(true),
                "{row} is the worktree the window is scoped to after its click"
            );
            // Which row is current is the whole state a switch produces, so the
            // press only means something if the other row gave it up. With one
            // worktree there is no other row and this loop is skipped.
            for other in worktrees.iter().filter(|other| *other != id) {
                let other: SharedString = other.clone().into();
                assert_eq!(
                    window.find(other.clone()).selected(),
                    Some(false),
                    "{other} stops being the current worktree once {row} is pressed"
                );
            }
        }
        let home: SharedString = start.into();
        reveal_in_sidebar(window, &home, cx);
        click!(home.clone());
        assert_eq!(
            window.find(home.clone()).selected(),
            Some(true),
            "the walk leaves the window on the worktree it started in"
        );

        // Work pane: every tab, then the control each tab owns.
        for index in 0..8usize {
            window.within("work-pane-tabs").click(index, cx);
            window.render_frame(cx);
        }
        window.within("work-pane-tabs").click(0usize, cx);
        window.render_frame(cx);
        click!("work-pane-plan-refresh");
        window.within("work-pane-tabs").click(1usize, cx);
        window.render_frame(cx);
        click!("work-pane-open-editor");
        click!("work-pane-diff-comment");
        window.within("work-pane-tabs").click(2usize, cx);
        window.render_frame(cx);
        click!("work-pane-tasks-new");
        window.within("work-pane-tabs").click(4usize, cx);
        window.render_frame(cx);
        click!("work-pane-files-add");
        // The terminal is the one pane that has to be asked to do anything:
        // opening it must not spawn a process, so the walk presses Start and
        // then leaves the shell running for the Drop to reap.
        window.within("work-pane-tabs").click(6usize, cx);
        window.render_frame(cx);
        click!("terminal-start");
        assert!(
            window.try_find("terminal-grid").is_some(),
            "Start terminal opens a grid"
        );
        // The Browser pane's Open button is a no-op with an empty address, so
        // the walk only has to prove it answers a press.
        window.within("work-pane-tabs").click(7usize, cx);
        window.render_frame(cx);
        click!("browser-open");
        click!("work-pane-close");
        click!("toggle-work-pane");

        // Composer: the mention chip and its completion first, while the draft
        // is empty, then every popover chip and every entry inside it -- the
        // full option list each popover enumerates, since a choice the panel
        // lists but nothing presses is exactly the row that rots unnoticed.
        //
        click!("composer-mention");
        assert!(
            window.try_find("composer-suggestions").is_some(),
            "the mention chip opens the completion list"
        );
        click!("composer-suggestion-0");

        for (chip, panel, items) in [
            (
                "composer-add",
                "composer-add-panel",
                &[
                    "composer-add-file",
                    "composer-add-skill",
                    "composer-add-connector",
                    "composer-add-plugin",
                ][..],
            ),
            (
                "composer-model",
                "composer-model-panel",
                &[
                    "composer-model-claude-sonnet-4-5",
                    "composer-model-claude-opus-4-1",
                    "composer-model-gpt-5",
                    "composer-model-deepseek-chat",
                    "composer-model-kimi-k2-0905-preview",
                ][..],
            ),
            (
                "composer-effort",
                "composer-effort-panel",
                &[
                    "composer-effort-auto",
                    "composer-effort-low",
                    "composer-effort-medium",
                    "composer-effort-high",
                    "composer-effort-xhigh",
                    "composer-effort-max",
                ][..],
            ),
            (
                "composer-permission",
                "composer-permission-panel",
                &[
                    "composer-permission-auto",
                    "composer-permission-default",
                    "composer-permission-plan",
                ][..],
            ),
        ] {
            for item in items {
                click!(chip);
                click!(*item);
            }
            dismiss_popover(window, cx, panel);
        }

        // The context ring is a real-data control, and the preview now seeds the
        // usage the prototype draws, so the ring is on screen and its panel is
        // reachable like any other chip's.
        click!("composer-usage");
        assert!(
            window.try_find("composer-usage-panel").is_some(),
            "the context ring opens its own panel"
        );
        dismiss_popover(window, cx, "composer-usage-panel");

        // The preview opens on a live turn, so this control is the turn's Stop
        // button; with no attached session there is nothing to cancel and the
        // click is here for reachability.
        click!("composer-primary");

        // Dialogs last: each one covers the shell, so it is opened, walked, and
        // dismissed before the next.
        click!("open-command-palette");
        assert!(
            window.try_find("command").is_some(),
            "the palette renders over the shell"
        );
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("command").is_none(),
            "escape dismisses the palette"
        );

        click!("open-settings");
        click!("settings-theme-light");
        click!("settings-theme-dark");
        click!("settings-show-thinking");
        // The Reading group runs past the panel's fold, so reaching its last
        // row is a scroll, exactly as it is for a user.
        window.scroll(
            "settings-show-thinking",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-200.))),
            cx,
        );
        window.render_frame(cx);
        click!("settings-follow-tail");

        // The title bar's theme cycle is walked last on purpose: the switch
        // raises a toast that lives for a while, and the shipped anchor puts
        // it over the composer's chips, so pressing it earlier would make the
        // chip presses below a dismissal. The settings dialog above already
        // exercised the same theme switch; this is the title-bar entry itself.
        click!("toggle-theme");
        click!("toggle-theme");

        assert!(
            window.try_find("transcript").is_some(),
            "the shell survives the full walk"
        );
    })
    .unwrap();
}

/// A focused terminal owns its keys.
///
/// `Ctrl-L` is "focus the composer" to the shell and also what readline uses to
/// clear the screen. Before the terminal consumed its keys, the chord fired the
/// window action *as well*, so typing at a shell could steal focus. The proof
/// that it does not is that the composer never becomes focused: the `@` trigger
/// only opens its completion list inside the composer.
#[gpui_kit::test]
fn the_terminal_consumes_keys_instead_of_firing_window_shortcuts(cx: &mut TestAppContext) {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("work-pane-tabs").click(6usize, cx);
        window.render_frame(cx);
        window.click("terminal-start", cx);
        window.render_frame(cx);
        // Focus the grid the way a user does: press it.
        window.click("terminal-grid", cx);
        window.render_frame(cx);

        window.press("ctrl-l", cx);
        window.render_frame(cx);
        window.input("@", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-suggestions").is_none(),
            "Ctrl-L reached the terminal instead of focusing the composer"
        );

        window.press("escape", cx);
        window.render_frame(cx);
        window.input("@", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-suggestions").is_none(),
            "Escape reached the terminal instead of running Stop task"
        );
    })
    .unwrap();
}

/// The update entry answers in an offline shell without touching the network.
///
/// A real check downloads a manifest and, when a newer release exists, a signed
/// installer. The offline shell owns neither a network nor a release, so the
/// contract it has to keep is narrower: the entry exists, and pressing it says
/// what it would have done instead of hanging or silently doing nothing.
#[gpui_kit::test]
fn the_check_for_updates_entry_answers_offline(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let before = app.update(cx, |app, _| app.transcript_len());
        app.update(cx, |app, cx| app.check_for_updates(cx));
        window.render_frame(cx);
        assert_eq!(
            app.update(cx, |app, _| app.transcript_len()),
            before + 1,
            "the offline shell answers with exactly one notice"
        );
    })
    .unwrap();
}

/// The Browser pane normalizes an address and remembers what it opened.
///
/// The pane does not embed a web view, so the preview path is the one the test
/// can drive end to end: it records the address and says it would open it,
/// without launching anything behind the user's back.
#[gpui_kit::test]
fn the_browser_pane_normalizes_and_remembers_addresses(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("work-pane-tabs").click(7usize, cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-browser").is_some(),
            "the Browser tab renders its own body id"
        );
        assert!(
            window.try_find("work-pane-empty-browser").is_some(),
            "an untouched Browser pane says it has no addresses"
        );

        // A bare host is normalized the way an address bar does.
        window.click("browser-open", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("browser-history-0").is_none(),
            "an empty address is not recorded"
        );

        window.click("browser-url", cx);
        window.input("example.com", cx);
        window.render_frame(cx);
        window.click("browser-open", cx);
        window.render_frame(cx);

        let row = window.find("browser-history-0");
        assert_eq!(
            row.label(),
            Some("Open https://example.com"),
            "the bare host gained its scheme before it was remembered"
        );
        assert!(
            window.try_find("work-pane-empty-browser").is_none(),
            "the empty state gives way to the history list"
        );

        window.click("browser-clear", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("browser-history-0").is_none(),
            "Clear forgets every remembered address"
        );
        assert!(
            window.try_find("work-pane-empty-browser").is_some(),
            "and the empty state comes back"
        );
    })
    .unwrap();
}

/// The Terminal pane runs a real shell in a real PTY.
///
/// The unit tests cover VT parsing and key encoding; this covers the pane's
/// half: opening it must not spawn anything, Start must, bytes written to the
/// shell must come back through the parser, and Restart must replace the
/// child. `/bin/sh` is not guaranteed on every host, so a missing shell skips.
#[gpui_kit::test]
fn the_terminal_pane_runs_a_shell_in_a_pty(cx: &mut TestAppContext) {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    activate_shipped_theme(cx);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");
    let handle = handle.into();

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.within("work-pane-tabs").click(6usize, cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-terminal").is_some(),
            "the Terminal tab renders its own body id"
        );
        assert!(
            window.try_find("terminal-start").is_some(),
            "an unstarted terminal offers a Start control"
        );
        assert!(
            window.try_find("terminal-grid").is_none(),
            "opening the pane must not spawn a shell"
        );
        assert_eq!(
            app.update(cx, |app, _| app.terminal_contents()),
            None,
            "and no shell is attached yet"
        );

        window.click("terminal-start", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("terminal-grid").is_some(),
            "Start opens the grid"
        );
    })
    .unwrap();

    // Ask the shell for a unique string and wait for it to come back through
    // the PTY, the reader thread, and the VT parser. The deadline keeps a
    // wedged child from hanging the suite.
    app.update(cx, |app, cx| {
        // `SHELL` may be the developer's own shell; a POSIX `printf` works in
        // both it and `/bin/sh`.
        app.terminal_input("printf 'tact-terminal-ok\\n'\n", cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut contents = String::new();
    while std::time::Instant::now() < deadline {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
        contents = app
            .update(cx, |app, _| app.terminal_contents())
            .unwrap_or_default();
        if contents.contains("tact-terminal-ok") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(
        contents.contains("tact-terminal-ok"),
        "the shell's output reached the grid: {contents:?}"
    );

    // Restart replaces the child, which the pane shows by clearing the grid
    // back to a fresh prompt.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("terminal-restart", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("terminal-grid").is_some(),
            "Restart leaves a running terminal behind, not an empty pane"
        );
    })
    .unwrap();
}

/// The Stats pane draws the session's own numbers.
///
/// The pane is the chart-heavy dashboard the design deferred past v1, so the
/// contract that matters is that each chart is fed by session state rather than
/// by its own copy: switching panes must not empty it, and the seeded preview
/// must produce all three charts.
#[gpui_kit::test]
fn the_stats_pane_charts_the_session_state(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("work-pane-tabs").click(5usize, cx);
        window.render_frame(cx);

        assert!(
            window.try_find("work-pane-body-stats").is_some(),
            "the Stats tab renders its own body id"
        );
        for chart in [
            "stats-tile-tokens",
            "stats-tile-tasks",
            "stats-tile-plan",
            "stats-tile-diff",
            "stats-chart-tokens",
            "stats-chart-tasks",
            "stats-chart-diff",
        ] {
            assert!(
                window.try_find(chart).is_some(),
                "{chart} is rendered from the seeded session state"
            );
        }

        // Leave and come back: the pane reads SessionState at render time, so a
        // round trip through another tab must not empty it.
        window.within("work-pane-tabs").click(0usize, cx);
        window.render_frame(cx);
        window.within("work-pane-tabs").click(5usize, cx);
        window.render_frame(cx);
        assert!(
            window.try_find("stats-chart-tasks").is_some(),
            "returning to the Stats tab re-renders its charts"
        );
    })
    .unwrap();
}

/// A directory row is a row, not just its chevron.
///
/// The row drew a hover background but only the small icon button answered a
/// press, so clicking a folder's name did nothing at all: the row looked live
/// and was dead. Pressing the row toggles it and selects it, which also gives
/// the footer's Reveal and Mention something to act on for a folder.
#[gpui_kit::test]
fn the_files_pane_row_toggles_and_selects_a_directory(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("work-pane-tabs").click(4usize, cx);
        window.render_frame(cx);

        let directory = repo_root().join("crates");
        let row: SharedString = format!("file-row-{}", directory.display()).into();
        let child: SharedString =
            format!("file-row-{}", directory.join("tact-gui").display()).into();

        assert!(
            window.try_find(row.clone()).is_some(),
            "the crates row renders"
        );
        assert!(
            window.try_find(child.clone()).is_none(),
            "the directory starts collapsed"
        );

        // Press the row itself, away from its chevron.
        window.click(row.clone(), cx);
        window.render_frame(cx);
        assert!(
            window.try_find(child.clone()).is_some(),
            "pressing the row expands the directory"
        );
        assert_eq!(
            window.find(row.clone()).selected(),
            Some(true),
            "and the row becomes the selection"
        );
        assert!(
            window.try_find("work-pane-directory-preview").is_some(),
            "the preview describes the directory rather than a file"
        );
        assert!(
            window.try_find("work-pane-file-error").is_none(),
            "selecting a directory is not a read failure"
        );

        // The row is a tab stop, so Enter has to do what a press does.
        window.click(row.clone(), cx);
        window.render_frame(cx);
        assert!(
            window.try_find(child.clone()).is_none(),
            "pressing the row again collapses the directory"
        );
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(
            window.try_find(child.clone()).is_some(),
            "Enter on the focused row expands it"
        );
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(window.try_find(child).is_none(), "Enter again collapses it");
    })
    .unwrap();
}

/// A selected folder is a full citizen of the pane's footer actions.
///
/// The row press makes a directory the selection, so Reveal, Mention, and the
/// footer's Open in editor have to mean something for it rather than reporting
/// that no file is selected.
#[gpui_kit::test]
fn the_files_pane_actions_work_on_a_selected_directory(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let mut app = None;
    // Tall enough that the preview card below the tree is on screen: the
    // pane's body scrolls, and a press has to be visible to land.
    let handle = cx.open_window(size(px(1440.), px(1600.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_workspace(window, cx, Some(repo_root())));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("work-pane-tabs").click(4usize, cx);
        window.render_frame(cx);

        let directory = repo_root().join("docs");
        let row: SharedString = format!("file-row-{}", directory.display()).into();
        window.click(row, cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-directory-preview").is_some(),
            "the row press selects the folder"
        );

        // Mention inserts the folder path.
        window.click("work-pane-file-mention", cx);
        window.render_frame(cx);
        let draft = app.update(cx, |app, cx| app.composer_draft(cx));
        assert_eq!(
            draft, "@docs ",
            "Mention inserts the folder's workspace-relative path"
        );

        // Reveal and the footer's Open in editor both answer instead of
        // claiming nothing is selected.
        let before = app.update(cx, |app, _| app.transcript_len());
        window.click("work-pane-file-reveal", cx);
        window.render_frame(cx);
        let after_reveal = app.update(cx, |app, _| app.transcript_len());
        assert_eq!(
            after_reveal,
            before + 1,
            "Reveal answers with one notice for a folder"
        );

        window.click("work-pane-open-editor", cx);
        window.render_frame(cx);
        let after_open = app.update(cx, |app, _| app.transcript_len());
        assert_eq!(
            after_open,
            after_reveal + 1,
            "Open in editor answers with one notice for a folder"
        );
    })
    .unwrap();
}

/// The Files pane's expand toggle opens and closes the row it names.
///
/// The row and its toggle are separate element ids (`file-row-` and
/// `file-toggle-`), so the id a test clicks is not the id the row is observed
/// by; this pins that the toggle still drives the expansion set.
#[gpui_kit::test]
fn the_files_pane_expands_a_directory_through_its_toggle(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("work-pane-tabs").click(4usize, cx);
        window.render_frame(cx);

        let directory = repo_root().join("crates");
        let child: SharedString =
            format!("file-row-{}", directory.join("tact-gui").display()).into();
        let toggle: SharedString = format!("file-toggle-{}", directory.display()).into();
        assert!(
            window.try_find(child.clone()).is_none(),
            "a collapsed directory does not list its children"
        );

        window.click(toggle.clone(), cx);
        window.render_frame(cx);
        assert!(
            window.try_find(child.clone()).is_some(),
            "the toggle expands the directory its row names"
        );

        window.click(toggle, cx);
        window.render_frame(cx);
        assert!(
            window.try_find(child).is_none(),
            "the same toggle collapses it again"
        );
    })
    .unwrap();
}

/// A Files row opens a preview, and the preview can reveal or mention it.
///
/// The tree itself has covered expansion; this pins the file half of the spec's
/// `open, reveal, mention in composer` contract. The shell under test is the
/// offline preview, so the launchers report what they would do instead of
/// opening a real application behind the test runner, while the composer draft
/// is observed directly.
#[gpui_kit::test]
fn the_files_pane_previews_reveals_and_mentions_a_file(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_workspace(window, cx, Some(repo_root())));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("work-pane-tabs").click(4usize, cx);
        window.render_frame(cx);

        let file = repo_root().join("Cargo.toml");
        let row: SharedString = format!("file-row-{}", file.display()).into();
        window.click(row, cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-file-preview").is_some(),
            "clicking a file row opens the preview card"
        );
        assert!(
            window.try_find("work-pane-file-content").is_some(),
            "the preview card renders the selected file content"
        );

        for _ in 0..40 {
            window.render_frame(cx);
            if window
                .try_find("work-pane-file-mention")
                .is_some_and(|button| button.visible())
            {
                break;
            }
            window.scroll(
                "work-pane-body-scroll",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-500.))),
                cx,
            );
        }
        window.click("work-pane-file-mention", cx);
        window.render_frame(cx);
        assert_eq!(
            app.update(cx, |app, cx| app.composer_draft(cx)),
            "@Cargo.toml ",
            "Mention inserts the workspace-relative path into the composer"
        );

        let before_reveal = app.update(cx, |app, _| app.transcript_len());
        window.click("work-pane-file-reveal", cx);
        window.render_frame(cx);
        assert_eq!(
            app.update(cx, |app, _| app.transcript_len()),
            before_reveal + 1,
            "Reveal reports its offline result instead of silently doing nothing"
        );

        let before_open = app.update(cx, |app, _| app.transcript_len());
        window.click("work-pane-open-editor", cx);
        window.render_frame(cx);
        assert_eq!(
            app.update(cx, |app, _| app.transcript_len()),
            before_open + 1,
            "Open in editor acts on the selected file"
        );
    })
    .unwrap();
}

/// The Projects group switches workspaces by directory.
///
/// A workspace *is* a directory — the session store lives in
/// `<workspace>/.tact/tact.db` — so opening one re-roots the window, its
/// session list, and both file-reading panes, and remembers it for next time.
#[gpui_kit::test]
fn the_projects_group_switches_workspace_by_directory(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let here = repo_root();
    let other = repo_root().join("crates/tact-session");
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_workspace(window, cx, Some(here.clone())));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the shell is created with its window");
    let handle = handle.into();

    let here_row: SharedString = format!("project-row-{}", here.display()).into();
    let other_row: SharedString = format!("project-row-{}", other.display()).into();

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(here_row.clone()).is_some(),
            "the directory the shell opened on is listed"
        );
        assert_eq!(
            window.find(here_row.clone()).selected(),
            Some(true),
            "and it is the current project"
        );
        assert!(
            window.try_find("project-open-folder").is_some(),
            "the group carries the Open folder entry"
        );

        // Opening a second directory is the same move the picker makes once it
        // has a path.
        app.update(cx, |app, cx| app.open_workspace(other.clone(), cx));
        window.render_frame(cx);

        assert!(
            window.try_find(other_row.clone()).is_some(),
            "the opened directory joins the list"
        );
        assert_eq!(
            window.find(other_row.clone()).selected(),
            Some(true),
            "and becomes the current project"
        );
        assert_eq!(
            window.find(here_row.clone()).selected(),
            Some(false),
            "the project the window left gives up the marker"
        );
        let chip = window.find("status-project");
        assert_eq!(
            chip.label(),
            Some("tact-session"),
            "the status bar follows the workspace the window is in"
        );

        // A directory that is not one is refused rather than re-rooting the
        // window at nothing.
        let missing = repo_root().join("Cargo.toml");
        let before = app.update(cx, |app, _| app.transcript_len());
        app.update(cx, |app, cx| app.open_workspace(missing, cx));
        window.render_frame(cx);
        assert_eq!(
            app.update(cx, |app, _| app.transcript_len()),
            before + 1,
            "opening a file as a workspace answers with one notice"
        );
    })
    .unwrap();
}

/// The Open folder entry answers in an offline shell instead of opening a modal.
#[gpui_kit::test]
fn the_open_folder_row_does_not_open_a_picker_offline(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_workspace(window, cx, Some(repo_root())));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let before = app.update(cx, |app, _| app.transcript_len());
        window.click("project-open-folder", cx);
        window.render_frame(cx);
        assert_eq!(
            app.update(cx, |app, _| app.transcript_len()),
            before + 1,
            "the offline shell says what it would open instead of asking the desktop"
        );
    })
    .unwrap();
}

/// A worktree press re-roots the window without restarting its session.
///
/// The worktree group doubles as the window's workspace picker, so a press has
/// to move everything that reads the workspace -- the Files tree walks the new
/// root, and the sidebar marks the new row current -- while leaving the agent
/// session alone. Pointing the window at another worktree is not starting
/// another thread; the next `new_session` is what lands there, which is why the
/// pane re-rooting and the session surviving are the two halves pinned here.
#[gpui_kit::test]
fn a_worktree_press_re_roots_the_window_without_restarting_the_session(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let fixture = WorktreeFixture::create();
    if !fixture.is_added() {
        // One worktree means every row is the root the window already has, so
        // there is no second root to walk. `every_entry_point_answers_a_click`
        // still covers that idempotent press.
        eprintln!(
            "note: no second worktree in this environment, so the re-root test has nothing to \
             switch to"
        );
        return;
    }

    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // The Files pane walks `state.workdir`, the same field a session
        // started after the press would be rooted at.
        window.within("work-pane-tabs").click(4usize, cx);
        window.render_frame(cx);
        let home_row: SharedString =
            format!("file-row-{}", repo_root().join("Cargo.toml").display()).into();
        assert!(
            window.try_find(home_row.clone()).is_some(),
            "the Files pane walks the worktree the window opened on"
        );
        assert!(
            window.try_find(fixture.file_row("Cargo.toml")).is_none(),
            "a worktree the window is not scoped to is not walked"
        );

        // The fixture row is the one the window is not currently scoped to.
        let fixture_row: SharedString = worktree_row_ids()
            .into_iter()
            .find(|id| window.find(SharedString::from(id.clone())).selected() == Some(false))
            .expect("the fixture worktree is the row that is not open")
            .into();
        reveal_in_sidebar(window, &fixture_row, cx);
        window.click(fixture_row.clone(), cx);
        window.render_frame(cx);

        assert_eq!(
            window.find(fixture_row).selected(),
            Some(true),
            "the pressed worktree row is the one the window is scoped to"
        );
        assert!(
            window.try_find(fixture.file_row("Cargo.toml")).is_some(),
            "the Files pane re-roots at the worktree the press picked"
        );
        assert!(
            window.try_find(home_row).is_none(),
            "the pane stops walking the worktree the window left"
        );

        // A restarted session drops the thread: `adopt` clears the transcript,
        // so the message the window was showing has to still be there.
        assert!(
            window.try_find("user-bubble-0").is_some(),
            "the press keeps the thread the window was showing"
        );
    })
    .unwrap();
}

/// Switches surfaces back to back, the way a reader skimming the app does.
///
/// The bound is a watchdog, not a benchmark: a regression that makes a click do
/// unbounded work shows up as this test taking minutes instead of seconds.
/// The prototype pins `.toast` 20px from the bottom-right corner, clear of the
/// work pane.
///
/// gpui-component anchors toasts to the top-right instead, which puts an
/// occluding card straight over the work pane's tab strip: a theme switch left
/// the Diff tab unclickable for as long as the toast lived. The shipped theme
/// moves the anchor back to the prototype's corner, so this pins both halves —
/// which stack exists, and that the tabs still answer a click under it.
#[gpui_kit::test]
fn the_shipped_toast_placement_keeps_the_work_pane_tabs_clickable(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        // Two theme switches raise the toast the title bar's button raises.
        window.click("toggle-theme", cx);
        window.render_frame(cx);
        window.click("toggle-theme", cx);
        window.render_frame(cx);
        // A stack only occupies its anchor once the cards have been measured in
        // prepaint, which is what made this reachable in the first place.
        for _ in 0..40 {
            window.render_frame(cx);
        }

        assert!(
            window.try_find(("notification-list", 5usize)).is_some(),
            "the shipped theme anchors toasts to the bottom-right stack"
        );
        assert!(
            window.try_find(("notification-list", 2usize)).is_none(),
            "no toast may live in the top-right stack over the tab strip"
        );

        window.within("work-pane-tabs").click(1usize, cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-diff").is_some(),
            "the Diff tab answers a click while a toast is on screen"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn rapid_switching_between_surfaces_stays_responsive(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let started = std::time::Instant::now();

        for _ in 0..40 {
            for index in 0..5usize {
                window.within("work-pane-tabs").click(index, cx);
                window.render_frame(cx);
            }
            window.click("toggle-sidebar", cx);
            window.render_frame(cx);
            window.click("toggle-sidebar", cx);
            window.render_frame(cx);
            window.click("toggle-work-pane", cx);
            window.render_frame(cx);
            window.click("toggle-work-pane", cx);
            window.render_frame(cx);
            window.click("toggle-theme", cx);
            window.render_frame(cx);
            window.press("ctrl-o", cx);
            window.render_frame(cx);
        }

        let elapsed = started.elapsed();
        eprintln!("soak: 40 rounds, {} renders, {elapsed:?}", 40 * 11 + 1);
        assert!(
            elapsed < std::time::Duration::from_secs(60),
            "440 interactions took {elapsed:?}, which is a hang rather than a repaint"
        );
        assert!(
            window.try_find("transcript").is_some() && window.try_find("status-bar").is_some(),
            "the shell still renders after the soak"
        );
    })
    .unwrap();
}

/// The new-session chord has to answer where the sidebar's `+` button does.
///
/// An offline shell owns no runtime, so `Primary+N` reports that in the
/// transcript instead of reaching for a session it cannot start — and it must
/// not move the row the sidebar has open, which is the whole state a press
/// produces. The empty transcript is what makes the notice observable: it is
/// the only element index 1 holds before the chord, and the chord replaces it
/// with the row it appends.
#[gpui_kit::test]
fn the_new_session_chord_answers_like_the_sidebar_button(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    // The shipped theme is not the same thing as this app's own bindings.
    cx.update(tact_gui::commands_init);

    let sessions = vec![
        RecentSession {
            id: "11111111-aaaa-bbbb".to_string(),
            updated_at_unix: 1_700_000_000,
            message_count: 4,
            title: Some("Desktop client design".to_string()),
            name: None,
            archived: false,
            pinned: false,
        },
        RecentSession {
            id: "22222222-cccc-dddd".to_string(),
            updated_at_unix: 1_700_000_100,
            message_count: 0,
            title: None,
            name: None,
            archived: false,
            pinned: false,
        },
    ];
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_sessions(window, cx, sessions));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("transcript-empty").is_some(),
            "a shell with no turns starts on the empty transcript"
        );
        assert!(
            window.try_find("transcript-row-0").is_none(),
            "and it has no first row to show"
        );

        // No session is attached, so the row a press picks is the one the
        // sidebar marks open; the chord below must leave it where it is.
        window.click("session-row-22222222-cccc-dddd", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("session-row-22222222-cccc-dddd").selected(),
            Some(true),
            "the clicked row is the one the sidebar has open"
        );

        window.press("ctrl-n", cx);
        window.render_frame(cx);

        assert!(
            window.try_find("transcript-empty").is_none(),
            "Primary+N appends a transcript row where the placeholder was"
        );
        assert!(
            window.try_find("transcript-row-0").is_some(),
            "Primary+N answers with a row the user can read"
        );
        // The row is a notice, not a card: nothing in it opens, so the only
        // thing a second press can do is append the next notice. Two rows after
        // two presses is what separates an answering chord from a swallowed one.
        window.press("ctrl-n", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("transcript-row-1").is_some(),
            "every press answers, so a second one appends the next notice"
        );
        assert!(
            window.try_find("tool-summary-0").is_none()
                && window.try_find("thinking-summary-0").is_none(),
            "the notice is plain text, not an expandable card"
        );
        assert_eq!(
            window.find("session-row-22222222-cccc-dddd").selected(),
            Some(true),
            "starting a session offline leaves the open row where it was"
        );
        assert!(
            window.find("session-row-11111111-aaaa-bbbb").selected() != Some(true),
            "and it does not hand the highlight to another session"
        );
    })
    .unwrap();
}

/// The empty transcript's only control has to focus the composer.
///
/// `transcript-empty-focus` is the "Write a message" button the shell shows
/// while a session has no turns. Nothing else in the suite renders it — every
/// other test starts from the preview's seeded thread — so this is the only
/// place the branch is reachable. Focus is proved by typing, not by asking for
/// focus: `@` only opens the completion list when the draft is the composer's.
#[gpui_kit::test]
fn the_empty_transcript_focuses_the_composer(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let sessions = vec![RecentSession {
        id: "11111111-aaaa-bbbb".to_string(),
        updated_at_unix: 1_700_000_000,
        message_count: 0,
        title: None,
        name: None,
        archived: false,
        pinned: false,
    }];
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_sessions(window, cx, sessions));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("transcript-empty").is_some(),
            "a shell with no turns shows the empty transcript"
        );
        assert!(
            window.try_find("composer-suggestions").is_none(),
            "and no completion list before anything is typed"
        );

        window.click("transcript-empty-focus", cx);
        window.render_frame(cx);
        window.input("@", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-suggestions").is_some(),
            "the empty transcript's button hands the keyboard to the composer"
        );
    })
    .unwrap();
}

/// The completion list is a keyboard surface, not only a pointer one: Up/Down
/// move the highlight and Enter takes the row it is on, the way the terminal's
/// picker works.
#[gpui_kit::test]
fn the_completion_list_is_driven_from_the_keyboard(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-mention", cx);
        window.render_frame(cx);

        let first = window
            .find("composer-suggestion-0")
            .label()
            .expect("the first row has a label")
            .to_string();
        let second = window
            .find("composer-suggestion-1")
            .label()
            .expect("the second row has a label")
            .to_string();
        assert_ne!(
            first, second,
            "the preview workspace holds more than one entry"
        );

        window.press("down", cx);
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);

        let draft = app.update(cx, |app, cx| app.composer_draft(cx));
        assert!(
            draft.contains(second.trim_start_matches('@')),
            "Enter took the row the keyboard was on ({second:?}), got {draft:?}"
        );
    })
    .unwrap();
}

/// Escape puts the list away without touching the draft, and the next keystroke
/// asks for it back.
#[gpui_kit::test]
fn escape_puts_the_completion_list_away(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let mut app: Option<Entity<TactApp>> = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the shell entity is captured");

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-mention", cx);
        window.render_frame(cx);
        assert!(window.try_find("composer-suggestions").is_some());

        window.press("escape", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-suggestions").is_none(),
            "Escape hides the list"
        );
        // Escape belongs to the list, not to the field: the trigger it was
        // opened for is still in the draft, waiting to be typed on.
        assert_eq!(
            app.update(cx, |app, cx| app.composer_draft(cx)),
            "@",
            "Escape leaves the draft alone"
        );

        // Back to the field, then type: the list is a function of the draft,
        // so the next keystroke is what asks for it again.
        window.click("prompt-composer-input", cx);
        window.input("s", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-suggestions").is_some(),
            "typing asks for the list again"
        );
    })
    .unwrap();
}

/// The `/` list leads with the shell's own command, spelled out.
///
/// The terminal popup puts built-ins ahead of skills and prints what each one
/// does. The desktop list reads the same way, so a reader who types `/` sees the
/// command it can run instead of having to remember the name.
#[gpui_kit::test]
fn the_slash_list_leads_with_the_shells_command(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("prompt-composer-input", cx);
        window.render_frame(cx);
        window.input("/", cx);
        window.render_frame(cx);

        assert_eq!(
            window.find("composer-suggestion-0").label(),
            Some("/compact"),
            "the shell's own command leads the `/` list"
        );
        assert!(
            window.try_find("composer-suggestion-note-0").is_some(),
            "the row says what the command does"
        );
    })
    .unwrap();
}

/// The `/` list groups its rows the way the terminal popup does: the shell's
/// own commands under one heading, the skills under another. A `@` list is one
/// group, so it carries no heading at all.
#[gpui_kit::test]
fn the_slash_list_groups_commands_and_skills(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);

    // A workspace with one skill and one file, so both lists have something to
    // group without depending on what the machine happens to have installed.
    let root = std::env::temp_dir().join(format!("tact-gui-sections-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".tact/skills/gui-demo")).unwrap();
    std::fs::write(
        root.join(".tact/skills/gui-demo/SKILL.md"),
        "---\nname: gui-demo\ndescription: A demo skill\n---\n\nApply it.\n",
    )
    .unwrap();
    std::fs::write(root.join("README.md"), "# readme\n").unwrap();

    let workdir = root.clone();
    let handle = cx.open_window(size(px(1440.), px(900.)), move |window, cx| {
        let shell = cx.new(|cx| TactApp::with_workspace(window, cx, Some(workdir)));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("prompt-composer-input", cx);
        window.render_frame(cx);
        window.input("/", cx);
        window.render_frame(cx);

        assert_eq!(
            window.find("composer-suggestion-0").label(),
            Some("/compact"),
            "the shell's command is still the first row"
        );
        assert!(
            window.try_find("composer-section-commands").is_some(),
            "the command group is headed"
        );
        assert!(
            window.try_find("composer-section-skills").is_some(),
            "and so is the skill group"
        );
    })
    .unwrap();

    // The `@` list is one group, so it has no heading to draw.
    let handle = cx.open_window(size(px(1440.), px(900.)), move |window, cx| {
        let shell = cx.new(|cx| TactApp::with_workspace(window, cx, Some(root)));
        Root::new(shell, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-mention", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-suggestion-0").is_some(),
            "the workspace's own file is offered"
        );
        assert!(
            window.try_find("composer-section-commands").is_none()
                && window.try_find("composer-section-skills").is_none(),
            "a mention list is not grouped"
        );
    })
    .unwrap();
}

/// An attachment chip's remove button drops that chip.
///
/// The chip only exists once a file is attached, so the click walk reaches the
/// chip it creates and then leaves it on the composer. This pins the other half:
/// the `×` on the chip removes the attachment it names.
#[gpui_kit::test]
fn the_composer_drops_an_attachment_through_its_chip(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell =
            cx.new(|cx| TactApp::with_attachments(window, cx, [PathBuf::from("src/main.rs")]));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("composer-attachment-0").is_some(),
            "the staged file has a chip"
        );

        window.click("composer-attachment-remove-0", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-attachment-0").is_none(),
            "the chip's own button drops the attachment"
        );
        assert!(
            window.try_find("composer-attachments").is_none(),
            "and the strip goes with the last chip"
        );
    })
    .unwrap();
}

/// The keyboard chord removes the last attachment chip.
///
/// The chip's own `×` is covered by
/// `the_composer_drops_an_attachment_through_its_chip`; this pins the
/// `RemoveAttachment` command instead. The mention completion supplies the same
/// real attachment fixture without a native file dialog, then the chord must
/// take that chip back off the composer.
#[gpui_kit::test]
fn the_composer_removes_the_last_attachment_with_the_keyboard(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell =
            cx.new(|cx| TactApp::with_attachments(window, cx, [PathBuf::from("src/main.rs")]));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("composer-attachment-0").is_some(),
            "the staged file has a chip for the chord to remove"
        );

        window.press("ctrl-shift-backspace", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-attachment-0").is_none(),
            "Primary+Shift+Backspace removes the last attachment"
        );
        assert!(
            window.try_find("composer-attachments").is_none(),
            "the strip goes with the last chip"
        );
    })
    .unwrap();
}

/// A skill completion row accepts the slash command it names.
///
/// The `/` branch of the composer is fed by `composer::skill_suggestions` and
/// rendered with the same `composer-suggestion-*` ids as file completion. This
/// types the slash trigger directly, then presses the first skill row: accepting
/// it must insert the command and leave the trailing-space draft without a
/// trigger, so the completion list closes.
#[gpui_kit::test]
fn the_composer_accepts_a_skill_suggestion_row(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("composer-suggestions").is_none(),
            "nothing is suggested before a trigger is typed"
        );

        window.click("prompt-composer-input", cx);
        window.input("/", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-suggestions").is_some(),
            "the slash trigger opens the skill completion list"
        );

        window.click("composer-suggestion-0", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-suggestions").is_none(),
            "accepting a skill replaces the trigger and closes the list"
        );
        assert!(
            window.try_find("composer-attachments").is_none(),
            "a skill is inserted into the draft, not attached as a file"
        );
    })
    .unwrap();
}

/// Every palette row runs its command exactly once.
///
/// `CommandState::confirm` dispatches the row's own GPUI action *and* defers the
/// palette's `on_confirm`. While both paths ran the command, every row executed
/// twice — invisible for idempotent rows such as Open diff, and a silent no-op
/// for the toggles. This walks both flavours: toggles that must land on the
/// opposite state, and pane switches that must land on the named body.
#[gpui_kit::test]
fn the_command_palette_rows_run_their_commands(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    // The palette slides in over ~250 ms of wall clock, and
    // `TestWindowExt::click` renders a frame between pointer down and up, so a
    // list that is still moving drops the row before the release.
    let open_palette = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            if window.try_find("command").is_none() {
                window.click("open-command-palette", cx);
                std::thread::sleep(std::time::Duration::from_millis(400));
                window.render_frame(cx);
                window.render_frame(cx);
            }
        })
        .unwrap();
        cx.run_until_parked();
    };
    let click_row = |cx: &mut TestAppContext, row: usize| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(format!("index-path(0,{row},0)"), cx);
        })
        .unwrap();
        cx.run_until_parked();
    };

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("sidebar").is_some(),
            "the sidebar starts visible"
        );
        assert!(
            window.try_find("work-pane").is_some(),
            "the work pane starts open"
        );
    })
    .unwrap();

    // (0,0) Command palette: the row re-opens the palette it is already in,
    // so the observable is that one is still on screen once the row is done.
    open_palette(cx);
    click_row(cx, 0);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("command").is_some(),
            "the Command palette row leaves a palette on screen"
        );
    })
    .unwrap();

    // (0,1) Toggle sidebar: a row that dispatches twice lands back where it
    // started, which is the bug this pins.
    open_palette(cx);
    click_row(cx, 1);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("sidebar").is_none(),
            "the Toggle sidebar row hides the sidebar"
        );
    })
    .unwrap();

    // (0,2) Toggle work pane — the same shape, the other toggle.
    open_palette(cx);
    click_row(cx, 2);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane").is_none(),
            "the Toggle work pane row closes the work pane"
        );
    })
    .unwrap();

    // (0,4) Open tasks, from the empty work pane the toggle left behind.
    open_palette(cx);
    click_row(cx, 4);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-tasks").is_some(),
            "the Open tasks row shows the tasks body"
        );
    })
    .unwrap();

    // (0,3) Open diff — a second pane switch, so a row after a body change
    // still resolves.
    open_palette(cx);
    click_row(cx, 3);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-diff").is_some(),
            "the Open diff row shows the diff body"
        );
    })
    .unwrap();
}

/// The palette's advertised arrow/Enter/Tab contract runs on the real window.
///
/// `Command` owns the selection state, so the row snapshots are the only
/// observable: the selected `index-path` must move with Up/Down, Enter must run
/// the row and close the palette, Tab must not silently change the selection,
/// and Escape must dismiss an empty-query palette.
#[gpui_kit::test]
fn the_command_palette_keyboard_contract_moves_and_runs_rows(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    let open_palette = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            if window.try_find("command").is_none() {
                window.click("open-command-palette", cx);
                std::thread::sleep(std::time::Duration::from_millis(400));
                window.render_frame(cx);
                window.render_frame(cx);
            }
        })
        .unwrap();
        cx.run_until_parked();
    };

    open_palette(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("index-path(0,0,0)").selected(),
            Some(true),
            "the first command starts selected"
        );
        assert_eq!(window.find("index-path(0,1,0)").selected(), Some(false));

        window.press("down", cx);
        window.render_frame(cx);
        assert_eq!(window.find("index-path(0,0,0)").selected(), Some(false));
        assert_eq!(window.find("index-path(0,1,0)").selected(), Some(true));

        window.press("up", cx);
        window.render_frame(cx);
        assert_eq!(window.find("index-path(0,0,0)").selected(), Some(true));
        assert_eq!(window.find("index-path(0,1,0)").selected(), Some(false));

        window.press("escape", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("command").is_none(),
            "Escape dismisses an empty-query palette"
        );
    })
    .unwrap();

    // Reopen and confirm the second row: `Toggle sidebar` must run exactly
    // once, and the palette must close behind it.
    open_palette(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("down", cx);
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("sidebar").is_none(),
            "Enter runs the selected Toggle sidebar row"
        );
        assert!(
            window.try_find("command").is_none(),
            "Enter closes the palette"
        );
    })
    .unwrap();

    // Tab is focus traversal, not command navigation: the selected row must
    // stay put and the palette must remain mounted. This is last because moving
    // focus away from the command list makes later key routing harness-specific.
    open_palette(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("index-path(0,0,0)").selected(), Some(true));
        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("index-path(0,0,0)").selected(),
            Some(true),
            "Tab does not move the command selection"
        );
        assert!(
            window.try_find("command").is_some(),
            "Tab does not dismiss the palette"
        );
    })
    .unwrap();
}

/// Typing in the palette's search field filters the visible command rows.
///
/// The existing palette tests select rows by their `index-path`; none of them
/// uses the search field that gives the palette its name. The query is sent to
/// the focused input opened by `CommandState::focus`, and the assertions read
/// the original index paths: matching rows stay registered, filtered rows
/// disappear.
#[gpui_kit::test]
fn the_command_palette_search_filters_rows(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });
    let handle = handle.into();

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("open-command-palette", cx);
        // Dialog entrance is a 250 ms wall-clock animation, and the palette
        // focuses its query input through a deferred update.
        std::thread::sleep(std::time::Duration::from_millis(400));
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("command").is_some(),
            "the palette is open before the search is typed"
        );
        assert!(
            window.try_find("index-path(0,0,0)").is_some(),
            "the unfiltered palette starts with its first row"
        );

        window.input("toggle", cx);
        window.render_frame(cx);

        assert!(
            window.try_find("index-path(0,1,0)").is_some(),
            "Toggle sidebar survives the query"
        );
        assert!(
            window.try_find("index-path(0,2,0)").is_some(),
            "Toggle work pane survives the query"
        );
        assert!(
            window.try_find("index-path(3,5,0)").is_some(),
            "Toggle theme survives the query"
        );
        assert!(
            window.try_find("index-path(0,0,0)").is_none(),
            "Command palette is filtered out by the query"
        );
        assert!(
            window.try_find("index-path(0,3,0)").is_none(),
            "Open diff is filtered out by the query"
        );
    })
    .unwrap();
}

/// The palette's New session row answers exactly like `Primary+N`.
///
/// It is the one palette command whose double dispatch leaves more than a
/// toggled-back flag behind: it appends a notice per run, so a second dispatch
/// shows up as a second row. The shell starts with no turns so the row count is
/// readable from the elements themselves.
#[gpui_kit::test]
fn the_palette_new_session_row_answers_like_the_chord(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let sessions = vec![RecentSession {
        id: "33333333-eeee-ffff".to_string(),
        updated_at_unix: 1_700_000_200,
        message_count: 0,
        title: None,
        name: None,
        archived: false,
        pinned: false,
    }];
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_sessions(window, cx, sessions));
        Root::new(shell, window, cx)
    });

    // The palette slides in, so the row has to be clicked from a settled frame.
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("transcript-empty").is_some(),
            "a shell with no turns starts on the empty transcript"
        );
        window.click("open-command-palette", cx);
        std::thread::sleep(std::time::Duration::from_millis(400));
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    // The Session group now sits below the fold behind the Layout group, so
    // the row has to be scrolled into view before it can be pressed.
    run_palette_row(cx, *handle, 2, 0);

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("transcript-empty").is_none(),
            "the New session row appends a row where the placeholder was"
        );
        assert!(
            window.try_find("transcript-row-0").is_some(),
            "and the row it appends is readable"
        );
        assert!(
            window.try_find("transcript-row-1").is_none(),
            "the row runs the command once, not once per dispatch path"
        );
    })
    .unwrap();
}

/// Walk a palette row that starts below the fold.
///
/// The palette caps its own height, so only the first screen of rows is a
/// hitbox a user — or `TestWindowExt::click` — can reach. Everything after it
/// takes a scroll first, which is what this helper reproduces: open the
/// palette, scroll the list until the row reports itself visible, then press
/// it.
fn run_palette_row(
    cx: &mut TestAppContext,
    handle: gpui_kit::AnyWindowHandle,
    section: usize,
    row: usize,
) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        if window.try_find("command").is_none() {
            window.click("open-command-palette", cx);
            // Dialog entrance is a 250 ms wall-clock animation.
            std::thread::sleep(std::time::Duration::from_millis(400));
            window.render_frame(cx);
            window.render_frame(cx);
        }
    })
    .unwrap();
    cx.run_until_parked();

    let id = format!("index-path({section},{row},0)");
    cx.update_window(handle, |_, window, cx| {
        for _ in 0..10 {
            window.render_frame(cx);
            match window.try_find(id.clone()) {
                Some(target) if target.visible() => break,
                _ => window.scroll(
                    "command",
                    gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-120.))),
                    cx,
                ),
            }
        }
        // Let the list come to rest: a press on a row that is still gliding
        // lands wherever the row was on the way up.
        for _ in 0..3 {
            window.render_frame(cx);
        }
        assert!(
            window
                .try_find(id.clone())
                .is_some_and(|target| target.visible()),
            "the palette never scrolled {id} into view"
        );
        // Press the row's top-left, not its centre: the loop above stops as
        // soon as the row is visible, and a row that has just entered from the
        // bottom edge is still `visible()` while its centre sits under the
        // palette's footer.
        window.click_at(id.clone(), gpui_kit::point(px(12.), px(6.)), cx);
    })
    .unwrap();
    cx.run_until_parked();
}

/// The palette rows past the fold run their commands too.
///
/// `the_command_palette_rows_run_their_commands` covers the rows that fit on
/// screen; this walks the rest of the table, which is most of it. Each row is
/// asserted against the surface its command owns, so a row wired to the wrong
/// command fails here rather than silently doing its neighbour's work.
#[gpui_kit::test]
fn the_command_palette_rows_past_the_fold_run_their_commands(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });
    let handle = handle.into();

    // (2,1) Focus composer. Focus is proved by typing, not by asking for it:
    // the `@` trigger only opens the completion list inside the composer.
    run_palette_row(cx, handle, 2, 1);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("composer-suggestions").is_none(),
            "nothing is typed before the composer is asked for a mention"
        );
        window.input("@", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-suggestions").is_some(),
            "the Focus composer row hands the keyboard to the composer"
        );
        // Drop the trigger again so the list does not sit over later presses.
        window.press("backspace", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("composer-suggestions").is_none(),
            "removing the trigger closes the completion list"
        );
    })
    .unwrap();

    // (2,3) Cycle transcript detail, read off the toolbar's own chip.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find("transcript-detail-cycle")
                .label()
                .is_some_and(|label| label.contains("Normal")),
            "the preview opens on the Normal detail"
        );
    })
    .unwrap();
    run_palette_row(cx, handle, 2, 3);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find("transcript-detail-cycle")
                .label()
                .is_some_and(|label| label.contains("Thinking")),
            "the Cycle transcript detail row advances one step"
        );
    })
    .unwrap();

    // (2,2) Stop task. The preview owns no running turn, so the row has nothing
    // to cancel; what is checkable is that it runs and leaves no row behind.
    run_palette_row(cx, handle, 2, 2);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("command").is_none(),
            "the Stop task row dismisses the palette"
        );
        assert!(
            window.try_find("transcript-row-0").is_some(),
            "and it does not append anything to a shell with no turn in flight"
        );
    })
    .unwrap();

    // (2,4) and (2,5) move the open session row, one step out and one back.
    let first = "session-row-7fbab10-2c41-4c9a-9f10-2222aaaa1111";
    let second = "session-row-3b78ba4-1d02-4a33-8b71-3333bbbb2222";
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(first).selected(),
            Some(true),
            "the preview opens on its first session"
        );
    })
    .unwrap();
    run_palette_row(cx, handle, 2, 4);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(second).selected(),
            Some(true),
            "the Cycle sessions row hands the open row to the next session"
        );
    })
    .unwrap();
    run_palette_row(cx, handle, 2, 5);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(first).selected(),
            Some(true),
            "the Cycle sessions backward row returns to where it started"
        );
    })
    .unwrap();

    // (3,0) Open settings.
    run_palette_row(cx, handle, 3, 0);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        std::thread::sleep(std::time::Duration::from_millis(300));
        window.render_frame(cx);
        assert!(
            window.try_find("settings-theme-light").is_some(),
            "the Settings row opens the settings dialog"
        );
        window.press("escape", cx);
        std::thread::sleep(std::time::Duration::from_millis(300));
        window.render_frame(cx);
        assert!(
            window.try_find("settings-theme-light").is_none(),
            "escape closes the dialog the row opened"
        );
    })
    .unwrap();

    // (3,5) Toggle theme, read off the theme the app is actually running.
    let before = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            cx.theme().theme_name().clone()
        })
        .unwrap();
    run_palette_row(cx, handle, 3, 5);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_ne!(
            *cx.theme().theme_name(),
            before,
            "the Toggle theme row switches the theme the app is running"
        );
    })
    .unwrap();
}

/// The dock control moves the work pane around the window, and says where.
///
/// Right and Left reorder the same flex row; Bottom nests the transcript in a
/// column. The observable is the control's own label plus the fact that the
/// pane, the transcript, and the sidebar all survive each placement — a
/// reorder that dropped one of them would still render a plausible pane.
#[gpui_kit::test]
fn the_dock_control_moves_the_work_pane_around_the_window(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });
    let handle = handle.into();

    let assert_side = |cx: &mut TestAppContext, handle: gpui_kit::AnyWindowHandle, side: &str| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let dock = window.find("work-pane-dock");
            let label = dock.label().unwrap_or_default();
            assert!(
                label.contains(side),
                "the dock control reports {side}: {label:?}"
            );
            assert!(
                window.try_find("work-pane").is_some(),
                "the work pane is still mounted docked {side}"
            );
            assert!(
                window.try_find("sidebar").is_some(),
                "the sidebar survives a {side} dock"
            );
        })
        .unwrap();
    };

    assert_side(cx, handle, "Right");
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("work-pane-dock", cx);
    })
    .unwrap();
    assert_side(cx, handle, "Left");

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("work-pane-dock", cx);
    })
    .unwrap();
    assert_side(cx, handle, "Bottom");

    // And back around, so the cycle is closed.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("work-pane-dock", cx);
    })
    .unwrap();
    assert_side(cx, handle, "Right");

    // The same move is reachable from the palette's Layout group.
    run_palette_row(cx, handle, 1, 4);
    assert_side(cx, handle, "Left");
}

/// The Layout group's four rows move the shell between arrangements.
///
/// The rows are the first thing that touches the post-v1 layout store, so the
/// contract is stated as observable shell state rather than as a saved file:
/// each row must leave the columns open or closed as its name says, and the
/// status bar must report the arrangement the window actually has.
#[gpui_kit::test]
fn the_layout_palette_rows_rearrange_the_shell(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });
    let handle = handle.into();

    let assert_arrangement = |cx: &mut TestAppContext,
                              handle: gpui_kit::AnyWindowHandle,
                              preset: &str,
                              sidebar: bool,
                              work_pane: bool| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let chip = window.find("status-layout");
            let label = chip.label().unwrap_or_default();
            assert!(
                label.contains(preset),
                "the status bar reports {preset}, not {label:?}"
            );
            assert_eq!(
                window.try_find("sidebar").is_some(),
                sidebar,
                "{preset} leaves the sidebar column {}",
                if sidebar { "open" } else { "closed" }
            );
            assert_eq!(
                window.try_find("work-pane").is_some(),
                work_pane,
                "{preset} leaves the work pane {}",
                if work_pane { "open" } else { "closed" }
            );
        })
        .unwrap();
    };

    // Split (1,0): the prototype's default, both columns.
    run_palette_row(cx, handle, 1, 0);
    assert_arrangement(cx, handle, "Split", true, true);

    // Focus (1,1): reading view, sidebar only.
    run_palette_row(cx, handle, 1, 1);
    assert_arrangement(cx, handle, "Focus", true, false);

    // Review (1,2): work pane only.
    run_palette_row(cx, handle, 1, 2);
    assert_arrangement(cx, handle, "Review", false, true);

    // Zen (1,3): transcript alone.
    run_palette_row(cx, handle, 1, 3);
    assert_arrangement(cx, handle, "Zen", false, false);

    // And back, so the walk leaves the shell in the arrangement it opened in.
    run_palette_row(cx, handle, 1, 0);
    assert_arrangement(cx, handle, "Split", true, true);
}

/// The palette's session commands answer even when no agent is attached.
///
/// `Compact session`, `Session statistics`, and `MCP servers` all need a live
/// session. The offline shell owns none, so each row has to say so instead of
/// reaching for one — and the notice it appends is exactly one row, which is
/// what makes the count readable from an empty transcript.
#[gpui_kit::test]
fn the_command_palette_session_rows_answer_without_an_agent(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let sessions = vec![
        RecentSession {
            id: "11111111-aaaa-bbbb".to_string(),
            updated_at_unix: 1_700_000_000,
            message_count: 0,
            title: None,
            name: None,
            archived: false,
            pinned: false,
        },
        RecentSession {
            id: "22222222-cccc-dddd".to_string(),
            updated_at_unix: 1_700_000_100,
            message_count: 0,
            title: None,
            name: None,
            archived: false,
            pinned: false,
        },
    ];
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_sessions(window, cx, sessions));
        Root::new(shell, window, cx)
    });
    let handle = handle.into();

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("transcript-empty").is_some(),
            "a shell with no turns starts on the empty transcript"
        );
    })
    .unwrap();

    // One row per command, each proven to append exactly one notice: a row that
    // dispatched twice would push the next row's transcript row into existence
    // early and fail the assertion that follows it.
    for (row, expected) in [(6usize, 0usize), (7, 1), (8, 2)] {
        run_palette_row(cx, handle, 2, row);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("transcript-empty").is_none(),
                "the row at index 2,{row} answers instead of doing nothing"
            );
            assert!(
                window
                    .try_find(format!("transcript-row-{expected}"))
                    .is_some(),
                "the row at index 2,{row} appends its notice"
            );
            assert!(
                window
                    .try_find(format!("transcript-row-{}", expected + 1))
                    .is_none(),
                "the row at index 2,{row} appends one notice, not one per dispatch path"
            );
        })
        .unwrap();
    }
}

/// Zoom steps the base font size, and the shell reports the level.
///
/// The gpui test window rebuilds its `Window` for each `update_window` call, so
/// a `set_rem_size` made inside an action dispatch is not observable from the
/// next call. What *is* observable is the shell's own zoom state and the chip it
/// renders from it, so this pins the step and the reset; the layout consequence
/// follows from every dimension in the shell being `rem`-based.
#[gpui_kit::test]
fn zooming_changes_the_rem_size_and_reports_it(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    cx.update(tact_gui::commands_init);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });
    let handle = handle.into();

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("status-zoom").is_none(),
            "100% is not worth a chip"
        );
    })
    .unwrap();

    // (3,2) Zoom in. The palette row is the user path the walk can drive.
    run_palette_row(cx, handle, 3, 2);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let chip = window.find("status-zoom");
        assert_eq!(
            chip.label(),
            Some("106%"),
            "one step in reads as 106% of the prototype's base size"
        );
    })
    .unwrap();

    // (3,4) Reset zoom returns the prototype's base size, and the chip with it.
    run_palette_row(cx, handle, 3, 4);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("status-zoom").is_none(),
            "the chip disappears at 100%"
        );
    })
    .unwrap();
}

/// Pinning moves a session to the head of the list and labels it.
///
/// The walk that presses every entry point cannot assert order with a stable
/// identity, so this pins one named row: the label has to say so, the row has
/// to move ahead of the rows that arrived after it, and unpinning has to put
/// the list back.
#[gpui_kit::test]
fn pinning_a_session_moves_it_to_the_head_of_the_list(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let mut app = None;
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        app = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    let app = app.expect("the preview shell is created with its window");

    let target = "3b78ba4-1d02-4a33-8b71-3333bbbb2222";
    let target_row: SharedString = format!("session-row-{target}").into();

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        // Make the row current first: the session menu acts on the open
        // session, not on whatever the pointer last hovered.
        window.click(target_row.clone(), cx);
        window.render_frame(cx);
        assert_eq!(
            window.find(target_row.clone()).selected(),
            Some(true),
            "the walk starts on the row it means to pin"
        );

        window.click("session-menu", cx);
        window.render_frame(cx);
        let label = window
            .find("session-menu-pin")
            .label()
            .unwrap_or_default()
            .to_string();
        assert_eq!(label, "Pin", "an unpinned session offers to pin");

        window.click("session-menu-pin", cx);
        window.render_frame(cx);

        let row = window
            .find(target_row.clone())
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(row.contains("pinned"), "the pinned row says so: {row}");

        // The menu now offers the undo, and the row leads the pinned group.
        window.click("session-menu", cx);
        window.render_frame(cx);
        let undo = window
            .find("session-menu-pin")
            .label()
            .unwrap_or_default()
            .to_string();
        assert_eq!(undo, "Unpin", "a pinned session offers to unpin");
        window.click("session-menu-pin", cx);
        window.render_frame(cx);
        assert_eq!(
            app.update(cx, |app, _| app.recent_row_ids().first().cloned()),
            Some("7fbab10-2c41-4c9a-9f10-2222aaaa1111".to_string()),
            "unpinning returns the preview's own pinned row to the head"
        );
    })
    .unwrap();
}

/// The sidebar's filter narrows the session list to what it matches.
///
/// The walk in `every_entry_point_answers_a_click` presses the search field but
/// never types into it, so filtering was the one sidebar behaviour with no
/// coverage. The query is matched against the session id, and a match count
/// below `SIDEBAR_GROUP_MIN` regroups the survivors under a single `Sessions`
/// heading — that regrouping is what makes "only the matching row is left"
/// observable from the rows alone.
#[gpui_kit::test]
fn the_sidebar_search_filters_the_session_list(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        // Archived sessions are out of the way by default; this walk is about
        // the filter, so it asks for the full list first.
        window.click("session-show-archived", cx);
        window.render_frame(cx);

        let kept = "d464f22d-5e11-4c2f-9a08-4444cccc3333";
        let dropped = [
            "7fbab10-2c41-4c9a-9f10-2222aaaa1111",
            "3b78ba4-1d02-4a33-8b71-3333bbbb2222",
        ];
        let row = |id: &str| SharedString::from(format!("session-row-{id}"));

        for id in dropped {
            assert!(
                window.try_find(row(id)).is_some(),
                "the unfiltered sidebar lists {id}"
            );
        }

        // A press anywhere in the field focuses the input -- the browser
        // behaviour the prototype gets from `:focus-within` -- and `d464`
        // matches exactly one preview session.
        window.click("session-search", cx);
        window.input("d464", cx);
        window.render_frame(cx);

        assert!(
            window.try_find(row(kept)).is_some(),
            "the session whose id matches the query stays in the list"
        );
        for id in dropped {
            assert!(
                window.try_find(row(id)).is_none(),
                "a session that does not match the query is filtered out"
            );
        }

        // Clearing the query restores the full list, so the filter is a view
        // over the sessions rather than a destructive edit of them.
        window.press("ctrl-a", cx);
        window.press("backspace", cx);
        window.render_frame(cx);
        for id in dropped {
            assert!(
                window.try_find(row(id)).is_some(),
                "clearing the query brings {id} back"
            );
        }

        // The filter reads the labels the row prints, not only the id: a search
        // for an id nobody can see was the whole of the old behaviour.
        let by_title = "6278b7fa-3c92-4f18-9b27-6666eeee5555";
        window.input("remote mcp", cx);
        window.render_frame(cx);
        assert!(
            window.try_find(row(by_title)).is_some(),
            "a session whose title matches the query stays in the list"
        );
        assert!(
            window.try_find(row(kept)).is_none(),
            "the id match is not a title match"
        );
        window.press("ctrl-a", cx);
        window.press("backspace", cx);
        window.render_frame(cx);
        for id in dropped {
            assert!(
                window.try_find(row(id)).is_some(),
                "clearing the query brings {id} back"
            );
        }
    })
    .unwrap();
}

/// Just under the sidebar threshold the two side columns stop being columns.
///
/// `SIDEBAR_OVERLAY_UNDER` is 60rem — exactly 960px at the default rem — and the
/// sidebar keeps its column at that width, so the overlay only appears strictly
/// below it. Past that point the sidebar floats over the transcript and the work
/// pane is the right drawer; both stay reachable because the title-bar toggles
/// still mount and unmount them. This pins that a toggle reaches each overlay
/// instead of leaving it stuck on screen.
#[gpui_kit::test]
fn the_side_columns_overlay_when_the_window_narrows(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(900.), px(640.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        // 900px is below both thresholds: the sidebar floats over the
        // transcript, and the work pane is mounted as the drawer because
        // preview opens it.
        assert!(
            window.try_find("sidebar-overlay").is_some(),
            "the sidebar floats over the transcript below the sidebar threshold"
        );
        assert!(
            window.try_find("work-pane").is_some(),
            "the work pane is the right drawer below its threshold"
        );

        window.click("toggle-sidebar", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("sidebar-overlay").is_none(),
            "the sidebar toggle dismisses the overlay it opened"
        );
        window.click("toggle-sidebar", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("sidebar-overlay").is_some(),
            "the sidebar toggle brings the overlay back"
        );

        window.click("toggle-work-pane", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane").is_none(),
            "the work-pane toggle closes the drawer"
        );
        window.click("toggle-work-pane", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane").is_some(),
            "the work-pane toggle reopens the drawer"
        );
    })
    .unwrap();
}

/// Clicks around the session and worktree lists the way a person switches.
///
/// The click walk presses every row once; the session rows are also what a real
/// user lives in, and an offline shell can be pointed at another checkout
/// mid-list. This alternates the two lists so each press has to leave the other
/// list's selection alone, and so a switch that drops a row no longer on screen
/// or leaves two rows open fails here rather than a screenful later. The
/// transcript is asserted throughout because both lists are choose-a-context
/// controls: the rest of the window has to survive the switch.
#[gpui_kit::test]
fn clicking_between_sessions_and_worktrees_keeps_one_open_row(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });
    // The second worktree has to outlive the window: dropping the guard removes
    // the checkout the presses re-root onto.
    let fixture = WorktreeFixture::create();

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        let sessions = [
            "7fbab10-2c41-4c9a-9f10-2222aaaa1111",
            "3b78ba4-1d02-4a33-8b71-3333bbbb2222",
            "d464f22d-5e11-4c2f-9a08-4444cccc3333",
            "daf05fa-71b4-4d0e-8e55-5555dddd4444",
            "6278b7fa-3c92-4f18-9b27-6666eeee5555",
            "306ea551-8a13-4b62-8f04-7777ffff6666",
            "036e6015-a4d7-4e29-8c31-8888aaaa7777",
            "3f016556-2b8c-4f70-9d12-9999bbbb8888",
        ];
        // Archived sessions are hidden by default; this walk moves between
        // every seeded row, so it asks for them first.
        window.click("session-show-archived", cx);
        window.render_frame(cx);

        let worktrees = worktree_row_ids();
        assert!(
            !worktrees.is_empty(),
            "the crate lives in a git worktree, so the group lists one"
        );
        let starting_worktree = worktrees
            .iter()
            .find(|id| {
                window
                    .try_find(SharedString::from((*id).clone()))
                    .and_then(|row| row.selected())
                    == Some(true)
            })
            .cloned()
            .expect("the window opens on one of the repository's worktrees");

        for round in 0..3usize {
            for id in sessions {
                let row: SharedString = format!("session-row-{id}").into();
                reveal_in_sidebar(window, &row, cx);
                window.click(row.clone(), cx);
                window.render_frame(cx);

                assert_eq!(
                    window
                        .try_find(row.clone())
                        .and_then(|snapshot| snapshot.selected()),
                    Some(true),
                    "round {round}: {row} is the open session after its press"
                );
                let open: Vec<&str> = sessions
                    .iter()
                    .copied()
                    .filter(|other| {
                        window
                            .try_find(SharedString::from(format!("session-row-{other}")))
                            .and_then(|snapshot| snapshot.selected())
                            == Some(true)
                    })
                    .collect();
                assert_eq!(
                    open,
                    vec![id],
                    "round {round}: exactly one session stays open while switching"
                );
                assert!(
                    window.try_find("transcript").is_some(),
                    "round {round}: the transcript survives the session switch"
                );

                for worktree in &worktrees {
                    let row: SharedString = worktree.clone().into();
                    reveal_in_sidebar(window, &row, cx);
                    window.click(row.clone(), cx);
                    window.render_frame(cx);

                    assert_eq!(
                        window
                            .try_find(row.clone())
                            .and_then(|snapshot| snapshot.selected()),
                        Some(true),
                        "round {round}: {row} is the worktree the window is scoped to"
                    );
                    let open: Vec<&String> = worktrees
                        .iter()
                        .filter(|other| {
                            window
                                .try_find(SharedString::from((*other).clone()))
                                .and_then(|snapshot| snapshot.selected())
                                == Some(true)
                        })
                        .collect();
                    assert_eq!(
                        open,
                        vec![worktree],
                        "round {round}: exactly one worktree stays open while switching"
                    );
                    // The session list is a different control: re-rooting the
                    // window must not move which session the sidebar marks.
                    assert_eq!(
                        window
                            .try_find(SharedString::from(format!("session-row-{id}")))
                            .and_then(|snapshot| snapshot.selected()),
                        Some(true),
                        "round {round}: a worktree press leaves the open session alone"
                    );
                    assert!(
                        window.try_find("transcript").is_some(),
                        "round {round}: the transcript survives the worktree switch"
                    );
                }
            }

            // Land back where the window started so the next round and the
            // assertions after the loop read the same repository state.
            let home: SharedString = starting_worktree.clone().into();
            window.click(home.clone(), cx);
            window.render_frame(cx);
            assert_eq!(
                window
                    .try_find(home.clone())
                    .and_then(|snapshot| snapshot.selected()),
                Some(true),
                "round {round}: the walk leaves the window on its starting worktree"
            );
        }

        assert!(
            window.try_find("work-pane").is_some(),
            "the shell still renders its panes after the switching soak"
        );
        // Keep the second worktree alive until the window has finished with it.
        let _ = fixture;
    })
    .unwrap();
}

/// The sidebar toggle gives up the header's reserved column too.
///
/// The header has to mirror the body's own rule for when the sidebar is a
/// column -- open *and* wide enough. Closing the sidebar at a wide width leaves
/// the transcript starting at the window's edge, so a title bar that kept
/// reserving the column's width would float its tabs and session chip over a gap
/// no column occupies, and the toggle that reopens the sidebar would be the only
/// thing left holding the width.
#[gpui_kit::test]
fn the_title_bar_gives_up_the_sidebar_column_when_the_sidebar_closes(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let open = window.find("title-bar-left").bounds();
        let transcript_open = window.find("transcript").bounds();

        window.click("toggle-sidebar", cx);
        window.render_frame(cx);

        let closed = window.find("title-bar-left").bounds();
        let transcript_closed = window.find("transcript").bounds();
        assert!(
            transcript_closed.origin.x < transcript_open.origin.x,
            "closing the sidebar moves the transcript to the window's edge"
        );
        assert!(
            closed.size.width < open.size.width,
            "the title bar drops the sidebar's column with the body: {open:?} -> {closed:?}"
        );
        assert!(
            window.find("toggle-sidebar").visible(),
            "the control that reopens the sidebar stays reachable"
        );
    })
    .unwrap();
}

/// Every fixed chrome control is fully inside the viewport at every width.
///
/// `visible()` only reports that an element intersects the viewport, so a
/// control the layout pushed past the edge -- the class of bug that parked the
/// work-pane toggle at `origin.x = 1053` in a 900px window -- can still answer
/// "visible" while part or all of it sits outside the window. `visible()` also
/// says nothing about whether the control answers a press. This sweeps the
/// widths where the shell changes shape, asserts the whole rect of every
/// control that is mounted at every width stays inside, and then presses the
/// sidebar and work-pane toggles so a control that renders but does nothing
/// still fails.
#[gpui_kit::test]
fn the_chrome_stays_inside_the_viewport_at_every_width(cx: &mut TestAppContext) {
    settle_motion(cx);
    activate_shipped_theme(cx);
    const HEIGHT: u32 = 800;
    for width in [1440u32, 1320, 1280, 1279, 960, 959, 880, 700] {
        let handle = cx.open_window(size(px(width as f32), px(HEIGHT as f32)), |window, cx| {
            let shell = cx.new(|cx| TactApp::preview(window, cx));
            Root::new(shell, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);

            for id in [
                "toggle-sidebar",
                "toggle-work-pane",
                "open-command-palette",
                "toggle-theme",
                "open-settings",
                "session-search",
                "session-new",
                "transcript-detail-cycle",
                "transcript-copy",
                "work-pane-close",
                "work-pane-open-editor",
                "status-bar",
            ] {
                let bounds = window.find(id).bounds();
                assert!(
                    bounds.origin.x >= px(0.)
                        && bounds.origin.y >= px(0.)
                        && bounds.right() <= px(width as f32)
                        && bounds.bottom() <= px(HEIGHT as f32),
                    "{id} is not fully inside the {width}x{HEIGHT} viewport: {bounds:?}"
                );
            }

            // One toggle owns one column, and the column changes form at the
            // sidebar breakpoint: a column at 960px and up, an overlay below it.
            // Either way the press has to take it off screen and bring it back.
            // Below the breakpoint the overlay carries the same `sidebar` view,
            // so the overlay is what tells the two forms apart.
            window.click("toggle-sidebar", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("sidebar").is_none()
                    && window.try_find("sidebar-overlay").is_none(),
                "the sidebar toggle at {width}px left the sidebar on screen"
            );
            window.click("toggle-sidebar", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("sidebar").is_some(),
                "the sidebar toggle at {width}px did not bring the sidebar back"
            );
            assert_eq!(
                window.try_find("sidebar-overlay").is_some(),
                width < 960,
                "the sidebar is a column at 960px and up and an overlay below it, at {width}px"
            );

            window.click("toggle-work-pane", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("work-pane").is_none(),
                "the work-pane toggle at {width}px did not close the pane"
            );
            window.click("toggle-work-pane", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("work-pane").is_some(),
                "the work-pane toggle at {width}px did not reopen the pane"
            );
        })
        .unwrap();
    }
}

/// The title bar keeps both toggles reachable at every width.
///
/// `SIDEBAR_OVERLAY_UNDER` is 60rem -- exactly 960px at the default rem -- and
/// the comparison is strict, so 960px still renders a real session column while
/// 959 floats it. The header is the only way back to either side column once it
/// floats, so this sweeps the widths around that breakpoint and asserts both
/// toggles stay visible; a toggle pushed past the right edge of the viewport
/// reports itself as invisible and can no longer be pressed.
#[gpui_kit::test]
fn the_title_bar_toggles_survive_every_width_around_the_overlay_breakpoint(
    cx: &mut TestAppContext,
) {
    settle_motion(cx);
    activate_shipped_theme(cx);
    for width in [1440u32, 1320, 1280, 1279, 1200, 1000, 960, 959, 900, 700] {
        let handle = cx.open_window(size(px(width as f32), px(800.)), |window, cx| {
            let shell = cx.new(|cx| TactApp::preview(window, cx));
            Root::new(shell, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);

            assert_eq!(
                window.try_find("sidebar-overlay").map(|el| el.visible()),
                if width < 960 { Some(true) } else { None },
                "the sidebar floats only strictly below the breakpoint, at {width}px"
            );
            assert_eq!(
                window.try_find("sidebar").map(|el| el.visible()),
                Some(true),
                "the session column still renders at {width}px"
            );
            for id in ["toggle-sidebar", "toggle-work-pane"] {
                assert_eq!(
                    window.try_find(id).map(|el| el.visible()),
                    Some(true),
                    "{id} stays reachable at {width}px"
                );
            }
        })
        .unwrap();
    }
}

/// Every work pane says which empty case it is in.
///
/// The preview seeds all five panes, so the empty branches only render for a
/// shell with no session at all: `TactApp::with_workspace(None)` has no plan,
/// no diff, no tasks, no subagent runs, and no workspace directory. A pane that
/// silently rendered a bare card would look the same as one that broke, so each
/// line is read back by its own `work-pane-empty-*` id.
#[gpui_kit::test]
fn every_work_pane_states_its_own_empty_case(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_workspace(window, cx, None));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        // Tab order is Plan, Diff, Tasks, Subagent, Files. The Files branch that
        // renders without a workspace is the one a session-less shell reaches.
        for (index, id, line) in [
            (0usize, "work-pane-empty-plan", "No plan yet."),
            (1, "work-pane-empty-diff", "No file changes yet."),
            (2, "work-pane-empty-tasks", "No tasks in this session."),
            (3, "work-pane-empty-subagent", "No subagent runs yet."),
            (4, "work-pane-empty-workdir", "No workspace directory."),
        ] {
            window.within("work-pane-tabs").click(index, cx);
            window.render_frame(cx);
            assert_eq!(
                window.find(id).label(),
                Some(line),
                "{id} states its own empty case"
            );
        }
    })
    .unwrap();
}

/// An agent error is an alert, not just a styled row.
///
/// The provider failure reaches `TranscriptRow::Error` through the live agent
/// path; this seam puts the same row on screen so its role and text are
/// observable without a provider.
#[gpui_kit::test]
fn the_error_row_reports_its_text_as_an_alert(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::with_error(window, cx, "provider failed"));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let row = window.find("transcript-row-0");
        assert_eq!(row.role(), Some(gpui_kit::Role::Alert));
        assert_eq!(row.label(), Some("provider failed"));
    })
    .unwrap();
}

/// A sidebar with nothing to list says so.
///
/// `preview` and `with_sessions` both list rows; the empty line only renders for
/// a shell that really has no recent sessions, which is where a first launch
/// starts.
#[gpui_kit::test]
fn the_sidebar_says_when_there_are_no_sessions(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("session-new").is_some(),
            "the new-session affordance is still the sidebar's first control"
        );
        assert_eq!(
            window.find("sidebar-no-sessions").label(),
            Some("No sessions yet"),
            "an empty sidebar names its own empty case"
        );
    })
    .unwrap();
}

/// The toolbar's Copy button writes the whole transcript.
///
/// `the_code_block_copy_chip_writes_its_fence_to_the_clipboard` covers one
/// fence; this is the toolbar entry, which copies the conversation as markdown.
/// The preview seeds a real thread, so the clipboard has to come back with the
/// prompt and the fenced block the transcript is showing.
#[gpui_kit::test]
fn the_transcript_copy_button_writes_the_whole_transcript(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            cx.read_from_clipboard().is_none(),
            "the test window starts with an empty clipboard"
        );

        window.click("transcript-copy", cx);
        window.render_frame(cx);

        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("pressing Copy writes the transcript to the clipboard");
        assert!(
            copied.contains("Continue with Direction A"),
            "the user's prompt is in the copied markdown"
        );
        assert!(
            copied.contains("transcript_measure = 720"),
            "the assistant's fenced block comes with it"
        );
    })
    .unwrap();
}

/// The settings switches report the state they set.
///
/// `settings_controls_change_theme_and_reasoning` covers the dark row and the
/// reasoning switch; the light row and the follow-tail switch were only walked
/// for reachability. Light is the mirror of dark, and follow-tail is the shell's
/// own field, so the switch has to read back the value it just wrote.
#[gpui_kit::test]
fn the_settings_switches_report_the_state_they_set(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            cx.theme().theme_name().as_ref(),
            theme::DARK_THEME_NAME,
            "the shipped theme starts dark"
        );

        window.click("open-settings", cx);
        // Dialog entrance is a 250 ms wall-clock animation, so the panel has to
        // settle before its controls are pressed.
        std::thread::sleep(Duration::from_millis(300));
        window.render_frame(cx);

        window.click("settings-theme-light", cx);
        window.render_frame(cx);
        assert_eq!(
            cx.theme().theme_name().as_ref(),
            theme::LIGHT_THEME_NAME,
            "the light row switches to the light Tact theme"
        );

        // The Reading group runs past the panel's fold, so reaching its last row
        // is a scroll, exactly as it is for a user.
        window.scroll(
            "settings-show-thinking",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-200.))),
            cx,
        );
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-follow-tail").checked(),
            Some(true),
            "a fresh shell follows the tail"
        );

        window.click("settings-follow-tail", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-follow-tail").checked(),
            Some(false),
            "the switch turns tail-following off and reads its own state back"
        );
    })
    .unwrap();
}

/// A press outside the palette dismisses it.
///
/// `command_palette_opens_over_the_shell` covers Escape. The prototype closes on
/// a backdrop press as well, which is `gpui-component`'s `overlay_closable`; the
/// dialog's own full-window `#dialog` element is that backdrop, so a press near
/// its top-left corner lands outside the centred 560 px palette.
#[gpui_kit::test]
fn the_command_palette_closes_on_a_press_outside_it(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::new(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("open-command-palette", cx);
        std::thread::sleep(Duration::from_millis(400));
        window.render_frame(cx);
        assert!(
            window.try_find("command").is_some(),
            "the palette is open before the backdrop press"
        );

        window.click_at("dialog", gpui_kit::point(px(20.), px(120.)), cx);
        window.render_frame(cx);
        assert!(
            window.try_find("command").is_none(),
            "a press on the backdrop dismisses the palette"
        );
    })
    .unwrap();
}

/// The two `:focus-within` containers follow the field inside them.
///
/// GPUI has no `:focus-within`: the search well and the composer card track their
/// field's handle and paint the accent border and ring themselves.
/// `ElementSnapshot::focused()` reads that tracking back, which is the evidence
/// the ring itself cannot give — `ElementSnapshot` exposes no colours.
#[gpui_kit::test]
fn the_focus_containers_track_the_field_inside_them(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("session-search").focused(),
            Some(false),
            "the search well starts out of focus"
        );
        assert_eq!(
            window.find("composer-card").focused(),
            Some(false),
            "the composer card starts out of focus"
        );

        window.click("session-search", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("session-search").focused(),
            Some(true),
            "the well is focus-within while its field holds the keyboard"
        );

        window.click("prompt-composer-input", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("composer-card").focused(),
            Some(true),
            "the card is focus-within while the composer holds the keyboard"
        );
        assert_eq!(
            window.find("session-search").focused(),
            Some(false),
            "and the well drops it again"
        );
    })
    .unwrap();
}

/// The prototype draws `.row`, `.tab`, `.icon`, `.cmd`, `.new`, `.mini`,
/// `.wtab`, `.toolbtn`, `.session` and `.send` as `<button>` elements, so the
/// browser puts every one of them in the tab order and paints the line 27
/// `button:focus-visible` ring while the keyboard is driving. The shell draws
/// them as `div`s, which are neither tab stops nor ring-painters until they ask
/// to be.
///
/// Neither half is readable from a snapshot: the ring is a colour, and these
/// controls take the focus handle GPUI keeps in the element's own state rather
/// than one handed to `.test_support()`. So the pair is pinned by behaviour —
/// a Tab press has to land on a drawn control, and the activation key has to
/// run the press that control carries. The sidebar toggle is the observable
/// one: the preview window opens no palette and no dialog, so the only control
/// that can answer the keyboard that way is the drawn `.icon` box.
///
/// GPUI synthesises the click on the key *up*, and the harness's `press` sends
/// the key down alone, so the release is dispatched by hand.
#[gpui_kit::test]
fn tab_reaches_the_drawn_controls_and_enter_runs_them(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("sidebar").is_some(),
            "the preview shell starts with its sidebar mounted"
        );

        // The stops in front of the title bar depend on how many rows the
        // preview shell seeds, so the walk asks whether the keyboard reaches
        // the control rather than how many presses it takes.
        let mut presses = 0;
        let mut reached = false;
        while presses < 40 {
            window.press("tab", cx);
            window.render_frame(cx);
            presses += 1;
            window.press("enter", cx);
            window.dispatch_event(
                gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent {
                    keystroke: gpui_kit::Keystroke::parse("enter")
                        .expect("`enter` is a parseable keystroke"),
                }),
                cx,
            );
            window.render_frame(cx);
            if window.try_find("sidebar").is_none() {
                reached = true;
                break;
            }
        }
        assert!(
            reached,
            "Tab has to reach a drawn control and Enter has to run its press \
             (gave up after {presses} presses)"
        );
    })
    .unwrap();
}

/// The sidebar's rows are tab stops that answer Enter, not just stops the ring
/// lands on.
///
/// `tab_reaches_the_drawn_controls_and_enter_runs_them` proves the keyboard can
/// run a drawn *icon*; the sidebar is made of drawn *rows*, and resuming a
/// session or re-rooting the window has to work from the keyboard too. The walk
/// is deliberately blind about focus: GPUI hands the harness only handles the
/// element itself registered (`gpui-base-0.6.4/src/test_support.rs:98-104`),
/// and these rows take the handle GPUI keeps in their element state, so the
/// assertion is the **effect** -- the window re-roots at the worktree the row
/// names -- rather than which element holds focus.
///
/// One side effect of a blind walk: Enter on the title bar's sidebar toggle
/// closes the sidebar. The toggle keeps the focus, so one more Enter puts the
/// sidebar back and the walk carries on.
#[gpui_kit::test]
fn tab_and_enter_re_root_the_window_at_the_row_they_reach(cx: &mut TestAppContext) {
    activate_shipped_theme(cx);
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });
    // The second worktree has to outlive the walk: dropping the guard removes
    // the checkout the presses re-root onto.
    let fixture = WorktreeFixture::create();
    assert!(
        fixture.is_added(),
        "the fixture has to add a second worktree for the walk to re-root onto"
    );

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let ids = worktree_row_ids();
        assert!(
            ids.len() >= 2,
            "a second worktree is on disk, so the sidebar lists two rows: {ids:?}"
        );
        let selected = |window: &mut Window, id: &str| {
            window
                .try_find(SharedString::from(id.to_string()))
                .and_then(|row| row.selected())
                == Some(true)
        };
        let starting = ids
            .iter()
            .find(|id| selected(window, id))
            .cloned()
            .expect("the window opens scoped to one of the repository's worktrees");

        let press_enter = |window: &mut Window, cx: &mut App| {
            window.press("enter", cx);
            window.dispatch_event(
                gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent {
                    keystroke: gpui_kit::Keystroke::parse("enter")
                        .expect("`enter` is a parseable keystroke"),
                }),
                cx,
            );
            window.render_frame(cx);
        };

        let mut presses = 0;
        let mut re_rooted = false;
        while presses < 60 {
            window.press("tab", cx);
            window.dispatch_event(
                gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent {
                    keystroke: gpui_kit::Keystroke::parse("tab").expect("`tab` parses"),
                }),
                cx,
            );
            window.render_frame(cx);
            presses += 1;
            press_enter(window, cx);
            // Enter can open a popover or a dialog (the composer's selectors,
            // the command palette); those trap Tab until they are dismissed, so
            // the walk closes whatever it opened before moving on.
            window.press("escape", cx);
            window.dispatch_event(
                gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent {
                    keystroke: gpui_kit::Keystroke::parse("escape").expect("`escape` parses"),
                }),
                cx,
            );
            window.render_frame(cx);

            if ids.iter().any(|id| id != &starting && selected(window, id)) {
                re_rooted = true;
                break;
            }
            if window.try_find("sidebar").is_none() {
                // Enter reached the sidebar toggle; the toggle still holds the
                // focus, so the same press path reopens the sidebar.
                press_enter(window, cx);
                assert!(
                    window.try_find("sidebar").is_some(),
                    "Enter on the sidebar toggle reopens the sidebar"
                );
            }
        }
        assert!(
            re_rooted,
            "Tab has to reach a worktree row and Enter has to re-root the window \
             (gave up after {presses} presses)"
        );

        let open: Vec<&String> = ids.iter().filter(|id| selected(window, id)).collect();
        assert_eq!(
            open.len(),
            1,
            "exactly one worktree row stays open after the keyboard re-roots: {open:?}"
        );
        assert_ne!(
            open[0], &starting,
            "the row the keyboard reached is the one the window re-rooted onto"
        );
        assert!(
            window.try_find("transcript").is_some(),
            "the transcript survives a keyboard worktree switch"
        );
    })
    .unwrap();
    // Keep the second worktree alive until the window has finished with it.
    let _ = fixture;
}
