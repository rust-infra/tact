//! Regression: the TUI driver must show the OAuth URL *before* the flow blocks.
//!
//! `authorize_server` reports the authorization URL through `notify` and then
//! waits for the browser redirect (up to 5 minutes). The driver used to buffer
//! those lines and flush them only after the future resolved, so `/mcp auth`
//! showed no link at all: the URL appeared only once the flow had already
//! failed or timed out. This drives the real command handler against a mock
//! OAuth provider and asserts the URL reaches the UI while the flow is still
//! waiting for the callback.
//!
//! Runs as its own integration-test binary because it changes the process
//! working directory to the temporary project.

use std::time::Duration;

use serde_json::json;
use tact_llm::MockClient;
use tact_protocol::{AgentUpdate, UserCommand};
use tact_ui::{
    driver::handle_user_command,
    test_support::{build_test_agent, install_test_config},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, path_regex},
};

/// Serves the two discovery documents rmcp needs: protected-resource metadata
/// pointing at this same server, and authorization-server metadata.
///
/// No registration endpoint is served because the test declares a client id,
/// which is exactly the config path a user with a pre-registered Figma client
/// would follow — and which keeps the flow offline.
async fn mount_oauth_provider(server: &MockServer) {
    let base = server.uri();
    Mock::given(method("GET"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "resource": format!("{base}/mcp"),
            "authorization_servers": [base],
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/\.well-known/.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": server.uri(),
            "authorization_endpoint": format!("{}/authorize", server.uri()),
            "token_endpoint": format!("{}/token", server.uri()),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn oauth_url_reaches_the_ui_before_the_callback_arrives() {
    install_test_config();
    // The OAuth manager builds its own HTTP client, which honours the ambient
    // proxy setup; loopback must bypass it or the mock server is unreachable.
    unsafe {
        std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
        std::env::set_var("no_proxy", "127.0.0.1,localhost");
    }

    let server = MockServer::start().await;
    mount_oauth_provider(&server).await;

    let project = tempfile::tempdir().expect("temp project");
    std::fs::create_dir_all(project.path().join(".tact")).expect("create .tact");
    let declaration = json!({
        "mcpServers": {
            "hosted": {
                "type": "http",
                "url": format!("{}/mcp", server.uri()),
                "auth": { "type": "oauth", "clientId": "test-client" }
            }
        }
    });
    std::fs::write(
        project.path().join(".tact/mcp.json"),
        serde_json::to_vec_pretty(&declaration).expect("serialize declaration"),
    )
    .expect("write mcp.json");
    std::env::set_current_dir(project.path()).expect("enter the temp project");

    let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (mut agent, workdir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

    let command = handle_user_command(
        &mut agent,
        UserCommand::McpAuth {
            server: "hosted".to_string(),
        },
        &workdir,
    );
    tokio::pin!(command);

    // The URL must arrive while the command is still parked on the callback;
    // `command` completing first means the old buffer-then-flush behavior.
    let timeout = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(timeout);
    let mut url = None;
    loop {
        tokio::select! {
            _ = &mut command => panic!("flow finished without ever showing the URL"),
            update = agent_rx.recv() => {
                match update {
                    // Only the authorization URL is actionable; ignore any
                    // other progress line and keep waiting for it.
                    Some(AgentUpdate::Info(line)) if line.contains("code_challenge") => {
                        url = Some(line);
                        break;
                    }
                    Some(_) => continue,
                    None => panic!("agent update channel closed before the URL arrived"),
                }
            }
            _ = &mut timeout => break,
        }
    }

    let url = url.expect("no Info update arrived before the flow finished");
    assert!(
        url.contains("code_challenge_method=S256") && url.contains("client_id=test-client"),
        "the update must carry the authorization URL, got: {url}"
    );
    assert!(
        url.contains("redirect_uri="),
        "the URL must carry the loopback redirect URI, got: {url}"
    );
}
