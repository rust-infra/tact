//! UI integration tests for the Tact desktop shell.

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
            420.,
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
        assert_eq!(width(work_pane), 420., "work pane keeps its 420 px column");
        assert_eq!(
            width(transcript),
            1440. - 260. - 420.,
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
        },
        RecentSession {
            id: "22222222-cccc-dddd".to_string(),
            updated_at_unix: 1_700_000_100,
            message_count: 0,
            title: None,
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
            window.try_find("worktree-row-feat/gpu").is_some(),
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
        let worktree = window.find("worktree-row-feat/gpu").bounds();
        let background = window.find("background-row-cargo test -p tact").bounds();
        assert!(
            worktree.origin.y < background.origin.y,
            "worktrees lead the background group"
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

        // Tab order is Plan, Diff, Tasks, Subagent, Files.
        for (index, body) in [
            (0usize, "work-pane-body-plan"),
            (1, "work-pane-body-diff"),
            (3, "work-pane-body-subagent"),
            (4, "work-pane-body-files"),
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

/// The status bar exposes the prototype's segments rather than a single string.
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

    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.scroll_transcript_to(3, cx));
        window.render_frame(cx);
        // Preview row 3 is the first tool card, seeded collapsed.
        assert!(
            window.try_find("tool-output-3").is_none(),
            "a collapsed tool card hides its output block"
        );

        window.click("tool-summary-3", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("tool-output-3").is_some(),
            "clicking the summary opens the output block"
        );

        window.click("tool-summary-3", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("tool-output-3").is_none(),
            "clicking the summary again closes it"
        );
    })
    .unwrap();
}

/// The prototype's permission card lists the refusal before the grants; the
/// agent core hands the choices over in a different order, so the card's own
/// order is what this pins down.
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

        // The preview seeds the driver's own trio: 0 = Allow once, 1 = Deny,
        // 2 = Always allow this tool.
        let deny = window.find("request-option-1").bounds();
        let allow_once = window.find("request-option-0").bounds();
        let allow_always = window.find("request-option-2").bounds();
        assert!(
            deny.origin.x < allow_once.origin.x,
            "the refusal leads: {deny:?} against {allow_once:?}"
        );
        assert!(
            allow_once.origin.x < allow_always.origin.x,
            "the lasting grant stays last: {allow_once:?} against {allow_always:?}"
        );
        assert!(
            deny.origin.y == allow_once.origin.y,
            "the options share one row: {deny:?} against {allow_once:?}"
        );
        for (id, bounds) in [
            ("request-option-1", deny),
            ("request-option-0", allow_once),
            ("request-option-2", allow_always),
        ] {
            assert_eq!(height(bounds), 28., "`.btn` is 28 px tall ({id})");
        }
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
        app.update(cx, |app, cx| app.scroll_transcript_to(5, cx));
        window.render_frame(cx);
        // Preview row 4 is the edit card; row 3 is its read sibling.
        assert!(
            window.try_find("work-pane-body-plan").is_some(),
            "the preview opens on the plan pane"
        );
        assert!(
            window.try_find("tool-diff-3").is_none(),
            "a read has no change to open"
        );
        assert!(
            window.try_find("tool-output-5").is_none(),
            "the edit card starts collapsed"
        );

        window.click("tool-diff-5", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("work-pane-body-diff").is_some(),
            "the badge switches the work pane to Diff"
        );
        assert!(
            window.try_find("tool-output-5").is_none(),
            "the badge does not also expand the tool card"
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
            window.try_find("request-option-1").is_none(),
            "the approval is virtualized below the initial viewport"
        );

        app.update(cx, |app, cx| app.scroll_transcript_to_end(cx));
        window.render_frame(cx);
        assert!(
            window.try_find("session-intro-detail-cycle").is_none(),
            "scrolling to the tail releases the header row"
        );
        assert!(
            window.try_find("request-option-1").is_some(),
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
/// Three controls stay out of it: the sidebar avatar (it has no handler at
/// all), `composer-attachment-*` (a press opens a native file dialog), and the
/// request-option rows (a press would resolve the seeded request). The session
/// rows are walkable only because the preview is an offline shell: there a row
/// click moves the sidebar's selection instead of starting an agent runtime.
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
struct WorktreeFixture {
    path: std::path::PathBuf,
    added: bool,
}

impl WorktreeFixture {
    fn create() -> Self {
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
        Self { path, added }
    }

    /// Whether a second worktree is on disk, i.e. whether a click can switch.
    fn is_added(&self) -> bool {
        self.added
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
            assert_eq!(
                worktrees.len(),
                2,
                "the fixture worktree and this checkout are both listed"
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
        click!(home.clone());
        assert_eq!(
            window.find(home.clone()).selected(),
            Some(true),
            "the walk leaves the window on the worktree it started in"
        );

        // Work pane: every tab, then the control each tab owns.
        for index in 0..5usize {
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
        click!("work-pane-close");
        click!("toggle-work-pane");

        // Composer: the mention chip and its completion first, while the draft
        // is empty, then every popover chip and every entry inside it.
        //
        // A gpui-component popover dismisses on a press *outside* it, and stays
        // open through the presses its own buttons take. So each panel is
        // dismissed before the next trigger is pressed: leaving one open would
        // make the next trigger's press a dismissal, and its entries would never
        // render. The work pane's first tab is far enough away to be that press.
        let dismiss_popover = |window: &mut Window, cx: &mut App, panel: &'static str| {
            window.within("work-pane-tabs").click(0usize, cx);
            window.render_frame(cx);
            assert!(
                window.try_find(panel).is_none(),
                "{panel} closes on a press outside it"
            );
        };
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
                    "composer-model-gpt-5",
                    "composer-budget-0",
                    "composer-budget-16384",
                ][..],
            ),
            (
                "composer-effort",
                "composer-effort-panel",
                &[
                    "composer-effort-auto",
                    "composer-effort-low",
                    "composer-effort-high",
                    "composer-effort-max",
                ][..],
            ),
            (
                "composer-permission",
                "composer-permission-panel",
                &["composer-permission-auto", "composer-permission-plan"][..],
            ),
        ] {
            for item in items {
                click!(chip);
                click!(*item);
            }
            dismiss_popover(window, cx, panel);
        }

        // The context ring is a real-data control: a session that has reported
        // no token usage has no ring to click, so the preview has none and the
        // walk checks the ring only when a session put one on screen.
        if window.try_find("composer-usage").is_some() {
            click!("composer-usage");
            assert!(
                window.try_find("composer-usage-panel").is_some(),
                "the context ring opens its own panel"
            );
            dismiss_popover(window, cx, "composer-usage-panel");
        } else {
            assert!(
                window.try_find("composer-usage-panel").is_none(),
                "no usage, no ring"
            );
        }

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
