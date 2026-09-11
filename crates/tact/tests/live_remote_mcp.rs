//! Live-network checks against public remote MCP servers.
//!
//! These are **opt-in** (`--ignored`) because they require internet access and
//! depend on third-party services staying reachable. They exist to verify the
//! remote (Streamable HTTP) and OAuth paths end to end against real servers,
//! which unit tests with mock services cannot prove.
//!
//! Run with:
//!
//! ```sh
//! cargo test -p tact --test live_remote_mcp -- --ignored --nocapture
//! ```
//!
//! Each test owns its process (integration test binaries run separately), so
//! mutating `HOME` to point the loader at a throwaway config is safe.

use std::path::PathBuf;

/// Writes `<home>/.tact/mcp.json` and restores the previous `HOME` on drop.
struct TempHome {
    _dir: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}

impl TempHome {
    fn new(mcp_servers: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let previous = std::env::var_os("HOME");
        std::fs::create_dir_all(dir.path().join(".tact")).expect("create .tact");
        std::fs::write(
            dir.path().join(".tact/mcp.json"),
            format!(r#"{{"mcpServers":{mcp_servers}}}"#),
        )
        .expect("write mcp.json");
        // SAFETY: this test binary is single-threaded per process and nothing
        // else reads HOME concurrently.
        unsafe { std::env::set_var("HOME", dir.path()) };
        Self {
            _dir: dir,
            previous,
        }
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        match &self.previous {
            // SAFETY: see `TempHome::new`.
            Some(home) => unsafe { std::env::set_var("HOME", home) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}

#[tokio::test]
#[ignore = "requires network access to public MCP servers"]
async fn public_unauthenticated_remote_servers_connect_and_expose_tools() {
    let _home = TempHome::new(
        r#"{
            "deepwiki": { "url": "https://mcp.deepwiki.com/mcp" },
            "cloudflare-docs": { "url": "https://docs.mcp.cloudflare.com/mcp" }
        }"#,
    );

    let (router, report) = tact::mcp::load_mcp_router_with_report()
        .await
        .expect("load router");

    eprintln!("connected: {:?}", report.connected);
    eprintln!("failures: {:?}", report.failures);
    eprintln!("pending_auth: {:?}", report.pending_auth);
    eprintln!("servers: {:?}", router.server_summaries());

    assert!(
        report.failures.is_empty(),
        "unexpected failures: {:?}",
        report.failures
    );
    let names: Vec<&str> = report.connected.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"deepwiki"), "connected: {names:?}");
    assert!(names.contains(&"cloudflare-docs"), "connected: {names:?}");
    for (server, tools) in &report.connected {
        assert!(*tools > 0, "{server} exposed no tools");
    }

    // Tool names must use the native `mcp__<key>__<tool>` scheme.
    let tools = router.all_tools();
    assert!(
        tools.iter().any(|t| t.name.starts_with("mcp__deepwiki__")),
        "no deepwiki tools in {:?}",
        tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
}

#[tokio::test]
#[ignore = "requires network access to public MCP servers"]
async fn oauth_remote_server_without_auth_declared_is_reported_as_pending() {
    // Linear's MCP endpoint requires OAuth but the config declares no `auth`.
    // The 401 (`Auth required`) must be detected and upgraded to "pending
    // authorization" — actionable and never silent — instead of surfacing as
    // an opaque connection failure.
    let _home = TempHome::new(r#"{ "linear": { "url": "https://mcp.linear.app/mcp" } }"#);

    let (_router, report) = tact::mcp::load_mcp_router_with_report()
        .await
        .expect("load router");

    eprintln!("report: {report:?}");
    assert!(
        report.connected.is_empty(),
        "must not connect unauthenticated: {:?}",
        report.connected
    );
    assert!(
        report.failures.is_empty(),
        "an OAuth requirement is not a failure: {:?}",
        report.failures
    );
    assert_eq!(
        report.pending_auth,
        vec!["linear".to_owned()],
        "401 must be upgraded to pending authorization: {report:?}"
    );
    let lines = report.notice_lines();
    assert!(
        lines.iter().any(|l| l.contains("/mcp auth linear")),
        "notice must tell the user what to do: {lines:?}"
    );
}

#[tokio::test]
#[ignore = "requires network access to public MCP servers"]
async fn oauth_authorization_url_is_produced_against_a_real_provider() {
    // Exercises discovery (RFC 9728 → RFC 8414), dynamic client registration,
    // and PKCE challenge generation — everything up to the browser step. The
    // callback is never completed, so this asserts the URL and then drops.
    let _home = TempHome::new(
        r#"{
            "linear": {
                "url": "https://mcp.linear.app/mcp",
                "auth": { "type": "oauth", "scopes": ["read"] }
            }
        }"#,
    );

    let (router, report) = tact::mcp::load_mcp_router_with_report()
        .await
        .expect("load router");
    assert!(router.all_tools().is_empty());
    assert_eq!(
        report.pending_auth,
        vec!["linear".to_owned()],
        "OAuth server without credentials must be pending: {report:?}"
    );

    let config = tact::mcp::remote_config_for("linear")
        .expect("config lookup")
        .expect("linear is configured");
    assert_eq!(config.url, "https://mcp.linear.app/mcp");
    assert!(
        config.needs_authorization("linear"),
        "no credential file should exist yet"
    );

    // The flow blocks on a loopback callback we never send, so race it against
    // a timeout: we only need the authorization URL to appear first.
    let mut url: Option<String> = None;
    {
        let mut notify = |line: &str| url = Some(line.to_owned());
        let authorize = tact::mcp::authorize_server("linear", &mut notify);
        let result = tokio::time::timeout(std::time::Duration::from_secs(45), authorize).await;
        assert!(
            result.is_err(),
            "authorization should block awaiting the callback, got {result:?}"
        );
    }

    let url = url.expect("authorization URL must be reported before blocking");
    eprintln!("authorization URL: {url}");
    assert!(url.starts_with("https://mcp.linear.app/authorize"), "{url}");
    assert!(
        url.contains("code_challenge="),
        "PKCE challenge missing: {url}"
    );
    assert!(url.contains("code_challenge_method=S256"), "{url}");
    assert!(url.contains("client_id="), "client_id missing: {url}");
    assert!(url.contains("state="), "state missing: {url}");
}

#[tokio::test]
#[ignore = "requires network access to public MCP servers"]
async fn authorization_works_for_a_server_that_declared_no_auth() {
    // Closes the loop for the new 401-detection path: a server reported as
    // pending *without* an `auth` declaration must still be authorizable, or
    // the notice would point at a dead end.
    let _home = TempHome::new(r#"{ "linear": { "url": "https://mcp.linear.app/mcp" } }"#);

    let mut url: Option<String> = None;
    {
        let mut notify = |line: &str| url = Some(line.to_owned());
        let authorize = tact::mcp::authorize_server("linear", &mut notify);
        let result = tokio::time::timeout(std::time::Duration::from_secs(45), authorize).await;
        assert!(
            result.is_err(),
            "authorization should block awaiting the callback, got {result:?}"
        );
    }

    let url = url.expect("authorization URL must be reported before blocking");
    eprintln!("authorization URL (no `auth` declared): {url}");
    assert!(url.starts_with("https://mcp.linear.app/authorize"), "{url}");
    assert!(url.contains("code_challenge_method=S256"), "{url}");
    assert!(url.contains("client_id="), "{url}");
}

/// The path helper must agree with where the flow persists credentials.
#[test]
fn oauth_credential_path_is_under_the_tact_home() {
    let _home = TempHome::new("{}");
    let path = tact::mcp::oauth_credential_path("linear").expect("home set");
    assert_eq!(
        path,
        PathBuf::from(std::env::var_os("HOME").unwrap()).join(".tact/mcp/oauth/linear.json")
    );
}
