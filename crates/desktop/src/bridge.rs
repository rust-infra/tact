//! The TSX bridge: one class that starts the desktop controller, pushes its
//! snapshots to JS, and turns JS commands into desktop intents.

use std::path::PathBuf;

use nsh_core::element::{Callback, Instance};
use nsh_core::prelude::*;
use tact_catalog::SettingId;
use tact_desktop_core::{
    DesktopIntent, GateResponse, InspectorTab, PermissionModeView, ReaderRef, SettingValue,
    Submission, VoiceCommand,
};

use crate::app;
use crate::json::{ReaderJson, ShellSnapshot, UiCommand};

/// The desktop controller, as TSX sees it.
#[class]
pub struct Controller;

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}

#[class]
impl Controller {
    /// Starts the controller and returns its handle.
    pub fn new() -> Self {
        app::start().expect("Tact Desktop starts");
        Self
    }

    /// The current snapshot, as JSON.
    pub fn snapshot(&self) -> String {
        snapshot_json()
    }

    /// Sends a command (`UiCommand` JSON).
    pub fn dispatch(&self, command: String) {
        dispatch(&command);
    }

    /// A reader's content, as JSON.
    pub fn reader(&self, kind: String, id: Option<u64>) -> String {
        reader_json(&kind, id)
    }

    /// The current trajectory ledger, as JSON.
    pub fn trajectory(&self) -> String {
        trajectory_json()
    }

    /// One trajectory record's detail, as JSON.
    pub fn trajectory_record(&self, index: usize) -> String {
        let snapshot = app::controller().snapshot();
        let view = tact_desktop_core::TrajectoryView::from_transcript(&snapshot.transcript);
        view.records()
            .get(index)
            .map(|record| {
                serde_json::to_string(&crate::json::TrajectoryDetailJson::from(record))
                    .unwrap_or_default()
            })
            .unwrap_or_else(|| "{}".to_owned())
    }

    /// Writes a picked image to a temporary file; the path to submit.
    pub fn attachment(&self, name: String, data_base64: String) -> String {
        use base64::Engine as _;
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data_base64) else {
            return String::new();
        };
        let dir = std::env::temp_dir()
            .join("tact-attachments")
            .join(std::process::id().to_string());
        if std::fs::create_dir_all(&dir).is_err() {
            return String::new();
        }
        let safe = name.rsplit(['/', '\\']).next().unwrap_or("image");
        let path = dir.join(safe);
        if std::fs::write(&path, bytes).is_err() {
            return String::new();
        }
        path.display().to_string()
    }

    /// Pushes a message on every commit, reader and toast until the app ends.
    pub async fn subscribe(this: Instance<Self>, callback: Callback<String>) {
        let _ = this;
        let events = app::events();
        let mut last_revision = 0_u64;
        while let Ok(event) = events.recv().await {
            match event {
                tact_desktop_core::DesktopEvent::Committed(commit) => {
                    if commit.revision == last_revision {
                        continue;
                    }
                    last_revision = commit.revision;
                    callback.post(snapshot_json());
                }
                tact_desktop_core::DesktopEvent::Effect(
                    tact_desktop_core::DesktopEffect::OpenReader(request),
                ) => {
                    callback.post(
                        serde_json::json!({
                            "type": "reader",
                            "title": request.title.to_string(),
                            "markdown": request.markdown.to_string(),
                        })
                        .to_string(),
                    );
                }
                tact_desktop_core::DesktopEvent::Effect(
                    tact_desktop_core::DesktopEffect::Notify { title, body },
                ) => {
                    callback.post(
                        serde_json::json!({
                            "type": "toast",
                            "title": title,
                            "body": body,
                        })
                        .to_string(),
                    );
                }
                tact_desktop_core::DesktopEvent::Effect(
                    tact_desktop_core::DesktopEffect::CopyText(text),
                ) => set_clipboard(text),
                tact_desktop_core::DesktopEvent::Effect(
                    tact_desktop_core::DesktopEffect::OpenExternal(url),
                ) => {
                    drop(open_url(url.to_string()));
                }
                tact_desktop_core::DesktopEvent::Effect(
                    tact_desktop_core::DesktopEffect::OpenPath(path),
                ) => {
                    drop(open_url(format!("file://{}", path.display())));
                }
                tact_desktop_core::DesktopEvent::Effect(
                    tact_desktop_core::DesktopEffect::WindowCloseRequested,
                ) => nsh_core::element::desktop::close_window(),
                tact_desktop_core::DesktopEvent::Voice(event) => {
                    let (event, text) = match event {
                        tact_desktop_core::VoiceEvent::RecordingStarted => {
                            ("recording_started", String::new())
                        }
                        tact_desktop_core::VoiceEvent::RecordingStopped => {
                            ("recording_stopped", String::new())
                        }
                        tact_desktop_core::VoiceEvent::Transcribing => {
                            ("transcribing", String::new())
                        }
                        tact_desktop_core::VoiceEvent::Transcript(text) => ("transcript", text),
                        tact_desktop_core::VoiceEvent::Error(message) => ("error", message),
                        tact_desktop_core::VoiceEvent::Cancelled => ("cancelled", String::new()),
                    };
                    callback.post(
                        serde_json::json!({ "type": "voice", "event": event, "text": text })
                            .to_string(),
                    );
                }
                tact_desktop_core::DesktopEvent::Fatal(failure) => {
                    callback.post(
                        serde_json::json!({
                            "type": "toast",
                            "title": "Tact stopped",
                            "body": failure.message,
                        })
                        .to_string(),
                    );
                }
                _ => {}
            }
        }
    }

    /// Stops the controller.
    pub fn shutdown(&self) {
        app::shutdown();
    }
}

fn snapshot_json() -> String {
    let snapshot = app::controller().snapshot();
    serde_json::to_string(&ShellSnapshot::from(&*snapshot)).expect("the shell snapshot serializes")
}

fn trajectory_json() -> String {
    let snapshot = app::controller().snapshot();
    let view = tact_desktop_core::TrajectoryView::from_transcript(&snapshot.transcript);
    serde_json::to_string(&crate::json::TrajectoryJson::from(&view))
        .expect("the trajectory view serializes")
}

fn reader_json(kind: &str, id: Option<u64>) -> String {
    let snapshot = app::controller().snapshot();
    let request = match kind {
        "tool_output" => {
            let Some(id) = id else {
                return "{}".to_owned();
            };
            let mut found = None;
            for row in snapshot.transcript.iter() {
                let tact_desktop_core::RowBody::ToolGroup(group) = &row.body else {
                    continue;
                };
                for item in group.items.iter() {
                    if let tact_desktop_core::ToolGroupItem::Tool(tool) = item
                        && tool.id.value() == id
                    {
                        found = Some(ReaderJson {
                            title: tool.name.to_string(),
                            markdown: tool
                                .output
                                .detail
                                .clone()
                                .unwrap_or_else(|| tool.output.preview.clone())
                                .to_string(),
                        });
                    }
                }
            }
            found
        }
        "diff" => {
            let Some(id) = id else {
                return "{}".to_owned();
            };
            snapshot.transcript.iter().find_map(|row| {
                if row.id.value() != id {
                    return None;
                }
                let tact_desktop_core::RowBody::Diff(diff) = &row.body else {
                    return None;
                };
                Some(ReaderJson {
                    title: diff.path.as_deref().unwrap_or("Diff").to_owned(),
                    markdown: diff.patch.to_string(),
                })
            })
        }
        "thinking" => {
            let Some(id) = id else {
                return "{}".to_owned();
            };
            snapshot.transcript.iter().find_map(|row| {
                if row.id.value() != id {
                    return None;
                }
                let tact_desktop_core::RowBody::Thinking(thinking) = &row.body else {
                    return None;
                };
                Some(ReaderJson {
                    title: "Thinking".to_owned(),
                    markdown: thinking.markdown.to_string(),
                })
            })
        }
        "subagent" => {
            let child = id.map(|id| id.to_string()).unwrap_or_default();
            snapshot.inspector.subagents.iter().find_map(|agent| {
                if agent.child_id.as_ref() != child {
                    return None;
                }
                Some(ReaderJson {
                    title: child.clone(),
                    markdown: agent.summary.to_string(),
                })
            })
        }
        "background" => {
            let task_id = id.map(|id| id.to_string()).unwrap_or_default();
            snapshot.inspector.background.iter().find_map(|task| {
                if task.id.as_ref() != task_id {
                    return None;
                }
                Some(ReaderJson {
                    title: task.command.to_string(),
                    markdown: task.output_tail.to_string(),
                })
            })
        }
        "help" => {
            let request = tact_desktop_core::help_reader(snapshot.language);
            Some(ReaderJson {
                title: request.title.to_string(),
                markdown: request.markdown.to_string(),
            })
        }
        _ => None,
    };
    match request {
        Some(request) => serde_json::to_string(&request).expect("a reader serializes"),
        None => "{}".to_owned(),
    }
}

fn reader_ref(kind: &str, id: Option<u64>, key: Option<String>) -> Option<ReaderRef> {
    let row = id.map(tact_desktop_core::RowId::new);
    Some(match kind {
        "tool_output" => ReaderRef::ToolOutput(row?),
        "diff" => ReaderRef::Diff(row?),
        "thinking" => ReaderRef::Thinking(row?),
        "system_prompt" => ReaderRef::SystemPrompt,
        "stats" => ReaderRef::Stats,
        "tasks_dag" => ReaderRef::TasksDag,
        "history" => ReaderRef::History,
        "help" => ReaderRef::Help,
        "balance" => ReaderRef::Balance,
        "subagent" => ReaderRef::Subagent(key?),
        "background" => ReaderRef::Background(key?),
        _ => return None,
    })
}

fn dispatch(json: &str) {
    let Ok(command) = serde_json::from_str::<UiCommand>(json) else {
        return;
    };
    let controller = app::controller();
    let intent = match command {
        UiCommand::NewSession => DesktopIntent::NewSession,
        UiCommand::ResumeSession { session_id } => DesktopIntent::ResumeSession {
            session_id: tact_desktop_core::SessionId(session_id),
        },
        UiCommand::Submit { text, attachments } => DesktopIntent::Submit(Submission {
            text,
            attachments: attachments.into_iter().map(PathBuf::from).collect(),
            model: None,
            permission_mode: None,
        }),
        UiCommand::Queue { text } => DesktopIntent::Queue(Submission {
            text,
            attachments: Vec::new(),
            model: None,
            permission_mode: None,
        }),
        UiCommand::CancelRun => DesktopIntent::CancelRun,
        UiCommand::RetryProvider => DesktopIntent::RetryProvider,
        UiCommand::SetInspector { tab } => DesktopIntent::SetInspector(match tab.as_str() {
            "goals" => InspectorTab::Goals,
            "subagents" => InspectorTab::Subagents,
            "background" => InspectorTab::Background,
            "context" => InspectorTab::Context,
            _ => InspectorTab::Run,
        }),
        UiCommand::ToggleGroup { id } => DesktopIntent::ToggleGroup {
            id: tact_desktop_core::RowId::new(id),
        },
        UiCommand::ToggleTool { group, tool } => DesktopIntent::ToggleTool {
            group: tact_desktop_core::RowId::new(group),
            tool: tact_desktop_core::RowId::new(tool),
        },
        UiCommand::RespondGate {
            session_epoch,
            request_id,
            selected,
            single,
        } => DesktopIntent::RespondGate {
            session_epoch: tact_desktop_core::SessionEpoch(session_epoch),
            request_id,
            response: if let Some(index) = single {
                GateResponse::Select(Some(index))
            } else {
                GateResponse::MultiSelect(Some(selected))
            },
        },
        UiCommand::SetGateDraft {
            session_epoch,
            request_id,
            selected,
        } => DesktopIntent::SetGateDraft {
            session_epoch: tact_desktop_core::SessionEpoch(session_epoch),
            request_id,
            selected,
        },
        UiCommand::SetSessionFilter { filter } => {
            DesktopIntent::SetSessionFilter(match filter.as_str() {
                "archived" => tact_desktop_core::ArchiveFilter::Archived,
                "all" => tact_desktop_core::ArchiveFilter::All,
                _ => tact_desktop_core::ArchiveFilter::Active,
            })
        }
        UiCommand::SetSessionQuery { query } => DesktopIntent::SetSessionQuery(query),
        UiCommand::RenameSession { session_id, title } => DesktopIntent::RenameSession {
            session_id: tact_desktop_core::SessionId(session_id),
            title,
        },
        UiCommand::SetSessionArchived {
            session_id,
            archived,
        } => DesktopIntent::SetSessionArchived {
            session_id: tact_desktop_core::SessionId(session_id),
            archived,
        },
        UiCommand::SendQueuedNow { index } => DesktopIntent::SendQueuedNow { index },
        UiCommand::RemoveQueued { index } => DesktopIntent::RemoveQueued { index },
        UiCommand::MoveQueued { from, to } => DesktopIntent::MoveQueued { from, to },
        UiCommand::EditQueued { index, text } => DesktopIntent::EditQueued { index, text },
        UiCommand::CancelSubagent { child_id } => DesktopIntent::CancelSubagent { child_id },
        UiCommand::CancelBackgroundTask { task_id } => {
            DesktopIntent::CancelBackgroundTask { task_id }
        }
        UiCommand::InvokeCommand { path, tokens } => {
            let Some(descriptor) = tact_catalog::builtins()
                .resolve_path(&path.iter().map(String::as_str).collect::<Vec<_>>())
            else {
                return;
            };
            DesktopIntent::InvokeCommand {
                id: descriptor.id.clone(),
                args: tact_catalog::CommandArguments { tokens },
            }
        }
        UiCommand::OpenReader {
            reader_kind,
            id,
            key,
        } => {
            let Some(reference) = reader_ref(&reader_kind, id, key) else {
                return;
            };
            DesktopIntent::OpenReader(reference)
        }
        UiCommand::OpenProject { root } => DesktopIntent::OpenProject {
            root: PathBuf::from(root),
        },
        UiCommand::SetSetting { id, value } => {
            let Some(setting) = setting_id(&id) else {
                return;
            };
            DesktopIntent::SetSetting {
                id: setting,
                value: match value {
                    crate::json::SettingValueJson::Bool { value } => SettingValue::Bool(value),
                    crate::json::SettingValueJson::Text { value } => SettingValue::Text(value),
                    crate::json::SettingValueJson::Choice { value } => SettingValue::Choice(value),
                },
            }
        }
        UiCommand::SetSkillEnabled { name, enabled } => {
            DesktopIntent::SetSkillEnabled { name, enabled }
        }
        UiCommand::SetTailFollowing { following } => DesktopIntent::SetTailFollowing(following),
        UiCommand::BackToBottom => DesktopIntent::BackToBottom,
        UiCommand::Voice { command } => DesktopIntent::Voice(match command.as_str() {
            "stop" => VoiceCommand::Stop,
            "cancel" => VoiceCommand::Cancel,
            _ => VoiceCommand::Start,
        }),
        UiCommand::Shutdown => DesktopIntent::Shutdown,
    };
    let _ = controller.dispatch(intent);
}

fn setting_id(id: &str) -> Option<SettingId> {
    SettingId::ALL
        .iter()
        .copied()
        .find(|setting| setting.as_str() == id)
}

/// The voice permission mode, for TSX to read (unused placeholder).
pub fn permission_label(mode: PermissionModeView) -> &'static str {
    match mode {
        PermissionModeView::Default => "default",
        PermissionModeView::Plan => "plan",
        PermissionModeView::Auto => "auto",
    }
}
