//! UI integration tests for the Tact desktop shell.

use gpui_kit::component::{ActiveTheme as _, Root, ThemeMode, ThemeRegistry};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext, px, size};

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

fn width(snapshot: gpui_kit::Bounds<gpui_kit::Pixels>) -> f32 {
    f32::from(snapshot.size.width)
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
        },
        RecentSession {
            id: "22222222-cccc-dddd".to_string(),
            updated_at_unix: 1_700_000_100,
            message_count: 0,
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
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
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
    let handle = cx.open_window(size(px(1440.), px(900.)), |window, cx| {
        let shell = cx.new(|cx| TactApp::preview(window, cx));
        Root::new(shell, window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
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
