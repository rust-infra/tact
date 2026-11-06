//! Offscreen visual capture harness for the real Tact Desktop shell.
//!
//! The harness renders the production `DesktopRoot` through GPUI's real
//! platform renderer into an off-screen window, so it produces the same pixels
//! the compositor would show without needing Screen Recording permission. It is
//! used by `examples/ui_capture.rs` to walk every surface and write PNGs for
//! visual review.

use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, OnceLock},
};

use anyhow::{Context as _, Result};
use gpui::{AnyWindowHandle, AppContext as _, Entity, VisualTestAppContext, px, size};
use gpui_component::Root;
use tact_catalog::SettingId;
use tact_desktop_core::{
    Appearance, DesktopConfig, DesktopController, DesktopSnapshot, DesktopTheme, ExtensionsTab,
    HookItemSnapshot, HookSnapshot, InspectorTab, LatestToolView, ReaderRef, RowBody, RowId,
    RowStatus, RunPhase, SessionPhase, SurfaceSnapshot, ToolBody, ToolGroupItem, ToolGroupRow,
    ToolMetaView, ToolOutputView, ToolRow, TranscriptRow, TranscriptView, transcript::ThinkingRow,
};

use crate::root::{DesktopRoot, ReaderKind};
use crate::ui::shell::RunTab;

const WINDOW_WIDTH: f32 = 1280.0;
const WINDOW_HEIGHT: f32 = 840.0;
const NARROW_WIDTH: f32 = 900.0;
const NARROW_HEIGHT: f32 = 700.0;

/// Tact configuration is process-global; a second harness in the same process
/// must not reinstall it.
static PROCESS_CONFIG: OnceLock<Result<(), String>> = OnceLock::new();

pub struct Harness {
    pub cx: VisualTestAppContext,
    root: Option<Entity<DesktopRoot>>,
    pub window: AnyWindowHandle,
    pub controller: DesktopController,
    theme: DesktopTheme,
    data_dir: PathBuf,
}

impl Harness {
    pub fn open(appearance: Appearance, text_scale: f32) -> Result<Self> {
        Self::open_themed(
            DesktopTheme::Tact,
            appearance,
            text_scale,
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
        )
    }

    pub fn open_sized(
        appearance: Appearance,
        text_scale: f32,
        width: f32,
        height: f32,
    ) -> Result<Self> {
        Self::open_themed(DesktopTheme::Tact, appearance, text_scale, width, height)
    }

    pub fn open_themed(
        theme: DesktopTheme,
        appearance: Appearance,
        text_scale: f32,
        width: f32,
        height: f32,
    ) -> Result<Self> {
        let platform = gpui_platform::current_platform(true);
        let mut cx =
            VisualTestAppContext::with_asset_source(platform, Arc::new(crate::assets::Assets));

        let data_dir = std::env::temp_dir().join(format!(
            "tact-ui-capture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&data_dir).context("create capture data dir")?;
        let mut config = DesktopConfig::load(data_dir.clone())
            .map_err(|error| anyhow::anyhow!("load desktop config: {error:?}"))?;
        config.appearance = appearance;
        config.theme = theme;
        config.text_scale = text_scale;
        // The harness must not read the checkout's real `.tact` store: session
        // rows would change with the developer's history and the captures
        // would not be reproducible. Point the controller at a disposable
        // project and seed fixed rows after startup.
        let project_dir = data_dir.join("tact");
        std::fs::create_dir_all(&project_dir).context("create capture project")?;
        config.initial_project = Some(project_dir.clone());
        let installed = PROCESS_CONFIG.get_or_init(|| {
            config
                .install_process_config()
                .map_err(|error| format!("install desktop config: {error:?}"))
        });
        if let Err(error) = installed {
            anyhow::bail!("{error}");
        }
        let (controller, _events) = DesktopController::start(config)
            .map_err(|error| anyhow::anyhow!("start desktop controller: {error:?}"))?;
        let shutdown = controller.clone();

        // Screenshots must show settled surfaces, so scripted entrances are
        // skipped the same way the reduced-motion setting skips them.
        cx.update(|cx| cx.set_reduce_motion(true));

        let root_slot: Rc<std::cell::RefCell<Option<Entity<DesktopRoot>>>> = Default::default();
        let root_slot_for_window = root_slot.clone();
        let window = cx
            .open_offscreen_window(size(px(width), px(height)), move |window, cx| {
                gpui_component::init(cx);
                crate::theme::apply(theme, appearance, Some(window), cx);
                let root = cx.new(|cx| DesktopRoot::new_for_test(shutdown, window, cx));
                *root_slot_for_window.borrow_mut() = Some(root.clone());
                cx.new(|cx| Root::new(root, window, cx))
            })
            .context("open offscreen window")?;
        let window: AnyWindowHandle = window.into();
        let root = root_slot
            .borrow_mut()
            .take()
            .context("root entity was not created")?;

        let mut harness = Self {
            cx,
            root: Some(root),
            window,
            controller,
            theme,
            data_dir,
        };
        harness.seed_capture_sessions(&project_dir);
        Ok(harness)
    }

    /// Replaces the live project/session snapshot with fixed rows so every
    /// capture is deterministic and contains no developer session data.
    fn seed_capture_sessions(&mut self, project_dir: &Path) {
        use chrono::Utc;
        use tact_desktop_core::{ProjectId, SessionId, SessionSummaryView};

        // Offsets are relative to now so every run shows the same relative
        // labels ("2m ago", "1h ago", "Yesterday") instead of drifting.
        let base = Utc::now();
        let rows = [
            (
                "session-8f2a",
                "分析下这个库能否满足当前tact所有的功能，如果要接入desktop的话",
                46,
                2,
            ),
            ("session-4c19", "Tool output compaction layout", 112, 65),
            ("session-9d03", "Agent kernel parity", 203, 185),
            (
                "session-2b77",
                "Review the permission gate layout",
                31,
                1_500,
            ),
        ];
        let items = rows
            .iter()
            .map(|(id, title, count, minutes)| SessionSummaryView {
                id: (*id).to_string(),
                project_id: ProjectId("tact".into()),
                root_dir: project_dir.to_path_buf(),
                title: (*title).to_string(),
                updated_at: base - chrono::Duration::minutes(*minutes),
                archived_at: None,
                locked: false,
                message_count: *count,
                search_score: 0,
            })
            .collect();

        let mut snapshot = self.snapshot();
        snapshot.project.id = Some(ProjectId("tact".into()));
        snapshot.project.root = Some(project_dir.to_path_buf());
        snapshot.project.display_name = Arc::from("tact");
        snapshot.project.branch = Some(Arc::from("feat/desktop"));
        snapshot.project.repository = Some(Arc::from("rust-infra/tact"));
        snapshot.project.available = true;
        // The registry would otherwise carry the controller's project id,
        // while the seeded sessions use the stable `tact` id.
        snapshot.projects = Arc::from([snapshot.project.clone()]);
        snapshot.sessions.project = Some(ProjectId("tact".into()));
        snapshot.sessions.items = items;
        snapshot.sessions.selected = Some(SessionId("session-8f2a".into()));
        snapshot.session.id = Some(SessionId("session-8f2a".into()));
        snapshot.session.title =
            Arc::from("分析下这个库能否满足当前tact所有的功能，如果要接入desktop的话");
        snapshot.session.phase = SessionPhase::Ready;
        snapshot.account = Some(tact_desktop_core::AccountView::UsageQuota {
            plan: Some(Arc::from("Kimi K2.5")),
            used: Some(78),
            limit: Some(100),
            resets_at: Some(Arc::from("in 3h 12m")),
        });
        self.set_snapshot(snapshot);
    }

    pub fn draw(&mut self) {
        // Let scripted transitions (tab fades, dialog entrances) reach their
        // final frame before the pixels are captured.
        self.cx.advance_clock(std::time::Duration::from_millis(240));
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.draw(cx).clear(cx);
            })
            .expect("draw capture window");
        self.cx.advance_clock(std::time::Duration::from_millis(240));
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.draw(cx).clear(cx);
            })
            .expect("draw capture window");
        self.cx.run_until_parked();
    }

    pub fn snapshot(&self) -> DesktopSnapshot {
        self.cx.read(|cx| {
            self.root
                .as_ref()
                .expect("capture root")
                .read(cx)
                .snapshot()
                .as_ref()
                .clone()
        })
    }

    pub fn set_snapshot(&mut self, snapshot: DesktopSnapshot) {
        self.cx.update(|cx| {
            self.root
                .as_ref()
                .expect("capture root")
                .update(cx, |root, cx| {
                    root.set_snapshot_for_capture(Arc::new(snapshot), cx);
                });
        });
    }

    /// Runs `f` with the live root, window, and app context.
    pub(crate) fn with_root<R>(
        &mut self,
        f: impl FnOnce(&mut DesktopRoot, &mut gpui::Window, &mut gpui::Context<DesktopRoot>) -> R,
    ) -> R {
        let root = self.root.clone().expect("capture root");
        self.cx
            .update_window(self.window, |_, window, cx| {
                root.update(cx, |root, cx| f(root, window, cx))
            })
            .expect("update root")
    }

    pub fn set_appearance(&mut self, appearance: Appearance) {
        let theme = self.theme;
        self.cx
            .update_window(self.window, |_, window, cx| {
                crate::theme::apply(theme, appearance, Some(window), cx);
            })
            .expect("apply appearance");
        let mut snapshot = self.snapshot();
        snapshot.appearance = appearance;
        self.set_snapshot(snapshot);
    }

    pub fn set_theme(&mut self, theme: DesktopTheme) {
        self.theme = theme;
        let appearance = self.snapshot().appearance;
        self.cx
            .update_window(self.window, |_, window, cx| {
                crate::theme::apply(theme, appearance, Some(window), cx);
            })
            .expect("apply theme");
        let mut snapshot = self.snapshot();
        snapshot.theme = theme;
        self.set_snapshot(snapshot);
    }

    pub fn simulate_keystrokes(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(self.window, keys);
        self.draw();
    }

    pub fn simulate_input(&mut self, text: &str) {
        self.cx.simulate_input(self.window, text);
        self.draw();
    }

    pub fn click(&mut self, x: f32, y: f32) {
        self.cx.simulate_click(
            self.window,
            gpui::point(px(x), px(y)),
            gpui::Modifiers::default(),
        );
        self.draw();
    }

    pub fn capture(&mut self, out: &Path, name: &str) -> Result<PathBuf> {
        self.draw();
        let image = self
            .cx
            .capture_screenshot(self.window)
            .with_context(|| format!("capture {name}"))?;
        let path = out.join(format!("{name}.png"));
        image
            .save(&path)
            .with_context(|| format!("save {path:?}"))?;
        Ok(path)
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.cx
            .update_window(self.window, |_, window, _| {
                window.resize(size(px(width), px(height)));
            })
            .expect("resize capture window");
        self.draw();
    }

    /// Stops the controller, removes the off-screen window, and skips the
    /// visual-test leak assertion at process exit.
    pub fn finish(mut self) {
        let _ = self.controller.shutdown_blocking();
        let _ = std::fs::remove_dir_all(&self.data_dir);
        let window = self.window;
        let _ = self
            .cx
            .update_window(window, |_, window, _| window.remove_window());
        self.cx.run_until_parked();
        // The harness owns the only remaining entity handles; the process is
        // exiting, so let them go with the app instead of tripping the
        // visual-test leak detector during teardown.
        std::mem::forget(self);
    }
}

/// Walks every shell surface and writes one PNG per surface.
pub fn capture_all(out: &Path) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(out)?;
    let mut harness = Harness::open(Appearance::Light, 1.0)?;
    let mut shots = Vec::new();
    let mut shoot = |harness: &mut Harness, name: &str| -> Result<()> {
        shots.push(harness.capture(out, name)?);
        Ok(())
    };

    eprintln!("capture: opened harness");
    harness.draw();
    shoot(&mut harness, "01-shell-light")?;
    eprintln!("capture: shell light");

    harness.set_appearance(Appearance::Dark);
    shoot(&mut harness, "02-shell-dark")?;
    harness.set_appearance(Appearance::Light);

    harness.set_theme(DesktopTheme::Claude);
    shoot(&mut harness, "02b-shell-claude-light")?;
    harness.set_appearance(Appearance::Dark);
    shoot(&mut harness, "02c-shell-claude-dark")?;
    harness.set_appearance(Appearance::Light);
    harness.set_theme(DesktopTheme::Tact);

    harness.with_root(|root, window, cx| root.capture_open_command_center(window, cx));
    shoot(&mut harness, "03-command-center")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, window, cx| root.capture_open_slash(window, cx));
    shoot(&mut harness, "04-slash-overlay")?;
    harness.with_root(|root, window, cx| root.capture_reset_composer(window, cx));
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, _, _| root.capture_open_settings());
    shoot(&mut harness, "05-settings")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, _, _| root.capture_open_session_manager());
    shoot(&mut harness, "06-session-manager")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    for (name, tab) in [
        ("07-extensions-mcp", ExtensionsTab::Mcp),
        ("08-extensions-skills", ExtensionsTab::Skills),
        ("09-extensions-plugins", ExtensionsTab::Plugins),
        ("10-extensions-hooks", ExtensionsTab::Hooks),
    ] {
        harness.with_root(|root, _, _| root.capture_open_extensions(tab));
        shoot(&mut harness, name)?;
        harness.with_root(|root, _, cx| root.capture_close_overlay(cx));
    }

    harness.with_root(|root, _, cx| {
        let mut snapshot = root.snapshot().as_ref().clone();
        snapshot.surface = Some(SurfaceSnapshot::Hooks(HookSnapshot {
            trusted: vec![HookItemSnapshot {
                source: "~/.tact/hooks.json".into(),
                event: "session_start".into(),
                matcher: None,
                command: "tact notify started".into(),
            }],
            pending: vec![
                HookItemSnapshot {
                    source: ".tact/hooks.json".into(),
                    event: "post_tool".into(),
                    matcher: Some("cargo".into()),
                    command: "cargo fmt".into(),
                },
                HookItemSnapshot {
                    source: "plugin ponytail".into(),
                    event: "post_tool".into(),
                    matcher: None,
                    command: "format".into(),
                },
            ],
        }));
        root.set_snapshot_for_capture(Arc::new(snapshot), cx);
        root.capture_open_hooks();
    });
    shoot(&mut harness, "10c-hooks-sheet")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, _, _| {
        root.capture_open_subagent("child-2");
    });
    shoot(&mut harness, "30b-subagent-drawer")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, _, _| {
        root.capture_open_extensions_manager(ExtensionsTab::Mcp);
    });
    shoot(&mut harness, "10b-extensions-manager")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, _, cx| root.capture_open_background(cx));
    shoot(&mut harness, "11-background")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    for (name, tab) in [
        ("12-inspector-run", InspectorTab::Run),
        ("13-inspector-goals", InspectorTab::Goals),
        ("14-inspector-subagents", InspectorTab::Subagents),
        ("15-inspector-context", InspectorTab::Context),
    ] {
        harness.with_root(|root, _, cx| root.capture_set_inspector(tab, cx));
        shoot(&mut harness, name)?;
    }
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    for (name, kind) in [
        ("16-reader-system-prompt", ReaderKind::SystemPrompt),
        ("17-reader-stats", ReaderKind::Stats),
        ("18-reader-history", ReaderKind::History),
        ("19-reader-tasks-dag", ReaderKind::TasksDag),
        ("20-reader-balance", ReaderKind::Balance),
    ] {
        harness.with_root(|root, _, _| root.capture_open_reader(kind));
        shoot(&mut harness, name)?;
        harness.with_root(|root, _, cx| root.capture_close_overlay(cx));
    }

    harness.with_root(|root, _, cx| root.capture_open_model_picker(SettingId::Model, cx));
    shoot(&mut harness, "21-model-picker")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, window, cx| {
        root.capture_open_setting_form(SettingId::BaseUrl, window, cx);
    });
    shoot(&mut harness, "22-command-form")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    fn design_transcript() -> tact_desktop_core::TranscriptView {
        use tact_desktop_core::RowBody;
        let transcript = tact_desktop_core::fixtures::demo_transcript();
        let rows = transcript
            .iter()
            .map(|row| {
                let mut row = (**row).clone();
                match row.id.value() {
                    0 => {
                        if let RowBody::User(user) = &mut row.body {
                            user.text = Arc::from(
                                "https://labcoder.github.io/gpui-ai/components 分析下这个库能否满足当前tact所有的功能，如果要接入desktop的话\n以第2个目标来设计，可以先出prd原型图，用html画原型，采用适合的pstack skill来处理",
                            );
                        }
                    }
                    7 => {
                        if let RowBody::Assistant(assistant) = &mut row.body {
                            assistant.markdown = Arc::from(
                                "I'll keep `tact_host` as the runtime boundary and give the desktop one reducer over `AgentUpdate`. The shell will use gpui-ai leaf components, with a custom virtualized transcript for mixed rows.",
                            );
                            assistant.suggestions = Arc::from([]);
                        }
                    }
                    _ => {}
                }
                Arc::new(row)
            })
            .collect::<Vec<_>>();
        tact_desktop_core::TranscriptView::from_rows(rows)
    }

    // Seed representative runtime data so every transcript row kind and every
    // inspector panel renders with real content rather than an empty state.
    let mut snapshot = harness.snapshot();
    snapshot.transcript = design_transcript();
    snapshot.inspector = tact_desktop_core::fixtures::demo_inspector();
    snapshot.failure = None;
    snapshot.tail_following = false;
    snapshot.session.phase = SessionPhase::Running;
    snapshot.runtime.usage.total_tokens = 136_000;
    snapshot.runtime.usage.prompt_tokens = 96_000;
    snapshot.runtime.usage.cache_hit_tokens = 72_000;
    snapshot.runtime.turns_taken = 7;
    snapshot.runtime.current_phase = tact_desktop_core::RunPhase::RunningTools;
    snapshot.runtime.latest_tool = Some("edit_file bridge.rs".into());
    snapshot.runtime.latest_tool_detail = Some(tact_desktop_core::LatestToolView {
        tool_id: "call-edit-bridge".into(),
        group: None,
        row: None,
        label: "edit_file bridge.rs".into(),
        duration_ms: None,
        status: tact_desktop_core::RowStatus::Running,
    });
    snapshot.pending_gates = std::sync::Arc::from([tact_desktop_core::fixtures::demo_gate()]);
    harness.set_snapshot(snapshot);
    harness.with_root(|root, _, cx| root.capture_scroll_to(0, cx));
    shoot(&mut harness, "26a-transcript-top")?;
    let last_row = harness.snapshot().transcript.len().saturating_sub(1);
    harness.with_root(|root, _, cx| root.capture_scroll_to(last_row, cx));
    shoot(&mut harness, "26b-transcript-bottom")?;
    // The expanded strip, grouped in progress -> pending -> completed.
    harness.with_root(|root, _, cx| root.capture_set_sticky_tasks_collapsed(false, cx));
    shoot(&mut harness, "26b-sticky-tasks-grouped")?;
    harness.with_root(|root, _, cx| root.capture_set_sticky_tasks_collapsed(true, cx));
    harness.with_root(|root, _, cx| root.capture_scroll_to(4, cx));
    shoot(&mut harness, "26c-transcript-tools")?;
    harness.with_root(|root, _, cx| root.capture_scroll_to(3, cx));
    shoot(&mut harness, "26d-transcript-plan")?;
    // The design transcript above shortens the answer; this capture keeps the
    // fixture's Markdown heading and lists so the prose scale is reviewable.
    let mut snapshot = harness.snapshot();
    snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
    harness.set_snapshot(snapshot);
    harness.with_root(|root, _, cx| root.capture_scroll_to(3, cx));
    shoot(&mut harness, "26e-assistant-markdown")?;
    let mut snapshot = harness.snapshot();
    snapshot.transcript = design_transcript();
    harness.set_snapshot(snapshot);

    // A real `bash` summary arrives as a multi-line script. The group header
    // must flatten it to one label instead of growing to the command's shape.
    let long_command = tact_desktop_core::TranscriptView::from_rows(
        design_transcript()
            .iter()
            .map(|row| {
                let mut row = (**row).clone();
                if let tact_desktop_core::RowBody::ToolGroup(group) = &mut row.body {
                    let mut items = group.items.to_vec();
                    if let Some(tact_desktop_core::ToolGroupItem::Tool(tool)) = items.first_mut() {
                        tool.arg_summary = Arc::from(
                            "export https_proxy=http://127.0.0.1:7890 \
                             http_proxy=http://127.0.0.1:7890 all_proxy=http://127.0.0.1:7890\n\
                             echo \"--- bitcoin\"\n\
                             bash python3 -c \"\n\
                             import datetime, time\n\
                             print('ok ts ->', datetime.datetime.fromtimestamp(1793533960))\"",
                        );
                    }
                    group.items = Arc::from(items);
                }
                Arc::new(row)
            })
            .collect::<Vec<_>>(),
    );
    let mut snapshot = harness.snapshot();
    snapshot.transcript = long_command;
    harness.set_snapshot(snapshot);
    // A running process group replaces its counts with the live action and
    // clock; the settled group keeps the elapsed time.
    let demo = tact_desktop_core::fixtures::demo_transcript();
    let loop_id = demo
        .iter()
        .find_map(|row| match &row.body {
            RowBody::ToolGroup(group) => Some(group.loop_id),
            _ => None,
        })
        .expect("demo tool group");
    let rows = vec![
        Arc::new(TranscriptRow {
            id: RowId::new(0),
            ordinal: 0,
            loop_id: None,
            time: None,
            body: RowBody::Thinking(ThinkingRow {
                markdown: Arc::from("plan"),
                status: RowStatus::Success,
                duration_ms: None,
            }),
        }),
        Arc::new(TranscriptRow {
            id: RowId::new(1),
            ordinal: 1,
            loop_id: Some(loop_id),
            time: None,
            body: RowBody::ToolGroup(ToolGroupRow {
                loop_id,
                items: Arc::from([ToolGroupItem::Tool(ToolRow {
                    id: RowId::new(11),
                    tool_id: Arc::from("tool-1"),
                    name: Arc::from("edit_file"),
                    arg_summary: Arc::from("root.rs"),
                    arg_full: None,
                    status: RowStatus::Running,
                    body: ToolBody::None,
                    output: ToolOutputView::default(),
                    meta: ToolMetaView::default(),
                    subagent: None,
                    expanded: false,
                    keep_live: false,
                    destructive: false,
                    duration_ms: None,
                })]),
                summary: Arc::from("edit_file"),
                collapsed: true,
                collapse_override: None,
            }),
        }),
    ];
    let mut snapshot = harness.snapshot();
    snapshot.transcript = TranscriptView::from_rows(rows);
    snapshot.session.phase = SessionPhase::Running;
    snapshot.runtime.current_phase = RunPhase::RunningTools;
    snapshot.runtime.latest_tool_detail = Some(LatestToolView {
        tool_id: "call-1".into(),
        group: None,
        row: None,
        label: "Editing root.rs".into(),
        duration_ms: None,
        status: RowStatus::Running,
    });
    harness.set_snapshot(snapshot);
    harness.with_root(|root, _, cx| root.capture_scroll_to(0, cx));
    shoot(&mut harness, "26f-process-summary-clock")?;
    let mut snapshot = harness.snapshot();
    snapshot.transcript = design_transcript();
    harness.set_snapshot(snapshot);

    harness.with_root(|root, _, cx| root.capture_scroll_to(5, cx));
    shoot(&mut harness, "46-tool-group-long-command")?;
    let mut snapshot = harness.snapshot();
    snapshot.transcript = design_transcript();
    harness.set_snapshot(snapshot);

    // A permission gate repeats the command it asks about. A heredoc must stay
    // a preview, or the card swallows the transcript and pushes the decision
    // out of view.
    let script = "export https_proxy=http://127.0.0.1:7890 http_proxy=$https_proxy\n\
cd /tmp/btc/artifacts\n\
python3 - <<'PY'\n\
import json, datetime, statistics\n\
raw=json.load(open('/tmp/mp_1y.json'))['prices']\n\
pairs=sorted((q['time'], q['USD']) for q in raw if q.get('USD') is not None)\n\
print('range', datetime.datetime.fromtimestamp(pairs[0][0], datetime.UTC).date(), '->', datetime.datetime.fromtimestamp(pairs[-1][0], datetime.UTC).date(), 'pts', len(pairs))\n\
byday={}\n\
for t,p in pairs:\n\
    k=int(datetime.datetime.fromtimestamp(t, datetime.UTC).replace(hour=0,minute=0,second=0,microsecond=0).timestamp())\n\
    byday[k]=p\n\
days=sorted(byday.items())\n\
vals=[p for _,p in days]\n\
i_max=vals.index(max(vals)); i_min=vals.index(min(vals))\n\
print('ATH', days[i_max], 'ATL', days[i_min])\n\
cur=vals[-1]\n\
print('cur %.0f drawdown from ATH %.1f%%' % (cur, (cur/max(vals)-1)*100))\n\
def ago(n):\n\
    return days[-1-n][1]\n\
for n,lab in [(1,'24h'),(7,'7d'),(30,'30d'),(90,'90d'),(365,'1y')]:\n\
    print(lab, '%.2f%%' % ((cur/ago(n)-1)*100))\n\
json.dump({'series': days}, open('/tmp/btc/btc.json','w'))\n\
PY";
    let mut rows = design_transcript()
        .iter()
        .take(6)
        .map(|row| (**row).clone())
        .collect::<Vec<_>>();
    if let tact_desktop_core::RowBody::ToolGroup(group) = &mut rows[5].body {
        let mut items = group.items.to_vec();
        let mut tool_id = None;
        if let Some(tact_desktop_core::ToolGroupItem::Tool(tool)) = items.first_mut() {
            tool.arg_summary = Arc::from("python3 - <<'PY' · btc series");
            tool.arg_full = Some(Arc::from(script));
            tool_id = Some(tool.tool_id.clone());
        }
        if let Some(tool_id) = tool_id
            && let Some(tact_desktop_core::ToolGroupItem::Gate(gate)) = items
                .iter_mut()
                .find(|item| matches!(item, tact_desktop_core::ToolGroupItem::Gate(_)))
        {
            gate.origin = tact_desktop_core::GateOrigin::Tool { tool_id };
            gate.prompt = Arc::from("Allow Tact to run this script?");
        }
        items.retain(|item| match item {
            tact_desktop_core::ToolGroupItem::Tool(tool) => tool.name.as_ref() == "shell",
            _ => true,
        });
        group.items = Arc::from(items);
    }
    let tool_group = rows.remove(5);
    let long_gate = tact_desktop_core::TranscriptView::from_rows(vec![Arc::new(tool_group)]);
    let mut snapshot = harness.snapshot();
    snapshot.transcript = long_gate;
    harness.set_snapshot(snapshot);
    harness.with_root(|root, _, cx| root.capture_scroll_to(0, cx));
    shoot(&mut harness, "33c-gate-long-command")?;
    let mut snapshot = harness.snapshot();
    snapshot.transcript = design_transcript();
    harness.set_snapshot(snapshot);

    harness.set_appearance(Appearance::Dark);
    harness.with_root(|root, _, cx| root.capture_scroll_to(0, cx));
    shoot(&mut harness, "27-transcript-dark")?;
    harness.set_appearance(Appearance::Light);

    // Trajectory: the same rows as the transcript, organized by turn and
    // projected onto the overview's three lanes.
    harness.with_root(|root, _, cx| root.capture_set_run_tab(RunTab::Trajectory, cx));
    shoot(&mut harness, "47-trajectory")?;
    let trajectory = harness.with_root(|root, _, cx| {
        let view = tact_desktop_core::TrajectoryView::from_transcript(&root.snapshot().transcript);
        let index = view
            .records()
            .iter()
            .position(|record| record.kind == tact_desktop_core::TrajectoryKind::Tool)
            .unwrap_or(0);
        root.capture_select_trajectory_record(view.records()[index].id, cx);
        view.records().len()
    });
    assert!(trajectory > 0);
    shoot(&mut harness, "48-trajectory-selected")?;
    // The detail panel's Payload tab, so the capture shows the tabbed facets
    // rather than only the landing tab.
    harness.with_root(|root, _, cx| {
        root.capture_set_trajectory_tab(crate::ui::shell::TrajectoryDetailTab::Payload, cx)
    });
    shoot(&mut harness, "48b-trajectory-payload")?;
    harness.with_root(|root, _, cx| {
        root.capture_set_trajectory_tab(crate::ui::shell::TrajectoryDetailTab::Summary, cx)
    });
    // A query with several hits, so the capture shows the ledger narrowing to
    // the matching records, the turn header surviving the filter, and the
    // overview dimming every span it dropped.
    harness.with_root(|root, window, cx| {
        root.capture_set_trajectory_query("cargo", window, cx);
    });
    shoot(&mut harness, "49-trajectory-search")?;
    harness.with_root(|root, window, cx| {
        root.capture_set_trajectory_query("", window, cx);
    });
    harness.set_appearance(Appearance::Dark);
    shoot(&mut harness, "50-trajectory-dark")?;
    harness.set_appearance(Appearance::Light);
    harness.with_root(|root, _, cx| root.capture_set_run_tab(RunTab::Chat, cx));

    for (name, tab) in [
        ("28-inspector-run-populated", InspectorTab::Run),
        ("29-inspector-goals-populated", InspectorTab::Goals),
        ("30-inspector-subagents-populated", InspectorTab::Subagents),
        ("31-inspector-context-populated", InspectorTab::Context),
    ] {
        harness.with_root(|root, _, cx| root.capture_set_inspector(tab, cx));
        shoot(&mut harness, name)?;
    }
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, _, cx| root.capture_open_background(cx));
    shoot(&mut harness, "32-background-populated")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    // The design reviews a gate in the transcript; scroll to the gate row
    // instead of opening a dialog over it.
    harness.with_root(|root, _, cx| {
        let last = root.snapshot().transcript.len().saturating_sub(1);
        root.capture_scroll_to(last, cx);
    });
    shoot(&mut harness, "33-gate")?;
    let mut snapshot = harness.snapshot();
    snapshot.transcript = tact_desktop_core::fixtures::demo_questions();
    snapshot.pending_gates = std::sync::Arc::from([]);
    harness.set_snapshot(snapshot);
    harness.with_root(|root, _, cx| root.capture_scroll_to(0, cx));
    shoot(&mut harness, "33b-questions")?;

    harness.with_root(|root, _, _| root.capture_open_reader(ReaderKind::TasksDag));
    shoot(&mut harness, "38-reader-tasks-dag-populated")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    // P2 readers: ANSI terminal output, web-search results, a multi-file
    // review and an inline Mermaid diagram.
    let mut snapshot = harness.snapshot();
    snapshot.transcript = tact_desktop_core::fixtures::demo_reader_transcript();
    snapshot.pending_gates = std::sync::Arc::from([]);
    harness.set_snapshot(snapshot);
    harness.with_root(|root, _, cx| {
        root.capture_show_transcript_reader(ReaderRef::ToolOutput(RowId::new(1)), cx);
    });
    shoot(&mut harness, "41-reader-ansi")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));
    harness.with_root(|root, _, cx| {
        root.capture_show_transcript_reader(ReaderRef::ToolOutput(RowId::new(2)), cx);
    });
    shoot(&mut harness, "42-reader-search")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));
    harness.with_root(|root, _, cx| {
        root.capture_show_transcript_reader(ReaderRef::Diff(RowId::new(3)), cx);
    });
    shoot(&mut harness, "43-reader-diff-review")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));
    harness.with_root(|root, _, cx| root.capture_scroll_to(2, cx));
    shoot(&mut harness, "44-assistant-mermaid")?;

    let mut snapshot = harness.snapshot();
    snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
    harness.set_snapshot(snapshot);
    harness.with_root(|root, _, cx| {
        root.capture_scroll_to(2, cx);
        root.capture_expand_thinking(RowId::new(2), cx);
    });
    shoot(&mut harness, "45-thinking-expanded")?;

    let help = tact_desktop_core::help_reader(tact_catalog::CatalogLocale::En);
    harness.with_root(|root, _, cx| root.capture_show_reader(help, cx));
    shoot(&mut harness, "40-help")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    let icon = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/app-icon.png");
    harness.with_root(|root, _, cx| root.capture_attach_image(icon, cx));
    shoot(&mut harness, "39b-attachment-thumbnail")?;
    harness.with_root(|root, _, cx| root.capture_clear_attachments(cx));

    harness.simulate_keystrokes("tab");
    shoot(&mut harness, "39-focus-visible")?;

    let mut snapshot = harness.snapshot();
    snapshot.text_scale = 1.5;
    harness.set_snapshot(snapshot);
    shoot(&mut harness, "24-text-scale-150")?;

    let mut snapshot = harness.snapshot();
    snapshot.text_scale = 1.0;
    snapshot.language = tact_catalog::CatalogLocale::Zh;
    harness.set_snapshot(snapshot);
    shoot(&mut harness, "25-chinese")?;

    harness.finish();

    // A separate off-screen window is the only reliable way to capture a
    // different viewport size; resizing the first window does not reflow the
    // rendered scene in the visual test platform.
    let mut narrow = Harness::open_sized(Appearance::Light, 1.0, NARROW_WIDTH, NARROW_HEIGHT)?;
    shoot(&mut narrow, "23-narrow-window")?;
    narrow.finish();

    // A tall off-screen window renders the whole transcript without relying on
    // the virtualized list's scroll position.
    let mut tall = Harness::open_sized(Appearance::Light, 1.0, 1280.0, 2600.0)?;
    let mut snapshot = tall.snapshot();
    snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
    snapshot.inspector = tact_desktop_core::fixtures::demo_inspector();
    snapshot.failure = None;
    snapshot.session.phase = SessionPhase::Running;
    tall.set_snapshot(snapshot);
    shoot(&mut tall, "34-transcript-bottom")?;
    tall.with_root(|root, _, cx| root.capture_scroll_to(0, cx));
    shoot(&mut tall, "35-transcript-top")?;
    tall.with_root(|root, _, cx| root.capture_scroll_to(7, cx));
    shoot(&mut tall, "36-transcript-middle")?;
    tall.set_appearance(Appearance::Dark);
    shoot(&mut tall, "37-transcript-middle-dark")?;
    tall.finish();

    Ok(shots)
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.controller.shutdown_blocking();
        let _ = std::fs::remove_dir_all(&self.data_dir);
        // Release the root entity and the window before the app cell drops so
        // the visual-test leak detector stays quiet on error paths.
        let _ = self.root.take();
        let window = self.window;
        let _ = self
            .cx
            .update_window(window, |_, window, _| window.remove_window());
        self.cx.run_until_parked();
    }
}
