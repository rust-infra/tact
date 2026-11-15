//! End-to-end ACP session test.
//!
//! Drives the real `tact-acp` binary over stdio against a temp config, a temp
//! session cwd, and a local OpenAI-compatible SSE server. Nothing inside the
//! adapter is mocked: the assertions are the ACP traffic T3 would see.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use agent_client_protocol::schema::{MaybeUndefined, ProtocolVersion};
use agent_client_protocol::v2::{ConnectionTo, Responder, schema};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, Client, Error};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc::{self, UnboundedReceiver},
};

const STEP_TIMEOUT: Duration = Duration::from_secs(20);
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(120);
const QUIET_AFTER_IDLE: Duration = Duration::from_millis(300);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_session_end_to_end() {
    let fake = FakeModel::start().await;
    let home = tempfile::tempdir().expect("temp home");
    let session_cwd = tempfile::tempdir().expect("temp session cwd");
    let skill_dir = home.path().join("skills");
    write_skill(&skill_dir);
    let config = write_config(home.path(), &fake.base_url, &skill_dir);
    let mcp = StdioMcpServer::install(session_cwd.path());

    let agent = AcpAgent::new(
        AcpAgentConfig::new(env!("CARGO_BIN_EXE_tact-acp"))
            .arg("--config")
            .arg(config.display().to_string())
            .env(
                "HOME",
                home.path().to_str().expect("temp home is valid UTF-8"),
            )
            .env("TACT_API_KEY", "test"),
    );

    let result = tokio::time::timeout(
        SCENARIO_TIMEOUT,
        run_client(agent, fake, session_cwd.path().to_path_buf(), mcp),
    )
    .await;

    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("ACP session failed: {error:?}"),
        Err(_) => panic!("ACP session did not finish within {SCENARIO_TIMEOUT:?}"),
    }
}

/// A minimal MCP stdio server, written as a shell script so the test needs no
/// Python toolchain. It answers `initialize` and `tools/list` with one tool and
/// echoes a fixed string for any `tools/call`.
struct StdioMcpServer {
    path: PathBuf,
}

impl StdioMcpServer {
    fn install(dir: &Path) -> Self {
        let path = dir.join("fake-mcp-server.py");
        std::fs::write(
            &path,
            r#"#!/usr/bin/env python3
import json, sys

def reply(message_id, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message_id, "result": result}) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    message = json.loads(line)
    method = message.get("method")
    if method == "initialize":
        reply(message["id"], {
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fake-mcp", "version": "1.0.0"},
        })
    elif method == "tools/list":
        reply(message["id"], {"tools": [{
            "name": "bridge_ping",
            "description": "Returns the bridge marker",
            "inputSchema": {"type": "object", "properties": {}},
        }]})
    elif method == "tools/call":
        reply(message["id"], {"content": [{"type": "text", "text": "bridge-tool-ran"}]})
    elif "id" in message:
        reply(message["id"], {})
"#,
        )
        .expect("write fake MCP server");
        let mut permissions = std::fs::metadata(&path)
            .expect("fake MCP server metadata")
            .permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).expect("make fake MCP server executable");
        Self { path }
    }
}

fn write_config(home: &Path, base_url: &str, skill_dir: &Path) -> PathBuf {
    let dir = home.join(".tact");
    std::fs::create_dir_all(&dir).expect("create temp config dir");
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!(
            r#"
[llm]
provider = "fake"

[llm.providers.fake]
api_key = "test"
base_url = "{base_url}"
model = "fake-a"
models = ["fake-a", "fake-b"]
protocol = "chat_completions"

[permission]
mode = "default"

[agent]
skill_dirs = ["{}"]
"#,
            skill_dir.display()
        ),
    )
    .expect("write temp config");
    config
}

/// One skill on disk, so the adapter has something to advertise and run.
fn write_skill(skill_dir: &Path) {
    let dir = skill_dir.join("greet");
    std::fs::create_dir_all(&dir).expect("create temp skill dir");
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: greet\ndescription: Greet the reader\n---\nSay hello to $ARGUMENTS.\n",
    )
    .expect("write temp skill");
}

async fn run_client(
    agent: AcpAgent,
    fake: FakeModel,
    cwd: PathBuf,
    mcp: StdioMcpServer,
) -> Result<(), Error> {
    let client_state = Arc::new(ClientState::default());
    let (update_tx, update_rx) = mpsc::unbounded_channel();
    let notification_state = client_state.clone();
    let permission_state = client_state.clone();
    let elicitation_state = client_state.clone();

    Client
        .v2()
        .name("acp-session-test")
        .on_receive_notification(
            async move |notification: schema::UpdateSessionNotification,
                        _connection: ConnectionTo<Agent>| {
                notification_state
                    .log
                    .lock()
                    .expect("update log lock")
                    .push(notification.update.clone());
                let _ = update_tx.send(notification);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: schema::RequestPermissionRequest,
                        responder: Responder<schema::RequestPermissionResponse>,
                        _connection: ConnectionTo<Agent>| {
                permission_state
                    .permissions
                    .lock()
                    .expect("permissions lock")
                    .push(
                        request
                            .options
                            .iter()
                            .map(|option| option.kind.clone())
                            .collect(),
                    );
                let option = request
                    .options
                    .iter()
                    .find(|option| option.kind == schema::PermissionOptionKind::AllowOnce)
                    .or_else(|| request.options.first());
                let outcome = match option {
                    Some(option) => schema::RequestPermissionOutcome::Selected(
                        schema::SelectedPermissionOutcome::new(option.option_id.clone()),
                    ),
                    None => schema::CancelledPermissionOutcome::new().into(),
                };
                responder.respond(schema::RequestPermissionResponse::new(outcome))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: schema::CreateElicitationRequest,
                        responder: Responder<schema::CreateElicitationResponse>,
                        _connection: ConnectionTo<Agent>| {
                let record = ElicitationRecord::from_request(&request);
                elicitation_state
                    .elicitations
                    .lock()
                    .expect("elicitations lock")
                    .push(record);
                let behavior = elicitation_state
                    .behavior
                    .lock()
                    .expect("behavior lock")
                    .clone();
                match behavior {
                    ElicitationBehavior::AcceptOne(value) => responder.respond(accept_response(
                        schema::ElicitationContentValue::String(value),
                    )),
                    ElicitationBehavior::AcceptMany(values) => responder.respond(accept_response(
                        schema::ElicitationContentValue::StringArray(values),
                    )),
                    ElicitationBehavior::Block => {
                        // Hold the dialog without blocking the dispatch loop.
                        // The peer's `$/cancel_request` is the observable we
                        // assert on after `session/cancel`.
                        let cancellation = responder.cancellation();
                        let state = elicitation_state.clone();
                        tokio::spawn(async move {
                            cancellation.cancelled().await;
                            state.cancel_seen.store(true, Ordering::SeqCst);
                            drop(responder);
                        });
                        Ok(())
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, async move |connection| {
            run_scenario(connection, update_rx, client_state, fake, cwd, mcp).await
        })
        .await
}

fn accept_response(value: schema::ElicitationContentValue) -> schema::CreateElicitationResponse {
    let mut content = BTreeMap::new();
    content.insert("selections".to_string(), value);
    schema::CreateElicitationResponse::new(schema::ElicitationAction::Accept(
        schema::ElicitationAcceptAction::new().content(content),
    ))
}

#[allow(clippy::too_many_arguments)]
async fn run_scenario(
    connection: ConnectionTo<Agent>,
    mut updates: UnboundedReceiver<schema::UpdateSessionNotification>,
    client_state: Arc<ClientState>,
    fake: FakeModel,
    cwd: PathBuf,
    mcp: StdioMcpServer,
) -> Result<(), Error> {
    // 1. initialize: v2 session capability, and form elicitation available.
    let initialize = connection
        .send_request(
            schema::InitializeRequest::new(
                ProtocolVersion::V2,
                schema::Implementation::new("acp-session-test", "1.0.0"),
            )
            .capabilities(
                schema::ClientCapabilities::new().elicitation(
                    schema::ElicitationCapabilities::new()
                        .form(schema::ElicitationFormCapabilities::new()),
                ),
            ),
        )
        .block_task()
        .await?;
    assert_eq!(initialize.info.name, "tact-acp");
    let session_capabilities = initialize
        .capabilities
        .session
        .as_ref()
        .expect("initialize must advertise capabilities.session");
    let advertised_mcp = session_capabilities
        .mcp
        .as_ref()
        .expect("an accepted client MCP server must be advertised");
    assert!(
        advertised_mcp.stdio.is_some() && advertised_mcp.http.is_some(),
        "both MCP transports this adapter accepts must be advertised"
    );
    assert!(
        session_capabilities
            .prompt
            .as_ref()
            .is_some_and(|prompt| prompt.image.is_some()),
        "image prompts are accepted by writing them into the workspace"
    );
    assert!(
        session_capabilities.delete.is_some(),
        "session/delete is implemented and must be advertised"
    );

    // 2. session/new with a stdio MCP server must succeed, advertise the model
    //    option, and actually connect the server.
    let request =
        schema::NewSessionRequest::new(cwd.clone()).mcp_servers(vec![schema::McpServer::Stdio(
            schema::McpServerStdio::new("t3-code", schema::AbsolutePath::new(mcp.path.clone())),
        )]);
    let setup = connection
        .build_session_from(request)
        .start_session()
        .block_task()
        .await?;
    let session = setup.session;
    let session_id = session.session_id().clone();
    assert!(
        !session_id.0.is_empty(),
        "session/new must return a session id"
    );
    let model = model_option(&setup.response.config_options);
    assert_eq!(model_current(model), "fake-a");
    assert_eq!(model_values(model), vec!["fake-a", "fake-b"]);
    let permission = config_option(&setup.response.config_options, "permission_mode");
    assert_eq!(
        permission.category,
        Some(schema::SessionConfigOptionCategory::Mode),
        "T3 only makes its plan toggle settable through a mode-category select"
    );
    assert_eq!(model_current(permission), "default");
    assert_eq!(
        model_values(permission),
        vec!["default", "plan", "auto"],
        "every permission mode must be offered"
    );
    let thought = config_option(&setup.response.config_options, "thought_level");
    assert_eq!(
        thought.category,
        Some(schema::SessionConfigOptionCategory::ThoughtLevel)
    );
    assert!(
        !model_values(thought).is_empty(),
        "a thought-level selector needs at least one tier"
    );
    let advertised = setup
        .response
        .available_commands
        .iter()
        .map(|command| command.name().to_string())
        .collect::<Vec<_>>();
    assert!(
        advertised.iter().any(|name| name == "compact"),
        "the commands this adapter handles must be advertised: {advertised:?}"
    );
    assert!(
        advertised.iter().any(|name| name == "$greet"),
        "a skill must be advertised with the client's `$name` spelling: {advertised:?}"
    );

    // 3. Idle model switch: the response is the source of truth and the
    //    runtime confirmation arrives as config_option_update.
    drain(&mut updates);
    let switched = session
        .set_config_option("model", session_value("fake-b"))
        .block_task()
        .await?;
    let model = model_option(&switched.config_options);
    assert_eq!(model_current(model), "fake-b");
    let confirmation = wait_for_model_update(&mut updates, &session_id, "fake-b").await;
    let model = model_option(&confirmation.config_options);
    assert_eq!(model_current(model), "fake-b");
    assert_eq!(model_values(model), vec!["fake-a", "fake-b"]);

    // 4. Prompt: accepted immediately, user message echoed, text streamed,
    //    exactly one end-turn idle. The next request body carries the switch.
    client_state.take_updates();
    let requests_before = fake.request_count();
    let accepted = session
        .send_prompt("hello from the test")
        .block_task()
        .await?;
    assert!(
        !accepted.message_id.0.is_empty(),
        "prompt response must carry a messageId"
    );
    let (idle, config_updates) = wait_for_idle(&mut updates, &session_id).await;
    assert_eq!(idle.stop_reason, Some(schema::StopReason::EndTurn));
    assert!(
        config_updates.is_empty(),
        "no config update should arrive during a plain turn"
    );
    assert!(
        fake.request_count() > requests_before,
        "the prompt must reach the model"
    );
    for index in requests_before..fake.request_count() {
        assert_eq!(
            fake.model_at(index).as_deref(),
            Some("fake-b"),
            "request {index} must carry the switched model"
        );
    }
    let observed = client_state.take_updates();
    let echo = observed
        .iter()
        .find_map(|update| match update {
            schema::SessionUpdate::UserMessage(message)
                if message.message_id == accepted.message_id =>
            {
                Some(message)
            }
            _ => None,
        })
        .expect("the user message must be echoed with the response messageId");
    assert!(matches!(
        &echo.content,
        MaybeUndefined::Value(blocks) if !blocks.is_empty()
    ));
    let chunk = observed
        .iter()
        .find_map(|update| match update {
            schema::SessionUpdate::AgentMessageChunk(chunk) => Some(chunk),
            _ => None,
        })
        .expect("the fake model text must stream as agent_message_chunk");
    assert!(
        !chunk.message_id.0.is_empty(),
        "agent_message_chunk needs a non-empty messageId"
    );
    assert_no_state_after_idle(&mut updates, &session_id).await;

    // 5. Permission: write_file asks through ACP, allow once, the tool writes
    //    into the session cwd, and the turn reaches idle.
    drain(&mut updates);
    client_state.take_updates();
    session
        .send_prompt("please ACP_TEST_WRITE a file")
        .block_task()
        .await?;
    let (idle, _) = wait_for_idle(&mut updates, &session_id).await;
    assert_eq!(idle.stop_reason, Some(schema::StopReason::EndTurn));
    {
        let permissions = client_state.permissions.lock().expect("permissions lock");
        let write_permission = permissions
            .last()
            .expect("write_file must ask for permission");
        assert!(write_permission.contains(&schema::PermissionOptionKind::AllowOnce));
        assert!(write_permission.contains(&schema::PermissionOptionKind::RejectOnce));
        assert!(write_permission.contains(&schema::PermissionOptionKind::AllowAlways));
    }
    assert!(
        cwd.join("created-by-acp.txt").exists(),
        "the allowed write_file must create the file in the session cwd"
    );
    assert_no_state_after_idle(&mut updates, &session_id).await;

    // 5b. The client's MCP server is connected for this session: its tool is
    //     offered to the model and a call round-trips through the real stdio
    //     fixture.
    drain(&mut updates);
    client_state.take_updates();
    session
        .send_prompt("please ACP_TEST_MCP")
        .block_task()
        .await?;
    let (idle, _) = wait_for_idle(&mut updates, &session_id).await;
    assert_eq!(idle.stop_reason, Some(schema::StopReason::EndTurn));
    let last = fake.last_request();
    assert!(
        last["tools"].to_string().contains("bridge_ping"),
        "the client MCP server's tool must be offered to the model: {last}"
    );
    assert!(
        tool_results(&last).contains("bridge-tool-ran"),
        "the MCP tool result must reach the next model call: {last}"
    );
    assert_no_state_after_idle(&mut updates, &session_id).await;

    // 6. Multi-select question: a form elicitation carries the option values,
    //    two indices are accepted, and the tool result names those options.
    drain(&mut updates);
    client_state.take_updates();
    *client_state.behavior.lock().expect("behavior lock") =
        ElicitationBehavior::AcceptMany(vec!["0".into(), "2".into()]);
    session
        .send_prompt("please ACP_TEST_QUESTION")
        .block_task()
        .await?;
    let (idle, _) = wait_for_idle(&mut updates, &session_id).await;
    assert_eq!(idle.stop_reason, Some(schema::StopReason::EndTurn));
    {
        let elicitations = client_state.elicitations.lock().expect("elicitations lock");
        let question = elicitations
            .last()
            .expect("multi-select question must reach the client");
        assert!(question.multi, "expected a multi-select schema");
        assert_eq!(
            question.values,
            vec!["0".to_string(), "1".to_string(), "2".to_string()]
        );
        assert_eq!(question.titles, vec!["first", "second", "third"]);
    }
    let last = fake.last_request();
    assert!(
        tool_results(&last).contains("first, third"),
        "the accepted indices must reach ask_user: {last}"
    );
    assert_no_state_after_idle(&mut updates, &session_id).await;

    // Single-select question: string schema and a string response.
    drain(&mut updates);
    client_state.take_updates();
    *client_state.behavior.lock().expect("behavior lock") =
        ElicitationBehavior::AcceptOne("1".into());
    session
        .send_prompt("please ACP_TEST_SINGLE")
        .block_task()
        .await?;
    let (idle, _) = wait_for_idle(&mut updates, &session_id).await;
    assert_eq!(idle.stop_reason, Some(schema::StopReason::EndTurn));
    {
        let elicitations = client_state.elicitations.lock().expect("elicitations lock");
        let question = elicitations
            .last()
            .expect("single-select question must reach the client");
        assert!(!question.multi, "expected a single-select schema");
    }
    let last = fake.last_request();
    assert!(
        tool_results(&last).contains("second"),
        "the accepted index must reach ask_user: {last}"
    );
    assert_no_state_after_idle(&mut updates, &session_id).await;

    // 5b2. An image prompt is accepted: it becomes a workspace file the model
    //      reads by path, so no binary ever crosses the model protocol.
    drain(&mut updates);
    client_state.take_updates();
    session
        .send_prompt_blocks(vec![
            schema::ContentBlock::Text(schema::TextContent::new("what is in this image?")),
            schema::ContentBlock::Image(schema::ImageContent::new("aGVsbG8=", "image/png")),
        ])
        .block_task()
        .await?;
    let (idle, _) = wait_for_idle(&mut updates, &session_id).await;
    assert_eq!(idle.stop_reason, Some(schema::StopReason::EndTurn));
    let last = fake.last_request();
    let prompt = last.to_string();
    assert!(
        prompt.contains(".tact-acp-images/prompt-1.png"),
        "the image must reach the model as a workspace path: {last}"
    );
    assert!(
        cwd.join(".tact-acp-images/prompt-1.png").exists(),
        "the prompt image must be written into the session cwd"
    );
    assert_no_state_after_idle(&mut updates, &session_id).await;

    // 5c. Live state: token usage feeds the client's context meter, and
    //     Tact's persistent task list is published as an ACP plan.
    drain(&mut updates);
    client_state.take_updates();
    session
        .send_prompt("please ACP_TEST_TASK")
        .block_task()
        .await?;
    let mut saw_plan = false;
    loop {
        match next_update(&mut updates, &session_id).await {
            schema::SessionUpdate::PlanUpdate(plan) => {
                let schema::PlanUpdateContent::Items(items) = &plan.plan else {
                    panic!("the task list must publish as plan items");
                };
                assert!(
                    items
                        .entries
                        .iter()
                        .any(|entry| entry.content == "write the ACP test"),
                    "the created task must appear in the plan: {items:?}"
                );
                saw_plan = true;
            }
            schema::SessionUpdate::StateUpdate(schema::StateUpdate::Idle(_)) => break,
            _ => {}
        }
    }
    assert!(saw_plan, "creating a task must publish an ACP plan");
    let observed = client_state.take_updates();
    let usage = observed
        .iter()
        .find_map(|update| match update {
            schema::SessionUpdate::UsageUpdate(usage) => Some(usage),
            _ => None,
        })
        .expect("a model call must report token usage");
    assert!(usage.used > 0, "usage must carry the tokens in context");
    assert_no_state_after_idle(&mut updates, &session_id).await;

    // 6a. Slash commands: `/stats` is answered by the host as a read-out (not a
    //     model turn), and `/model <id>` is the same option the config surface
    //     exposes. An unknown `/name` is forwarded verbatim.
    drain(&mut updates);
    client_state.take_updates();
    let requests_before = fake.request_count();
    session.send_prompt("/stats").block_task().await?;
    let notice = loop {
        if let schema::SessionUpdate::Notice(notice) = next_update(&mut updates, &session_id).await
        {
            break notice;
        }
    };
    assert!(
        notice.title.contains("Statistics") || notice.title.contains("Tact"),
        "the host's /stats answer must arrive as a notice: {notice:?}"
    );
    assert_eq!(
        fake.request_count(),
        requests_before,
        "/stats must not start a model turn"
    );

    let switched = session.send_prompt("/model fake-b").block_task().await?;
    assert!(!switched.message_id.0.is_empty());
    let switched_update = wait_for_model_update(&mut updates, &session_id, "fake-b").await;
    assert_eq!(
        model_current(model_option(&switched_update.config_options)),
        "fake-b"
    );
    assert_eq!(
        fake.request_count(),
        requests_before,
        "/model must apply the option without a model turn"
    );
    let back = session.send_prompt("/model fake-a").block_task().await?;
    assert!(!back.message_id.0.is_empty());
    wait_for_model_update(&mut updates, &session_id, "fake-a").await;
    assert_no_state_after_idle(&mut updates, &session_id).await;

    // 6a2. A `$name` prompt runs the skill body as the turn, and the model sees
    //      the wrapped body rather than the name.
    drain(&mut updates);
    client_state.take_updates();
    let requests_before = fake.request_count();
    session
        .send_prompt("$greet the reader")
        .block_task()
        .await?;
    let (idle, _) = wait_for_idle(&mut updates, &session_id).await;
    assert_eq!(idle.stop_reason, Some(schema::StopReason::EndTurn));
    assert!(
        fake.request_count() > requests_before,
        "a skill invocation must reach the model"
    );
    let last = fake.last_request();
    let prompt = last.to_string();
    assert!(
        prompt.contains("Say hello to the reader."),
        "the skill body and its arguments must be the user turn: {last}"
    );
    assert_no_state_after_idle(&mut updates, &session_id).await;

    // 6b. Permission mode and thought level are real options, not decoration:
    //     both round-trip through the host and are reflected in the next
    //     request's policy and thinking parameters.
    drain(&mut updates);
    client_state.take_updates();
    let permission = session
        .set_config_option("permission_mode", session_value("plan"))
        .block_task()
        .await?;
    assert_eq!(
        model_current(config_option(&permission.config_options, "permission_mode")),
        "plan"
    );
    let thought_before = model_values(config_option(&permission.config_options, "thought_level"))
        .last()
        .cloned()
        .expect("a thought tier to select");
    let thought = session
        .set_config_option("thought_level", session_value(&thought_before))
        .block_task()
        .await?;
    assert_eq!(
        model_current(config_option(&thought.config_options, "thought_level")),
        thought_before
    );
    session
        .send_prompt("plain text after config")
        .block_task()
        .await?;
    let (idle, _) = wait_for_idle(&mut updates, &session_id).await;
    assert_eq!(idle.stop_reason, Some(schema::StopReason::EndTurn));
    let last = fake.last_request();
    assert_eq!(
        last["reasoning_effort"].as_str(),
        Some(thought_before.as_str()),
        "the selected thought level must reach the request body: {last}"
    );
    // Restore the mode so the permission step below still asks.
    let restored = session
        .set_config_option("permission_mode", session_value("default"))
        .block_task()
        .await?;
    assert_eq!(
        model_current(config_option(&restored.config_options, "permission_mode")),
        "default"
    );
    assert_no_state_after_idle(&mut updates, &session_id).await;

    // 7. Idle-gated mid-turn switch: the response shows the requested model,
    //    no update arrives before idle, and the queued value is applied after.
    drain(&mut updates);
    client_state.take_updates();
    session
        .send_prompt("please ACP_TEST_SLOW")
        .block_task()
        .await?;
    wait_for_running(&mut updates, &session_id).await;
    let mid_turn = session
        .set_config_option("model", session_value("fake-b"))
        .block_task()
        .await?;
    assert_eq!(
        model_current(model_option(&mid_turn.config_options)),
        "fake-b"
    );
    // The turn cannot finish until this release, so the switch is guaranteed
    // to be observed while Running rather than racing a short fake response.
    fake.release_slow();
    let (idle, config_updates) = wait_for_idle(&mut updates, &session_id).await;
    assert_eq!(idle.stop_reason, Some(schema::StopReason::EndTurn));
    assert!(
        config_updates.is_empty(),
        "a queued switch must not be announced before the turn's idle"
    );
    let confirmation = wait_for_model_update(&mut updates, &session_id, "fake-b").await;
    assert_eq!(
        model_current(model_option(&confirmation.config_options)),
        "fake-b"
    );
    assert_no_state_after_idle(&mut updates, &session_id).await;
    let requests_before = fake.request_count();
    session.send_prompt("plain text").block_task().await?;
    let (idle, _) = wait_for_idle(&mut updates, &session_id).await;
    assert_eq!(idle.stop_reason, Some(schema::StopReason::EndTurn));
    for index in requests_before..fake.request_count() {
        assert_eq!(
            fake.model_at(index).as_deref(),
            Some("fake-b"),
            "request {index} must carry the model applied at idle"
        );
    }
    assert_no_state_after_idle(&mut updates, &session_id).await;

    // 7b. Live resume reads the actor's options, emits the replay marker and
    //     idle, and rejects cursors or cwds it cannot honour.
    drain(&mut updates);
    let live = connection
        .resume_session_from(
            schema::ResumeSessionRequest::new(session_id.clone(), cwd.clone())
                .replay_from(schema::ReplayFrom::Start(schema::ReplayFromStart::new())),
        )
        .start_session()
        .block_task()
        .await?;
    let model = model_option(&live.response.config_options);
    assert!(model_values(model).contains(&model_current(model)));
    config_option(&live.response.config_options, "permission_mode");
    config_option(&live.response.config_options, "thought_level");
    let (replay_complete, idle) = wait_for_resume_settled(&mut updates, &session_id).await;
    assert!(replay_complete, "a replay resume must send replay_complete");
    assert_eq!(idle.stop_reason, None);

    let error = connection
        .resume_session_from(
            schema::ResumeSessionRequest::new(session_id.clone(), cwd.clone()).replay_from(
                schema::ReplayFrom::Other(schema::OtherReplayFrom::new("_test", BTreeMap::new())),
            ),
        )
        .start_session()
        .block_task()
        .await
        .expect_err("an unsupported replay cursor must be rejected");
    assert_eq!(error.code, agent_client_protocol::ErrorCode::INVALID_PARAMS);

    let other_cwd = tempfile::tempdir().expect("other cwd");
    let error = connection
        .resume_session_from(schema::ResumeSessionRequest::new(
            session_id.clone(),
            other_cwd.path().to_path_buf(),
        ))
        .start_session()
        .block_task()
        .await
        .expect_err("a live session cwd mismatch must be rejected");
    assert_eq!(error.code, agent_client_protocol::ErrorCode::INVALID_PARAMS);

    // 8. Cancel while blocked on a question: answer Tact, withdraw the ACP
    //    dialog, and settle with a cancelled idle. No running may follow idle.
    drain(&mut updates);
    client_state.take_updates();
    *client_state.behavior.lock().expect("behavior lock") = ElicitationBehavior::Block;
    client_state.cancel_seen.store(false, Ordering::SeqCst);
    session
        .send_prompt("please ACP_TEST_CANCEL")
        .block_task()
        .await?;
    wait_until(
        || {
            client_state
                .elicitations
                .lock()
                .expect("elicitations lock")
                .iter()
                .any(|record| record.message.contains("Blocking question"))
        },
        "the blocking question",
    )
    .await;
    session.cancel_active_work()?;
    let (idle, _) = tokio::time::timeout(Duration::from_secs(10), async {
        wait_for_idle(&mut updates, &session_id).await
    })
    .await
    .expect("a cancelled turn must settle within 10 seconds");
    assert_eq!(idle.stop_reason, Some(schema::StopReason::Cancelled));
    wait_until(
        || client_state.cancel_seen.load(Ordering::SeqCst),
        "the agent to cancel its elicitation dialog",
    )
    .await;
    assert_no_state_after_idle(&mut updates, &session_id).await;

    // 9. Resume after close: config options, replay_complete, idle, a prompt.
    session.close().block_task().await?;
    drain(&mut updates);
    let resumed = connection
        .resume_session_from(
            schema::ResumeSessionRequest::new(session_id.clone(), cwd.clone())
                .replay_from(schema::ReplayFrom::Start(schema::ReplayFromStart::new())),
        )
        .start_session()
        .block_task()
        .await?;
    let model = model_option(&resumed.response.config_options);
    assert!(model_values(model).contains(&model_current(model)));
    let (replay_complete, idle) = wait_for_resume_settled(&mut updates, &session_id).await;
    assert!(replay_complete, "a replay resume must send replay_complete");
    assert_eq!(idle.stop_reason, None);
    resumed
        .session
        .send_prompt("after resume")
        .block_task()
        .await?;
    let (idle, _) = wait_for_idle(&mut updates, &session_id).await;
    assert_eq!(idle.stop_reason, Some(schema::StopReason::EndTurn));
    assert_no_state_after_idle(&mut updates, &session_id).await;

    // 10. Close succeeds; the client future returning proves the child exits.
    resumed.session.close().block_task().await?;
    Ok(())
}

// ── Client-side observation state ──────────────────────────────────────

#[derive(Default)]
struct ClientState {
    /// The scenario drains the notification channel while waiting for
    /// specific updates; this keeps the full sequence for post-hoc asserts.
    log: Mutex<Vec<schema::SessionUpdate>>,
    permissions: Mutex<Vec<Vec<schema::PermissionOptionKind>>>,
    elicitations: Mutex<Vec<ElicitationRecord>>,
    behavior: Mutex<ElicitationBehavior>,
    cancel_seen: AtomicBool,
}

impl ClientState {
    fn take_updates(&self) -> Vec<schema::SessionUpdate> {
        std::mem::take(&mut *self.log.lock().expect("update log lock"))
    }
}

#[derive(Clone, Default)]
enum ElicitationBehavior {
    AcceptOne(String),
    AcceptMany(Vec<String>),
    #[default]
    Block,
}

struct ElicitationRecord {
    message: String,
    multi: bool,
    values: Vec<String>,
    titles: Vec<String>,
}

impl ElicitationRecord {
    fn from_request(request: &schema::CreateElicitationRequest) -> Self {
        let mut values = Vec::new();
        let mut titles = Vec::new();
        let mut multi = false;
        if let schema::ElicitationMode::Form(form) = &request.mode
            && let Some(property) = form.requested_schema.properties.get("selections")
        {
            match property {
                schema::ElicitationPropertySchema::Array(array) => {
                    multi = true;
                    if let schema::MultiSelectItems::Titled(items) = &array.items {
                        for item in &items.options {
                            values.push(item.value.clone());
                            titles.push(item.title.clone());
                        }
                    }
                }
                schema::ElicitationPropertySchema::String(string) => {
                    if let Some(items) = &string.one_of {
                        for item in items {
                            values.push(item.value.clone());
                            titles.push(item.title.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        Self {
            message: request.message.clone(),
            multi,
            values,
            titles,
        }
    }
}

// ── Update helpers ─────────────────────────────────────────────────────

fn drain(updates: &mut UnboundedReceiver<schema::UpdateSessionNotification>) {
    while updates.try_recv().is_ok() {}
}

async fn next_update(
    updates: &mut UnboundedReceiver<schema::UpdateSessionNotification>,
    session_id: &schema::SessionId,
) -> schema::SessionUpdate {
    loop {
        let notification = tokio::time::timeout(STEP_TIMEOUT, updates.recv())
            .await
            .expect("timed out waiting for a session update")
            .expect("update channel closed");
        if &notification.session_id == session_id {
            return notification.update;
        }
    }
}

async fn wait_for_running(
    updates: &mut UnboundedReceiver<schema::UpdateSessionNotification>,
    session_id: &schema::SessionId,
) {
    loop {
        if let schema::SessionUpdate::StateUpdate(schema::StateUpdate::Running(_)) =
            next_update(updates, session_id).await
        {
            return;
        }
    }
}

async fn wait_for_idle(
    updates: &mut UnboundedReceiver<schema::UpdateSessionNotification>,
    session_id: &schema::SessionId,
) -> (schema::IdleStateUpdate, Vec<schema::ConfigOptionUpdate>) {
    let mut config_updates = Vec::new();
    loop {
        match next_update(updates, session_id).await {
            schema::SessionUpdate::ConfigOptionUpdate(update) => config_updates.push(update),
            schema::SessionUpdate::StateUpdate(schema::StateUpdate::Idle(idle)) => {
                return (idle, config_updates);
            }
            _ => {}
        }
    }
}

async fn wait_for_model_update(
    updates: &mut UnboundedReceiver<schema::UpdateSessionNotification>,
    session_id: &schema::SessionId,
    expected: &str,
) -> schema::ConfigOptionUpdate {
    loop {
        if let schema::SessionUpdate::ConfigOptionUpdate(update) =
            next_update(updates, session_id).await
        {
            let model = model_option(&update.config_options);
            if model_current(model) == expected {
                return update;
            }
        }
    }
}

async fn wait_for_resume_settled(
    updates: &mut UnboundedReceiver<schema::UpdateSessionNotification>,
    session_id: &schema::SessionId,
) -> (bool, schema::IdleStateUpdate) {
    let mut replay_complete = false;
    loop {
        match next_update(updates, session_id).await {
            schema::SessionUpdate::ReplayComplete(_) => replay_complete = true,
            schema::SessionUpdate::StateUpdate(schema::StateUpdate::Idle(idle)) => {
                return (replay_complete, idle);
            }
            _ => {}
        }
    }
}

async fn assert_no_state_after_idle(
    updates: &mut UnboundedReceiver<schema::UpdateSessionNotification>,
    session_id: &schema::SessionId,
) {
    let deadline = tokio::time::Instant::now() + QUIET_AFTER_IDLE;
    loop {
        match tokio::time::timeout_at(deadline, updates.recv()).await {
            Err(_) | Ok(None) => return,
            Ok(Some(notification)) => {
                if &notification.session_id == session_id
                    && let schema::SessionUpdate::StateUpdate(_) = notification.update
                {
                    panic!(
                        "a state update arrived after idle: {:?}",
                        notification.update
                    );
                }
            }
        }
    }
}

async fn wait_until(mut condition: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + STEP_TIMEOUT;
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ── Model option helpers ───────────────────────────────────────────────

fn config_option<'a>(
    options: &'a [schema::SessionConfigOption],
    config_id: &str,
) -> &'a schema::SessionConfigOption {
    options
        .iter()
        .find(|option| option.config_id.0.as_ref() == config_id)
        .unwrap_or_else(|| panic!("session must advertise the `{config_id}` option"))
}

fn model_option(options: &[schema::SessionConfigOption]) -> &schema::SessionConfigOption {
    let option = config_option(options, "model");
    assert_eq!(
        option.category,
        Some(schema::SessionConfigOptionCategory::Model)
    );
    option
}

fn model_current(option: &schema::SessionConfigOption) -> String {
    match &option.kind {
        schema::SessionConfigKind::Select(select) => select.current_value.to_string(),
        other => panic!("model option must be a select, got {other:?}"),
    }
}

fn model_values(option: &schema::SessionConfigOption) -> Vec<String> {
    match &option.kind {
        schema::SessionConfigKind::Select(select) => match &select.options {
            schema::SessionConfigSelectOptions::Ungrouped(values) => {
                values.iter().map(|value| value.value.to_string()).collect()
            }
            other => panic!("model values must be ungrouped, got {other:?}"),
        },
        other => panic!("model option must be a select, got {other:?}"),
    }
}

fn session_value(model: &str) -> schema::SessionConfigOptionValue {
    schema::SessionConfigOptionValue::Id {
        value: schema::SessionConfigValueId::new(model),
    }
}

// ── Fake OpenAI-compatible SSE server ──────────────────────────────────

#[derive(Clone)]
struct FakeModel {
    base_url: String,
    state: Arc<FakeState>,
}

struct FakeState {
    requests: Mutex<Vec<Value>>,
    /// Releases the gated `ACP_TEST_SLOW` response. A gate, not a sleep: the
    /// test decides when the turn may finish, so the mid-turn request window
    /// is deterministic on a loaded machine.
    release_slow: tokio::sync::Notify,
}

impl FakeModel {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake model");
        let address = listener.local_addr().expect("fake model address");
        let state = Arc::new(FakeState {
            requests: Mutex::new(Vec::new()),
            release_slow: tokio::sync::Notify::new(),
        });
        let accept_state = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let state = accept_state.clone();
                tokio::spawn(async move {
                    serve(stream, state).await;
                });
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            state,
        }
    }

    fn request_count(&self) -> usize {
        self.state.requests.lock().expect("requests lock").len()
    }

    fn model_at(&self, index: usize) -> Option<String> {
        self.state
            .requests
            .lock()
            .expect("requests lock")
            .get(index)
            .and_then(|request| request["model"].as_str())
            .map(str::to_string)
    }

    fn last_request(&self) -> Value {
        self.state
            .requests
            .lock()
            .expect("requests lock")
            .last()
            .cloned()
            .expect("at least one model request")
    }

    fn release_slow(&self) {
        self.state.release_slow.notify_one();
    }
}

async fn serve(mut stream: TcpStream, state: Arc<FakeState>) {
    let Some((headers, body_prefix)) = read_request_head(&mut stream).await else {
        return;
    };
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    let body = read_body(&mut stream, body_prefix, content_length).await;
    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    state
        .requests
        .lock()
        .expect("requests lock")
        .push(request.clone());

    let script = script_for(&request);
    if script.gated {
        state.release_slow.notified().await;
    }
    let body = script.body;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn read_request_head(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(index) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buffer[..index]).into_owned();
            let body = buffer[index + 4..].to_vec();
            return Some((headers, body));
        }
    }
}

async fn read_body(stream: &mut TcpStream, mut body: Vec<u8>, content_length: usize) -> Vec<u8> {
    let mut chunk = [0u8; 4096];
    while body.len() < content_length {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => body.extend_from_slice(&chunk[..read]),
        }
    }
    body.truncate(content_length);
    body
}

struct Script {
    body: String,
    gated: bool,
}

fn script_for(request: &Value) -> Script {
    let messages = request["messages"].as_array();
    if let Some(last) = messages.and_then(|messages| messages.last())
        && last["role"].as_str() == Some("tool")
    {
        return text_script("The tool finished.");
    }

    let user = messages
        .and_then(|messages| {
            messages
                .iter()
                .rev()
                .find(|message| message["role"].as_str() == Some("user"))
        })
        .map(|message| message.to_string())
        .unwrap_or_default();

    if user.contains("ACP_TEST_TASK") {
        tool_script("task_create", json!({"subject": "write the ACP test"}))
    } else if user.contains("ACP_TEST_MCP") {
        tool_script("mcp__t3-code__bridge_ping", json!({}))
    } else if user.contains("ACP_TEST_WRITE") {
        tool_script(
            "write_file",
            json!({"path": "created-by-acp.txt", "content": "written by the ACP test\n"}),
        )
    } else if user.contains("ACP_TEST_QUESTION") {
        tool_script(
            "ask_user",
            json!({
                "question": "Pick two",
                "options": ["first", "second", "third"],
                "multi_select": true
            }),
        )
    } else if user.contains("ACP_TEST_SINGLE") {
        tool_script(
            "ask_user",
            json!({
                "question": "Pick one",
                "options": ["first", "second", "third"],
                "multi_select": false
            }),
        )
    } else if user.contains("ACP_TEST_CANCEL") {
        tool_script(
            "ask_user",
            json!({
                "question": "Blocking question",
                "options": ["yes", "no"],
                "multi_select": false
            }),
        )
    } else if user.contains("ACP_TEST_SLOW") {
        Script {
            body: text_sse("Slow answer."),
            gated: true,
        }
    } else {
        text_script("Hello from the fake model.")
    }
}

fn text_script(text: &str) -> Script {
    Script {
        body: text_sse(text),
        gated: false,
    }
}

fn text_sse(text: &str) -> String {
    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({"choices": [{"delta": {"content": text}}]}),
        json!({
            "choices": [{"delta": {}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": 1200,
                "completion_tokens": 40,
                "total_tokens": 1240
            }
        })
    )
}

fn tool_script(name: &str, arguments: Value) -> Script {
    Script {
        body: format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"choices": [{
                "delta": {"tool_calls": [{
                    "index": 0,
                    "id": format!("call-{name}"),
                    "function": {
                        "name": name,
                        "arguments": arguments.to_string()
                    }
                }]},
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 900,
                "completion_tokens": 25,
                "total_tokens": 925
            }})
        ),
        gated: false,
    }
}

fn tool_results(request: &Value) -> String {
    request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message["role"].as_str() == Some("tool"))
        .map(|message| message["content"].to_string())
        .collect::<Vec<_>>()
        .join("\n")
}
