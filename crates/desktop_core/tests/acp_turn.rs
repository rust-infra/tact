//! End-to-end check that a real ACP agent's turn reaches the desktop reducer.
//!
//! Handshake-only coverage would pass on an adapter that starts a process and
//! then renders nothing. This test drives one full turn through the same
//! `HostBackend` the desktop uses and asserts the streamed text reaches the
//! transcript reducer, so the reverse mapping in `acp_bridge` is exercised over
//! a real connection.
//!
//! Skipped when no `tact-acp` binary and no model endpoint are available, so a
//! plain `cargo test` in a bare checkout still passes.

#![cfg(feature = "acp-host")]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tact_desktop_core::DesktopConfig;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

/// How long one turn gets, end to end.
const TURN_TIMEOUT: Duration = Duration::from_secs(60);

fn acp_binary() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("TACT_ACP_BIN") {
        return Some(path.into());
    }
    let executable = std::env::current_exe().ok()?;
    let dir = executable.parent()?.parent()?;
    let name = if cfg!(windows) {
        "tact-acp.exe"
    } else {
        "tact-acp"
    };
    let candidate = dir.join(name);
    candidate.is_file().then_some(candidate)
}

/// A model endpoint that streams one fixed answer, so the turn has a shape.
struct FakeModel {
    base_url: String,
    requests: Arc<Mutex<usize>>,
}

impl FakeModel {
    async fn start() -> Option<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await.ok()?;
        let address = listener.local_addr().ok()?;
        let requests = Arc::new(Mutex::new(0usize));
        let counter = requests.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let counter = counter.clone();
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        let Ok(read) = stream.read(&mut chunk).await else {
                            return;
                        };
                        if read == 0 {
                            break;
                        }
                        buffer.extend_from_slice(&chunk[..read]);
                        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                            // The body may still be in flight; reading the head
                            // is enough because the reply does not parse it.
                            break;
                        }
                    }
                    *counter.lock().expect("counter") += 1;
                    let events = [
                        r#"{"id":"c1","choices":[{"index":0,"delta":{"role":"assistant","content":"acp desktop "}}]}"#,
                        r#"{"id":"c1","choices":[{"index":0,"delta":{"content":"turn"},"finish_reason":"stop"}]}"#,
                        "[DONE]",
                    ];
                    let body = events
                        .iter()
                        .map(|event| format!("data: {event}\n\n"))
                        .collect::<String>();
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(body.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Some(Self {
            base_url: format!("http://{address}/v1"),
            requests,
        })
    }

    fn request_count(&self) -> usize {
        *self.requests.lock().expect("counter")
    }
}

fn write_config(home: &std::path::Path, base_url: &str) -> std::path::PathBuf {
    let dir = home.join(".tact");
    std::fs::create_dir_all(&dir).expect("config dir");
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        format!(
            r#"
[llm]
provider = "fake"

[llm.providers.fake]
api_key = "test"
base_url = "{base_url}"
model = "fake-a"
models = ["fake-a"]
protocol = "chat_completions"

[permission]
mode = "auto"
"#
        ),
    )
    .expect("write config");
    path
}

/// The same turn, with the agent running in this process.
///
/// This is the answer to "can ACP work without a binary": the transport is a
/// loopback `Channel` instead of a child process's stdio, and everything above
/// it — the protocol, the reverse mapping, the reducer — is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_in_process_acp_turn_reaches_the_desktop_transcript() {
    let Some(model) = FakeModel::start().await else {
        eprintln!("skipping: no loopback listener available");
        return;
    };

    let home = tempfile::tempdir().expect("temp home");
    let project = tempfile::tempdir().expect("temp project");
    std::fs::create_dir_all(project.path().join(".tact")).expect("project dir");
    let config_path = write_config(home.path(), &model.base_url);

    let config = DesktopConfig::load(home.path().join("app-data"))
        .expect("desktop config")
        .with_initial_project(project.path().to_path_buf());
    let mut config = config;
    config.host.config_path = Some(config_path);
    // Unlike the subprocess transport, an in-process agent has no argv to parse
    // its provider from: it reads the process globals the desktop installs at
    // startup. This test is the embedder, so it installs them itself.
    config
        .install_process_config()
        .expect("install process config");

    let transcript = tokio::time::timeout(
        TURN_TIMEOUT,
        tact_desktop_core::acp_inprocess::transcript_after_prompt(
            &config,
            project.path(),
            "say something",
        ),
    )
    .await;

    match transcript {
        Ok(Ok(text)) => {
            assert!(
                text.contains("acp desktop turn"),
                "the in-process agent's answer must reach the reducer, got: {text:?}"
            );
            assert!(
                model.request_count() > 0,
                "the turn must have called the model endpoint"
            );
        }
        Ok(Err(error)) => panic!("in-process ACP turn failed: {error}"),
        Err(_) => panic!("in-process ACP turn did not finish within {TURN_TIMEOUT:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_acp_turn_reaches_the_desktop_transcript() {
    let Some(binary) = acp_binary() else {
        eprintln!("skipping: no tact-acp binary next to the test executable");
        return;
    };
    let Some(model) = FakeModel::start().await else {
        eprintln!("skipping: no loopback listener available");
        return;
    };
    // `acp_agent_program` reads this, so the test can point at its own build.
    unsafe { std::env::set_var("TACT_ACP_BIN", &binary) };

    let home = tempfile::tempdir().expect("temp home");
    let project = tempfile::tempdir().expect("temp project");
    std::fs::create_dir_all(project.path().join(".tact")).expect("project dir");
    let config_path = write_config(home.path(), &model.base_url);

    let config = DesktopConfig::load(home.path().join("app-data"))
        .expect("desktop config")
        .with_initial_project(project.path().to_path_buf());
    // The agent reads its provider from a config file, not from the desktop's
    // in-memory options, so the path has to travel in the launch arguments.
    let mut config = config;
    config.host.config_path = Some(config_path);
    config.host.api_key = Some("test".to_string());

    let transcript = tokio::time::timeout(
        TURN_TIMEOUT,
        tact_desktop_core::acp_host::transcript_after_prompt(
            &config,
            project.path(),
            "say something",
        ),
    )
    .await;

    match transcript {
        Ok(Ok(text)) => {
            assert!(
                text.contains("acp desktop turn"),
                "the agent's streamed answer must reach the transcript reducer, got: {text:?}"
            );
            assert!(
                model.request_count() > 0,
                "the turn must have called the model endpoint"
            );
        }
        Ok(Err(error)) => panic!("ACP turn failed: {error}"),
        Err(_) => panic!("ACP turn did not finish within {TURN_TIMEOUT:?}"),
    }
}
