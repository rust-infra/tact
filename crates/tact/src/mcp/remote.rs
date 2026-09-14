//! Remote MCP servers: Streamable HTTP transport and OAuth 2.0 authorization.
//!
//! A remote entry in `mcp.json` declares a `url` instead of a `command`:
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "remote": {
//!       "url": "https://mcp.example.com/mcp",
//!       "headers": { "X-Api-Key": "..." },
//!       "auth": { "type": "oauth", "scopes": ["tools.read"] }
//!     }
//!   }
//! }
//! ```
//!
//! Transport is the MCP Streamable HTTP client from `rmcp`, so session
//! management and SSE reconnection are handled by the SDK. Authentication is
//! either static (`headers`) or OAuth 2.0 authorization-code + PKCE
//! (`auth.type = "oauth"`, MCP SEP-985): the interactive browser round-trip is
//! performed by [`authorize_remote_server`] through a loopback listener, and
//! the resulting token is persisted per server so later sessions connect
//! without prompting.
//!
//! Note: never log token or header *values* — log server names, URLs and
//! header *names* only.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use http::{HeaderName, HeaderValue};
use rmcp::{
    RoleClient, ServiceExt,
    service::{ClientInitializeError, RunningService},
    transport::{
        StreamableHttpClientTransport,
        auth::{
            AuthError, AuthorizationManager, AuthorizationSession, CredentialStore,
            OAuthClientConfig, StoredCredentials,
        },
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use crate::consts::TactPath;

/// How long the interactive browser round-trip may take before giving up.
const OAUTH_CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// How long the authorization code may take to become a token.
///
/// Separate from the browser window because it covers a machine-to-machine
/// call: if the token endpoint accepts the POST and never answers, the command
/// must fail rather than hang forever (rmcp builds that client without a
/// timeout of its own).
const OAUTH_TOKEN_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(60);

/// Loopback path the OAuth provider redirects back to.
const OAUTH_CALLBACK_PATH: &str = "/callback";

/// Ceiling on the remote `initialize` handshake.
///
/// The HTTP client is built without a total request timeout — a slow but
/// healthy server must not be cut off mid-request — so the handshake needs its
/// own bound, or a server that accepts the connection and never answers would
/// hang startup forever.
const REMOTE_INIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Authentication declared on a remote MCP server entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum McpAuthConfig {
    /// OAuth 2.0 authorization-code + PKCE. The access token is sent as
    /// `Authorization: Bearer <token>`; refresh is automatic while a refresh
    /// token is present, otherwise re-authorization is required.
    #[serde(rename_all = "camelCase")]
    Oauth {
        /// Pre-registered client id. When absent the server's dynamic client
        /// registration (RFC 7591) is used, which is the MCP default.
        #[serde(default)]
        client_id: Option<String>,
        /// `client_name` to register under, overriding `mcp.oauth_client_name`.
        ///
        /// Providers may gate registration on this exact string (see
        /// [`crate::config::McpSettings::oauth_client_name`]); this is the
        /// per-server escape hatch for one provider that needs a different
        /// answer than the global default.
        #[serde(default)]
        client_name: Option<String>,
        /// Requested scopes; empty means "let the server decide".
        #[serde(default)]
        scopes: Vec<String>,
        /// Loopback port for the redirect URI. `None` (or `0`) picks a free
        /// port, which is the safest default.
        #[serde(default)]
        callback_port: Option<u16>,
    },
}

/// A remote (Streamable HTTP) MCP server declaration.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRemoteConfig {
    pub url: String,
    /// Extra request headers. Static authentication (`Authorization: Bearer …`)
    /// belongs here; invalid header names/values are ignored with a warning.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub auth: Option<McpAuthConfig>,
}

/// Resolved authentication for a remote connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteAuthState {
    /// No auth configured; only static `headers` (if any) apply.
    None,
    /// Bearer token to send as `Authorization` (OAuth access token).
    Token(String),
    /// OAuth is configured but no usable credential exists yet.
    AuthorizationRequired,
}

impl McpRemoteConfig {
    /// Whether this entry is *known* to need an interactive OAuth run.
    ///
    /// True only when OAuth is declared and no credential file exists yet, so
    /// startup can report it without any network call. A server that does not
    /// declare `auth` cannot be predicted this way — it is discovered when it
    /// answers 401 (`serve_remote` → `is_auth_required_error`).
    #[must_use]
    pub fn needs_authorization(&self, server_name: &str) -> bool {
        matches!(self.auth, Some(McpAuthConfig::Oauth { .. }))
            && oauth_credential_path(server_name).is_none_or(|path| !path.is_file())
    }

    /// Builds the transport config (headers + resolved auth) for a connection.
    fn transport_config(
        &self,
        server_name: &str,
        auth: &RemoteAuthState,
    ) -> StreamableHttpClientTransportConfig {
        let mut custom_headers: HashMap<HeaderName, HeaderValue> = HashMap::new();
        let oauth_token = matches!(auth, RemoteAuthState::Token(_));
        for (name, value) in &self.headers {
            // An OAuth token always wins the Authorization header; keeping the
            // static one too would be ambiguous, so drop it loudly.
            if oauth_token && name.eq_ignore_ascii_case("authorization") {
                tracing::warn!(
                    mcp_server = %server_name,
                    "ignoring static Authorization header because OAuth is configured"
                );
                continue;
            }
            match (
                HeaderName::try_from(name.as_str()),
                HeaderValue::try_from(value.as_str()),
            ) {
                (Ok(header), Ok(header_value)) => {
                    custom_headers.insert(header, header_value);
                }
                _ => tracing::warn!(
                    mcp_server = %server_name,
                    header = %name,
                    "ignoring invalid MCP header"
                ),
            }
        }
        tracing::debug!(
            mcp_server = %server_name,
            url = %self.url,
            headers = custom_headers.len(),
            oauth = oauth_token,
            "building remote MCP transport"
        );
        let mut config = StreamableHttpClientTransportConfig::with_uri(self.url.clone());
        config.custom_headers = custom_headers;
        if let RemoteAuthState::Token(token) = auth {
            config.auth_header = Some(token.clone());
        }
        config
    }
}

/// Marker error: the server is configured for OAuth but has no usable
/// credential, so the interactive flow must run before it can connect.
///
/// Callers downcast to this to report a server as *pending authorization*
/// (an actionable state) rather than a connection failure.
#[derive(Debug)]
pub struct AuthorizationRequired;

impl std::fmt::Display for AuthorizationRequired {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OAuth authorization required")
    }
}

impl std::error::Error for AuthorizationRequired {}

/// Connects a remote server and returns an initialized client service.
///
/// Returns [`AuthorizationRequired`] when OAuth is configured but no usable
/// credential exists; callers that want the "pending authorization" UX should
/// check [`McpRemoteConfig::needs_authorization`] first.
pub async fn serve_remote(
    server_name: &str,
    config: &McpRemoteConfig,
) -> Result<RunningService<RoleClient, ()>> {
    let auth = resolve_remote_auth(server_name, config).await?;
    if matches!(auth, RemoteAuthState::AuthorizationRequired) {
        return Err(anyhow::Error::new(AuthorizationRequired));
    }
    let transport_config = config.transport_config(server_name, &auth);
    tracing::info!(mcp_server = %server_name, url = %config.url, "connecting remote MCP server");
    let transport =
        StreamableHttpClientTransport::with_client(http_client_for(&config.url), transport_config);
    tokio::time::timeout(REMOTE_INIT_TIMEOUT, ().serve(transport))
        .await
        .with_context(|| {
            format!(
                "remote MCP server {server_name} did not complete the handshake within {}s",
                REMOTE_INIT_TIMEOUT.as_secs()
            )
        })?
        .with_context(|| format!("failed to initialize remote MCP client for server {server_name}"))
}

/// Builds the HTTP client for a remote server endpoint.
///
/// reqwest honours `HTTP_PROXY` / `ALL_PROXY` by default, and that includes
/// **loopback** targets — which is wrong for a local MCP server such as Figma's
/// desktop server (`http://127.0.0.1:3845/mcp`). With a proxy exported, the
/// request never reaches the local server: the proxy answers, and the failure
/// surfaces as a misleading `Unexpected content type: None` rather than
/// "connection refused". Every other tool (curl, browsers) bypasses the proxy
/// for loopback, so Tact does too.
///
/// Non-loopback endpoints keep the environment's proxy, since a proxy is
/// exactly what makes a remote MCP server reachable in a restricted network.
/// Note this covers the MCP transport only: the OAuth manager builds its own
/// client, so discovery/registration for a loopback server would still use the
/// environment proxy.
fn http_client_for(url: &str) -> reqwest13::Client {
    if !is_loopback_url(url) {
        return reqwest13::Client::default();
    }
    tracing::debug!(
        url = %url,
        "loopback MCP server: connecting without a proxy"
    );
    match reqwest13::Client::builder().no_proxy().build() {
        Ok(client) => client,
        Err(error) => {
            // Falling back to the proxy-exposing default keeps the connection
            // attempt alive, but says why it may now fail.
            tracing::warn!(
                url = %url,
                error = %error,
                "failed to build a proxy-free HTTP client for a loopback MCP server"
            );
            reqwest13::Client::default()
        }
    }
}

/// Whether `url` targets the local machine.
///
/// Deliberately string-based: the MCP config carries a URL the user typed, and
/// resolving DNS or pulling in a URL parser just to recognise `localhost` would
/// add failure modes where a simple check is enough.
#[must_use]
fn is_loopback_url(url: &str) -> bool {
    let Some((_, rest)) = url.split_once("://") else {
        return false;
    };
    // The authority ends at the first path/query/fragment delimiter.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // Drop any `user:password@` prefix.
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if let Some(rest) = host_port.strip_prefix('[') {
        // Bracketed IPv6 literal, e.g. `[::1]:8080`.
        rest.split(']').next().unwrap_or_default()
    } else {
        // A bare IPv6 literal is never valid here; splitting on `:` yields the
        // host for the host:port form.
        host_port.split(':').next().unwrap_or_default()
    };
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host == "0:0:0:0:0:0:0:1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Resolves the bearer token for a remote server, refreshing it if needed.
///
/// A stored credential is honoured **regardless** of whether the config
/// declares `auth`: a token obtained through `/mcp auth` is valid evidence even
/// when the server was only discovered to require OAuth at connect time. When
/// the config *does* declare OAuth and no credential is usable,
/// `Ok(AuthorizationRequired)` signals the interactive flow.
pub async fn resolve_remote_auth(
    server_name: &str,
    config: &McpRemoteConfig,
) -> Result<RemoteAuthState> {
    if let Some(token) = stored_access_token(server_name, &config.url).await? {
        return Ok(RemoteAuthState::Token(token));
    }
    if matches!(config.auth, Some(McpAuthConfig::Oauth { .. })) {
        return Ok(RemoteAuthState::AuthorizationRequired);
    }
    // No `auth` declared and no credential: connect anonymously and let the
    // server decide. A 401 is then detected as "authorization required".
    Ok(RemoteAuthState::None)
}

/// Reads and (if needed) refreshes the stored OAuth access token.
///
/// `Ok(None)` means no usable credential exists: the file is absent, or the
/// token is expired and cannot be refreshed.
///
/// Note: rmcp re-discovers the authorization-server metadata before handing
/// back a cached token, so a transient outage of the metadata (or protected
/// resource metadata) endpoint makes the connection fail even though the
/// stored token itself is still valid. That is rmcp's flow, not a choice made
/// here; the error surfaces as a connection failure, and re-running once the
/// endpoint is reachable succeeds.
async fn stored_access_token(server_name: &str, url: &str) -> Result<Option<String>> {
    let Some(path) = oauth_credential_path(server_name) else {
        return Ok(None);
    };
    stored_access_token_at(&path, server_name, url).await
}

/// [`stored_access_token`] with an explicit credential path (unit-testable).
async fn stored_access_token_at(
    path: &Path,
    server_name: &str,
    url: &str,
) -> Result<Option<String>> {
    let store = FileCredentialStore::new(path.to_path_buf());
    if store.load().await.map_err(anyhow::Error::from)?.is_none() {
        tracing::debug!(mcp_server = %server_name, "no stored OAuth credentials");
        return Ok(None);
    }

    let mut manager = AuthorizationManager::new(url)
        .await
        .with_context(|| format!("failed to start OAuth manager for {server_name}"))?;
    manager.set_credential_store(store);
    if !manager
        .initialize_from_store()
        .await
        .with_context(|| format!("failed to load OAuth credentials for {server_name}"))?
    {
        return Ok(None);
    }
    match manager.get_access_token().await {
        Ok(token) => {
            tracing::debug!(mcp_server = %server_name, "using stored OAuth access token");
            Ok(Some(token))
        }
        Err(AuthError::AuthorizationRequired) => {
            tracing::info!(
                mcp_server = %server_name,
                "stored OAuth token expired and not refreshable"
            );
            Ok(None)
        }
        Err(error) => Err(anyhow::Error::from(error)
            .context(format!("failed to obtain OAuth token for {server_name}"))),
    }
}

/// Whether an MCP connect failure means "this server wants OAuth".
///
/// rmcp models this as `StreamableHttpError::AuthRequired`, but that variant
/// holds a type that implements neither `Display` nor `Error`, and
/// `ClientInitializeError::TransportError` does not chain it via `#[source]` —
/// so it cannot be downcast. What *is* reachable is the rmcp
/// [`ClientInitializeError`] itself (verified against rmcp 0.17). Matching on
/// its transport variant and looking for rmcp's `"Auth required"` message is
/// therefore the tightest available check; anything unrecognised falls back to
/// a plain connection failure, so a wording change degrades rather than
/// misreports.
#[must_use]
pub fn is_auth_required_error(error: &anyhow::Error) -> bool {
    let Some(ClientInitializeError::TransportError { error, .. }) =
        error.downcast_ref::<ClientInitializeError>()
    else {
        return false;
    };
    error.error.to_string().contains("Auth required")
}

/// The OAuth knobs for a remote server.
///
/// Defaults to "no client id, no scopes, ephemeral port" when the entry does
/// not declare `auth`: a server that demands OAuth can still be authorized on
/// demand, because the 401 is proof enough.
/// The resolved OAuth knobs for a remote server.
///
/// A named struct rather than a tuple: adding `client_name` made an anonymous
/// 4-tuple genuinely hard to read at the call site.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OauthRequestParameters {
    /// Pre-registered client id, if the entry declares one.
    client_id: Option<String>,
    /// Per-server `client_name` override, if declared.
    client_name: Option<String>,
    scopes: Vec<String>,
    callback_port: Option<u16>,
}

impl OauthRequestParameters {
    /// The `client_name` to register under: the server's override, else the
    /// configured default, else the built-in default.
    ///
    /// Falling back to [`McpSettings::default`] rather than panicking matters
    /// because `settings()` is unavailable in some unit-test and library
    /// contexts.
    fn effective_client_name(&self) -> String {
        self.client_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| {
                crate::config::try_settings()
                    .map(|settings| settings.mcp.oauth_client_name)
                    .unwrap_or_else(|| crate::config::McpSettings::default().oauth_client_name)
            })
    }
}

/// The OAuth knobs for a remote server.
///
/// Defaults to "no client id, no scopes, ephemeral port" when the entry does
/// not declare `auth`: a server that demands OAuth can still be authorized on
/// demand, because the 401 is proof enough.
fn oauth_parameters(config: &McpRemoteConfig) -> OauthRequestParameters {
    match &config.auth {
        Some(McpAuthConfig::Oauth {
            client_id,
            client_name,
            scopes,
            callback_port,
        }) => OauthRequestParameters {
            client_id: client_id.clone(),
            client_name: client_name.clone(),
            scopes: scopes.clone(),
            callback_port: *callback_port,
        },
        None => OauthRequestParameters {
            client_id: None,
            client_name: None,
            scopes: Vec::new(),
            callback_port: None,
        },
    }
}

/// Turns a dynamic-client-registration failure into an actionable error.
///
/// rmcp's own message is only `HTTP 403 Forbidden`: accurate but not actionable.
/// A 403 here almost always means the provider only accepts clients it already
/// knows about (Figma, for example, allowlists the clients in its MCP catalog),
/// in which case no retry helps and the fix is a config change.
fn registration_error(
    server_name: &str,
    url: &str,
    callback_port: Option<u16>,
    client_name: &str,
    has_registration_endpoint: bool,
    error: AuthError,
) -> anyhow::Error {
    tracing::warn!(
        mcp_server = %server_name,
        url = %url,
        client_name = %client_name,
        advertised_registration_endpoint = has_registration_endpoint,
        error = %error,
        "OAuth dynamic client registration failed"
    );
    anyhow!(registration_message(
        server_name,
        url,
        callback_port,
        client_name,
        has_registration_endpoint,
        &error.to_string(),
    ))
}

/// The user-facing text for a refused dynamic client registration.
///
/// Split out from [`registration_error`] so the guidance is unit-testable.
fn registration_message(
    server_name: &str,
    url: &str,
    callback_port: Option<u16>,
    client_name: &str,
    has_registration_endpoint: bool,
    raw_error: &str,
) -> String {
    let cause = if has_registration_endpoint {
        "the provider refused it"
    } else {
        "the provider does not advertise a registration endpoint"
    };
    // Providers commonly gate registration on the *client name*, which is why
    // it is configurable at all. Verified example: Figma's registration
    // endpoint answers 200 for `client_name: "Codex"` and 403 for "Tact" with
    // an otherwise identical request body.
    let name_hint = if has_registration_endpoint {
        format!(
            "Tact registered as \"{client_name}\". Providers often gate registration on that\n\
             name, so a different value may succeed: set `mcp.oauth_client_name` in config.toml,\n\
             or `auth.clientName` on this server. The default is \"Codex\", which is what\n\
             providers such as Figma admit."
        )
    } else {
        String::new()
    };
    // A concrete redirect URI is exactly what a provider asks for when a client
    // is registered by hand, so show one instead of describing it abstractly.
    let redirect = match callback_port {
        Some(port) => format!("http://127.0.0.1:{port}/callback"),
        None => "http://127.0.0.1:<port>/callback".to_string(),
    };

    format!(
        "OAuth client registration failed for {server_name}: {reason}\n\n\
         {server_name} ({url}) does not accept a self-registered OAuth client: {cause}.\n\
         {name_hint}\n\
         Retrying will not help — the provider decides who may register.\n\n\
         Options:\n\
         \x20 * Register an OAuth client with the provider yourself, then declare it:\n\
         \x20     \"auth\": {{ \"type\": \"oauth\", \"clientId\": \"<client-id>\", \"callbackPort\": <port> }}\n\
         \x20   Use {redirect} as its redirect URI, with that same port.\n\
         \x20 * Or use a static token the provider issues, via headers:\n\
         \x20     \"headers\": {{ \"Authorization\": \"Bearer <token>\" }}\n\
         \x20 * Or run the server locally if the provider ships one (Figma's desktop\n\
         \x20   server at http://127.0.0.1:3845/mcp needs no OAuth).",
        reason = registration_reason(raw_error),
    )
}

/// Reduces rmcp's nested registration error to its innermost reason.
///
/// rmcp wraps the failure twice, producing
/// `Registration failed: Dynamic registration failed: Registration failed: HTTP 403 Forbidden: Forbidden`.
/// Only the last segment carries information. If rmcp changes its wording the
/// prefix no longer matches and the full text is shown — degraded, not wrong.
fn registration_reason(message: &str) -> String {
    const PREFIXES: [&str; 2] = ["Registration failed: ", "Dynamic registration failed: "];
    let mut reason = message.trim();
    loop {
        match PREFIXES
            .iter()
            .find_map(|prefix| reason.strip_prefix(prefix))
        {
            Some(stripped) => reason = stripped.trim(),
            None => return reason.to_string(),
        }
    }
}

/// Runs the interactive OAuth flow for a remote server and persists the token.
///
/// Works whether or not the config declares `auth`: a server that answers 401
/// is authorized on demand (the declaration only pre-answers the question).
/// `notify` receives human-facing progress lines (the authorization URL above
/// all) so the caller can route them to the TUI or stderr; the flow itself
/// never prints. Binding the loopback listener happens before discovery so the
/// redirect URI registered with the provider is exact.
pub async fn authorize_remote_server(
    server_name: &str,
    config: &McpRemoteConfig,
    notify: &mut (dyn FnMut(&str) + Send),
) -> Result<()> {
    let params = oauth_parameters(config);
    let client_name = params.effective_client_name();
    let store_path = oauth_credential_path(server_name)
        .ok_or_else(|| anyhow!("cannot determine OAuth credential path ($HOME is not set)"))?;

    let listener = TcpListener::bind(("127.0.0.1", params.callback_port.unwrap_or(0)))
        .await
        .with_context(|| format!("failed to bind loopback listener for {server_name}"))?;
    let port = listener
        .local_addr()
        .context("failed to read loopback address")?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}{OAUTH_CALLBACK_PATH}");
    tracing::info!(
        mcp_server = %server_name,
        url = %config.url,
        %redirect_uri,
        %client_name,
        "starting remote MCP OAuth authorization"
    );

    let mut manager = AuthorizationManager::new(&config.url)
        .await
        .with_context(|| format!("failed to start OAuth manager for {server_name}"))?;
    manager.set_credential_store(FileCredentialStore::new(store_path.clone()));
    let metadata = manager
        .discover_metadata()
        .await
        .with_context(|| format!("OAuth discovery failed for {server_name}"))?;
    // Recorded before the metadata is moved into the manager: it distinguishes
    // "registration was offered and refused" from "registration was never
    // offered" when reporting a failure.
    let has_registration_endpoint = metadata.registration_endpoint.is_some();
    tracing::debug!(
        mcp_server = %server_name,
        issuer = ?metadata.issuer,
        registration_endpoint = metadata.registration_endpoint.as_deref().unwrap_or("<none>"),
        "OAuth metadata discovered"
    );
    manager.set_metadata(metadata);

    let scope_refs: Vec<&str> = params.scopes.iter().map(String::as_str).collect();
    let session = match params.client_id.clone() {
        Some(client_id) => {
            // A pre-registered client id skips dynamic registration; the
            // redirect URI must still be our loopback address.
            manager
                .configure_client(OAuthClientConfig {
                    client_id: client_id.clone(),
                    client_secret: None,
                    scopes: params.scopes.clone(),
                    redirect_uri: redirect_uri.clone(),
                })
                .with_context(|| format!("invalid OAuth client id for {server_name}"))?;
            let auth_url = manager
                .get_authorization_url(&scope_refs)
                .await
                .with_context(|| format!("failed to build authorization URL for {server_name}"))?;
            AuthorizationSession::for_scope_upgrade(manager, auth_url, &redirect_uri)
        }
        None => {
            match AuthorizationSession::new(
                manager,
                &scope_refs,
                &redirect_uri,
                Some(client_name.as_str()),
                None,
            )
            .await
            {
                Ok(session) => session,
                // Registration is the one step whose failure has a config-level
                // fix, so it gets a dedicated explanation instead of the raw
                // `HTTP 403 Forbidden` rmcp reports.
                Err(error @ AuthError::RegistrationFailed(_)) => {
                    return Err(registration_error(
                        server_name,
                        &config.url,
                        params.callback_port,
                        &client_name,
                        has_registration_endpoint,
                        error,
                    ));
                }
                Err(error) => {
                    return Err(anyhow::Error::from(error)
                        .context(format!("OAuth client setup failed for {server_name}")));
                }
            }
        }
    };
    let auth_url = session.get_authorization_url().to_string();
    // The URL is the most useful troubleshooting artifact, but it carries the
    // one-time `state` CSRF nonce, which must not be durably recorded.
    tracing::info!(
        mcp_server = %server_name,
        auth_url = %redact_query_value(&auth_url, "state"),
        "OAuth authorization URL ready"
    );
    notify(&auth_url);

    let (code, state) = await_oauth_callback(listener, OAUTH_CALLBACK_TIMEOUT).await?;
    tracing::debug!(mcp_server = %server_name, "OAuth callback received");
    // rmcp performs the exchange on a client it builds without a timeout, so a
    // token endpoint that never answers would otherwise hang this command with
    // no way for the user to interrupt it.
    tokio::time::timeout(
        OAUTH_TOKEN_EXCHANGE_TIMEOUT,
        session.handle_callback(&code, &state),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "timed out after {}s waiting for the OAuth token exchange for {server_name}",
            OAUTH_TOKEN_EXCHANGE_TIMEOUT.as_secs()
        )
    })?
    .with_context(|| format!("OAuth token exchange failed for {server_name}"))?;
    tracing::info!(
        mcp_server = %server_name,
        path = %store_path.display(),
        "remote MCP OAuth authorization completed"
    );
    Ok(())
}

/// Largest HTTP request head accepted from the loopback callback.
const MAX_REQUEST_BYTES: usize = 8192;

/// Accepts loopback HTTP requests until one carries the OAuth callback.
///
/// A single stray connection must not abort the flow: a browser prefetch, a
/// second tab, a port scan or a hand visit to the URL all arrive on this port,
/// and any of them winning the race used to fail the authorization. Anything
/// that is not the registered callback path is answered and ignored, and the
/// wait continues until `timeout`.
async fn await_oauth_callback(
    listener: TcpListener,
    timeout: Duration,
) -> Result<(String, String)> {
    let exchange = async {
        loop {
            let (stream, _) = listener.accept().await.context("accept failed")?;
            match handle_callback_request(stream).await {
                CallbackRequest::Callback { code, state } => return Ok((code, state)),
                // A denial is the user's answer: stop waiting for a callback
                // that will never come.
                CallbackRequest::Denied(error) => bail!("authorization was denied: {error}"),
                // Noise (or a malformed callback): the response is already
                // written, so keep listening for the real redirect.
                CallbackRequest::Ignored => continue,
            }
        }
    };

    match tokio::time::timeout(timeout, exchange).await {
        Ok(result) => result,
        Err(_) => bail!(
            "timed out waiting for OAuth callback after {}s",
            timeout.as_secs()
        ),
    }
}

/// What one loopback connection turned out to be.
enum CallbackRequest {
    /// The provider redirect, carrying the code and state to exchange.
    Callback { code: String, state: String },
    /// The provider reported an `error` (the user denied, or the request was
    /// rejected). Carries `error` plus `error_description` when provided.
    Denied(String),
    /// Anything else — a prefetch, a stray connection, or a request on another
    /// path. A response has already been sent; the caller keeps waiting.
    Ignored,
}

/// Reads and classifies one loopback connection.
///
/// Responses are written here (not by the caller) so every connection gets an
/// answer before it is dropped, whichever branch it takes.
async fn handle_callback_request(mut stream: TcpStream) -> CallbackRequest {
    let Some(request) = read_request_head(&mut stream).await else {
        respond(
            &mut stream,
            "400 Bad Request",
            "Malformed request. You can close this tab.",
        )
        .await;
        return CallbackRequest::Ignored;
    };

    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or_default();
    // Only the redirect URI registered with the provider is the callback;
    // `/favicon.ico` and anything else is browser noise.
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != OAUTH_CALLBACK_PATH {
        respond(&mut stream, "404 Not Found", "Not found.").await;
        return CallbackRequest::Ignored;
    }

    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut error_description = None;
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let value = percent_decode(value);
        match key {
            "code" => code = Some(value),
            "state" => state = Some(value),
            "error" => error = Some(value),
            "error_description" => error_description = Some(value),
            _ => {}
        }
    }

    if let Some(error) = error {
        let detail = error_description
            .filter(|description| !description.is_empty())
            .map_or_else(
                || error.clone(),
                |description| format!("{error}: {description}"),
            );
        respond(
            &mut stream,
            "400 Bad Request",
            "Authorization failed. You can close this tab.",
        )
        .await;
        return CallbackRequest::Denied(detail);
    }
    match (code, state) {
        (Some(code), Some(state)) => {
            respond(
                &mut stream,
                "200 OK",
                "Authorization complete. You can close this tab.",
            )
            .await;
            CallbackRequest::Callback { code, state }
        }
        // The registered path without a code: a hand visit, or a redirect that
        // lost its query. Answer, then keep waiting for the real callback
        // rather than failing the whole flow.
        _ => {
            respond(
                &mut stream,
                "400 Bad Request",
                "Missing authorization code. You can close this tab.",
            )
            .await;
            CallbackRequest::Ignored
        }
    }
}

/// Reads an HTTP request head, up to and excluding its terminating blank line.
///
/// A single `read` is not guaranteed to return the whole request line, so this
/// loops until the head is complete, the peer closes, or the buffer fills.
/// `Ok(None)` means the bytes on the wire were not a request.
async fn read_request_head(stream: &mut TcpStream) -> Option<String> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        // Stop as soon as the head is complete: reading further would block on
        // a client that waits for our response before sending anything else.
        if let Some(head) = request_head(&buffer) {
            return Some(head);
        }
        if buffer.len() >= MAX_REQUEST_BYTES {
            return None;
        }
        match stream.read(&mut chunk).await {
            // EOF without a blank line: parse a bare request line if that is
            // all the peer sent before closing.
            Ok(0) => break,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            Err(_) => return None,
        }
    }
    request_head(&buffer).or_else(|| {
        let text = String::from_utf8_lossy(&buffer);
        let line = text.lines().next().unwrap_or_default().trim();
        (!line.is_empty()).then(|| line.to_string())
    })
}

/// The request head ending at the first blank line, if the buffer holds one.
fn request_head(buffer: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(buffer);
    text.find("\r\n\r\n")
        .or_else(|| text.find("\n\n"))
        .map(|end| text[..end].to_string())
}

/// Writes a minimal HTTP response and closes the connection.
async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Minimal `application/x-www-form-urlencoded` value decoder (`%XX` and `+`).
///
/// Decodes from the raw bytes rather than slicing the `&str`: a two-byte hex
/// window can end inside a multi-byte character (`%aé`), and byte-slicing
/// there panics. Any local process can reach the callback port, so that must
/// degrade to a literal `%` instead of taking the process down.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                match (hex_nibble(bytes[index + 1]), hex_nibble(bytes[index + 2])) {
                    (Some(high), Some(low)) => {
                        out.push((high << 4) | low);
                        index += 3;
                    }
                    _ => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Value of one ASCII hex digit, or `None`.
fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Replaces one query parameter's value with `[redacted]`.
///
/// Used for `state` before logging the authorization URL: the rest of the URL
/// is safe and worth keeping for troubleshooting.
fn redact_query_value(url: &str, key: &str) -> String {
    let Some((head, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let redacted = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, _)) if name == key => format!("{name}=[redacted]"),
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{head}?{redacted}")
}

/// `~/.tact/mcp/oauth/<server>.json` — credential file for one server.
///
/// Returns `None` for a name that is unsafe as a path component, so a hostile
/// or malformed `mcp.json` key — or a `mcp logout <name>` argument — can never
/// read or delete a file outside this directory. Callers treat `None` the same
/// way they treat "no credential", which is the safe direction: the server is
/// reported as needing authorization rather than silently trusting a file.
#[must_use]
pub fn oauth_credential_path(server_name: &str) -> Option<PathBuf> {
    if !crate::mcp::is_safe_server_name(server_name) {
        tracing::warn!(
            mcp_server = %server_name,
            "refusing to derive an OAuth credential path from an unsafe server name"
        );
        return None;
    }
    TactPath::home_mcp_oauth_dir().map(|dir| dir.join(format!("{server_name}.json")))
}

/// Deletes the stored OAuth credential for `server_name`.
///
/// Returns the path that was removed, or `None` when nothing was stored. Unlike
/// [`authorize_remote_server`] this needs no configured server: cleaning up a
/// credential after removing a declaration is a legitimate thing to do.
///
/// `server_name` is validated before it becomes a path, so `logout` cannot be
/// pointed at an arbitrary file.
pub async fn forget_credentials(server_name: &str) -> Result<Option<PathBuf>> {
    crate::mcp::validate_server_name(server_name)?;
    let Some(path) = oauth_credential_path(server_name) else {
        return Ok(None);
    };
    if !path.is_file() {
        tracing::debug!(mcp_server = %server_name, "no stored OAuth credentials to forget");
        return Ok(None);
    }
    FileCredentialStore::new(path.clone())
        .clear()
        .await
        .map_err(|error| {
            anyhow!(
                "failed to delete MCP OAuth credentials {}: {error}",
                path.display()
            )
        })?;
    tracing::info!(mcp_server = %server_name, path = %path.display(), "forgot stored OAuth credentials");
    Ok(Some(path))
}

/// File-backed [`CredentialStore`]: one JSON file per MCP server.
///
/// The file is written `0600` on Unix because it holds a bearer token.
#[derive(Debug, Clone)]
pub struct FileCredentialStore {
    path: PathBuf,
}

impl FileCredentialStore {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait]
impl CredentialStore for FileCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        match tokio::fs::read(&self.path).await {
            Ok(bytes) => {
                let credentials = serde_json::from_slice(&bytes).map_err(|error| {
                    AuthError::InternalError(format!(
                        "failed to parse MCP OAuth credentials {}: {error}",
                        self.path.display()
                    ))
                })?;
                tracing::debug!(path = %self.path.display(), "loaded MCP OAuth credentials");
                Ok(Some(credentials))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(AuthError::InternalError(format!(
                "failed to read MCP OAuth credentials {}: {error}",
                self.path.display()
            ))),
        }
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        if let Some(parent) = self.path.parent() {
            create_dir_private(parent).await.map_err(|error| {
                AuthError::InternalError(format!(
                    "failed to create MCP OAuth directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let bytes = serde_json::to_vec_pretty(&credentials).map_err(|error| {
            AuthError::InternalError(format!(
                "failed to serialize MCP OAuth credentials: {error}"
            ))
        })?;
        write_private(&self.path, &bytes).await.map_err(|error| {
            AuthError::InternalError(format!(
                "failed to write MCP OAuth credentials {}: {error}",
                self.path.display()
            ))
        })?;
        restrict_to_owner(&self.path);
        tracing::info!(path = %self.path.display(), "saved MCP OAuth credentials");
        Ok(())
    }

    async fn clear(&self) -> Result<(), AuthError> {
        match tokio::fs::remove_file(&self.path).await {
            Ok(()) => {
                tracing::info!(path = %self.path.display(), "cleared MCP OAuth credentials");
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AuthError::InternalError(format!(
                "failed to remove MCP OAuth credentials {}: {error}",
                self.path.display()
            ))),
        }
    }
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(error) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!(path = %path.display(), error = %error, "failed to restrict MCP OAuth credentials permissions");
    }
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path) {}

/// Creates the credential directory readable only by its owner (Unix).
///
/// The token files inside are `0600`, but a world-listable parent would still
/// expose which servers the user has authorized.
async fn create_dir_private(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut builder = tokio::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(dir).await?;
        // `mode` only applies to directories this call creates, so tighten a
        // pre-existing one too (it is Tact's own directory).
        tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).await?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        tokio::fs::create_dir_all(dir).await
    }
}

/// Writes `bytes` to `path` through a `0600` temp file plus a rename.
///
/// Writing the final path directly would create it with the umask default
/// (`0644` typically) and only narrow it afterwards, leaving a window in which
/// another local user can read the bearer token. The rename also keeps a
/// crashed write from leaving a truncated credential file.
async fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temp).await?;
    if let Err(error) = file.write_all(bytes).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error);
    }
    if let Err(error) = file.flush().await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error);
    }
    drop(file);
    if let Err(error) = tokio::fs::rename(&temp, path).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_required_detection_ignores_unrelated_errors() {
        // A plain error (no rmcp `ClientInitializeError` in the chain) must not
        // be mistaken for an authorization requirement.
        assert!(!is_auth_required_error(&anyhow::anyhow!(
            "connection refused"
        )));
        assert!(!is_auth_required_error(&anyhow::anyhow!(
            "failed to spawn MCP server demo"
        )));

        // A `ClientInitializeError` that is *not* a transport error is equally
        // not an auth problem.
        let cancelled = anyhow::Error::new(ClientInitializeError::Cancelled);
        assert!(!is_auth_required_error(&cancelled));
    }

    #[tokio::test]
    async fn undeclared_auth_still_uses_a_stored_credential() {
        // `/mcp auth` can authorize a server whose config never declared
        // `auth` (the 401 is the evidence). On the next connect the stored
        // token must be picked up.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hosted.json");
        FileCredentialStore::new(path.clone())
            .save(StoredCredentials {
                client_id: "client".into(),
                token_response: None,
                granted_scopes: Vec::new(),
                token_received_at: None,
            })
            .await
            .expect("save");

        // A credential file without a token response is not usable, so the
        // caller falls through to an anonymous connect (and then to the
        // 401-based detection) rather than sending a broken token.
        assert!(
            stored_access_token_at(&path, "hosted", "https://example.invalid/mcp")
                .await
                .expect("probe")
                .is_none()
        );

        // A missing file is likewise "no credential", not an error.
        assert!(
            stored_access_token_at(
                &dir.path().join("absent.json"),
                "hosted",
                "https://example.invalid/mcp"
            )
            .await
            .expect("probe")
            .is_none()
        );
    }

    #[test]
    fn oauth_parameters_default_when_auth_is_not_declared() {
        // The interactive flow reads its knobs from `auth`, but must not
        // require it.
        let undeclared = McpRemoteConfig {
            url: "https://mcp.example.com/mcp".into(),
            headers: HashMap::new(),
            auth: None,
        };
        assert_eq!(
            oauth_parameters(&undeclared),
            OauthRequestParameters {
                client_id: None,
                client_name: None,
                scopes: Vec::new(),
                callback_port: None,
            }
        );

        let declared = McpRemoteConfig {
            url: "https://mcp.example.com/mcp".into(),
            headers: HashMap::new(),
            auth: Some(McpAuthConfig::Oauth {
                client_id: Some("pre-registered".into()),
                client_name: Some("Tact".into()),
                scopes: vec!["read".into()],
                callback_port: Some(41234),
            }),
        };
        let params = oauth_parameters(&declared);
        assert_eq!(params.client_id.as_deref(), Some("pre-registered"));
        assert_eq!(params.scopes, vec!["read".to_string()]);
        assert_eq!(params.callback_port, Some(41234));
        // A per-server name wins over the global default.
        assert_eq!(params.effective_client_name(), "Tact");
    }

    #[test]
    fn the_registration_name_falls_back_to_the_configured_default() {
        // No per-server override: the answer comes from config (or the built-in
        // default when config is not installed, as in this unit test).
        let config = McpRemoteConfig {
            url: "https://mcp.example.com/mcp".into(),
            headers: HashMap::new(),
            auth: None,
        };
        let expected = crate::config::try_settings()
            .map(|settings| settings.mcp.oauth_client_name)
            .unwrap_or_else(|| crate::config::McpSettings::default().oauth_client_name);
        assert_eq!(oauth_parameters(&config).effective_client_name(), expected);

        // A blank override is treated as absent rather than sent to the
        // provider (every provider would reject an empty client_name).
        let blank = McpRemoteConfig {
            url: "https://mcp.example.com/mcp".into(),
            headers: HashMap::new(),
            auth: Some(McpAuthConfig::Oauth {
                client_id: None,
                client_name: Some("   ".into()),
                scopes: Vec::new(),
                callback_port: None,
            }),
        };
        assert_eq!(oauth_parameters(&blank).effective_client_name(), expected);
    }

    #[test]
    fn an_unsafe_server_name_never_derives_a_credential_path() {
        // Guards `logout`: a name that reaches outside the credential
        // directory must be refused, not silently turned into a file path.
        for name in ["../../evil", "sub/dir", "..", ".", "", "a\u{0}b"] {
            assert!(
                oauth_credential_path(name).is_none(),
                "{name:?} produced a credential path"
            );
        }
        for name in ["figma", "linear", "plugin__id__server", "a-b_c.1"] {
            let path = oauth_credential_path(name).expect("safe name yields a path");
            assert!(path.ends_with(format!("{name}.json")), "{path:?}");
        }
    }

    #[tokio::test]
    async fn forgetting_credentials_rejects_an_unsafe_name_before_touching_disk() {
        let error = forget_credentials("../../evil").await.unwrap_err();
        assert!(format!("{error:#}").contains("name"), "{error:#}");
    }

    #[tokio::test]
    async fn forgetting_a_missing_credential_is_not_an_error() {
        // `logout` for a server that was never authorized (or already logged
        // out) must be idempotent, not a failure.
        assert_eq!(
            forget_credentials("tact-mcp-logout-nonexistent")
                .await
                .expect("no stored credential"),
            None
        );
    }

    #[test]
    fn registration_reason_unwraps_rmcps_nested_wrapping() {
        // The exact shape rmcp 0.17 produces for a refused registration.
        assert_eq!(
            registration_reason(
                "Registration failed: Dynamic registration failed: Registration failed: HTTP 403 Forbidden: Forbidden"
            ),
            "HTTP 403 Forbidden: Forbidden"
        );
        // A message with no known prefix is passed through unchanged, so a
        // wording change in rmcp degrades instead of printing nothing.
        assert_eq!(
            registration_reason("something else entirely"),
            "something else entirely"
        );
        assert_eq!(registration_reason("Registration failed: x"), "x");
    }

    #[test]
    fn refused_registration_explains_the_options_not_just_the_status() {
        // Regression guard for the Figma case: a bare "HTTP 403" left the user
        // with no way forward, because the fix is a config change.
        let message = registration_message(
            "figma",
            "https://mcp.figma.com/mcp",
            None,
            "Codex",
            true,
            "Registration failed: Dynamic registration failed: Registration failed: HTTP 403 Forbidden: Forbidden",
        );

        assert!(
            message.contains("OAuth client registration failed for figma"),
            "{message}"
        );
        assert!(
            message.contains("registration failed for figma: HTTP 403 Forbidden: Forbidden"),
            "reason should be unwrapped: {message}"
        );
        assert!(message.contains("the provider refused it"), "{message}");
        // The name actually sent is the thing providers gate on, so the message
        // must state it and how to change it.
        assert!(
            message.contains("registered as \"Codex\""),
            "the registration name should be shown: {message}"
        );
        assert!(
            message.contains("mcp.oauth_client_name") && message.contains("auth.clientName"),
            "the two ways to override the name should be named: {message}"
        );
        // All three escape hatches must be spelled out concretely.
        assert!(message.contains("\"clientId\""), "{message}");
        assert!(
            message.contains("http://127.0.0.1:<port>/callback"),
            "{message}"
        );
        assert!(message.contains("\"headers\""), "{message}");
        // The no-OAuth local route must be reachable from the error itself.
        assert!(
            message.contains("http://127.0.0.1:3845/mcp"),
            "the desktop-server route should be named: {message}"
        );
    }

    #[test]
    fn a_missing_registration_endpoint_does_not_blame_the_client_name() {
        // Without a registration endpoint there is no name to argue about, so
        // the client-name hint would be noise.
        let message = registration_message(
            "no-dcr",
            "https://mcp.example.com/mcp",
            None,
            "Codex",
            false,
            "Registration failed: Dynamic registration failed: Registration failed: Dynamic client registration not supported",
        );
        assert!(
            !message.contains("client name"),
            "no name hint expected: {message}"
        );
    }

    #[test]
    fn missing_registration_endpoint_reads_differently_from_a_refusal() {
        let message = registration_message(
            "no-dcr",
            "https://mcp.example.com/mcp",
            Some(41234),
            "Codex",
            false,
            "Registration failed: Dynamic registration failed: Registration failed: Dynamic client registration not supported",
        );
        assert!(
            message.contains("does not advertise a registration endpoint"),
            "{message}"
        );
        // A pinned port must appear in the redirect URI the user is told to
        // register, since that is the whole point of setting callbackPort.
        assert!(
            message.contains("http://127.0.0.1:41234/callback"),
            "{message}"
        );
    }

    #[test]
    fn loopback_urls_are_recognized_so_they_can_bypass_a_proxy() {
        for url in [
            "http://127.0.0.1:3845/mcp",
            "http://127.0.0.1/mcp",
            "http://127.1.2.3:9000/mcp",
            "http://localhost:3845/mcp",
            "https://localhost/mcp",
            "http://[::1]:3845/mcp",
            "http://user:pass@127.0.0.1:3845/mcp",
            "http://127.0.0.1:3845/mcp?x=1",
        ] {
            assert!(is_loopback_url(url), "{url} should be loopback");
        }
    }

    #[test]
    fn remote_urls_keep_using_the_environment_proxy() {
        // A proxy is what makes these reachable in a restricted network, so
        // they must not be lumped in with loopback.
        for url in [
            "https://mcp.deepwiki.com/mcp",
            "https://mcp.figma.com/mcp",
            "https://mcp.linear.app/mcp",
            "http://192.168.1.10:8080/mcp",
            "http://localhost.evil.com/mcp",
            "http://127.0.0.1.evil.com/mcp",
            "not-a-url",
            "127.0.0.1:3845/mcp",
        ] {
            assert!(!is_loopback_url(url), "{url} should not be loopback");
        }
    }

    #[test]
    fn percent_decode_handles_escapes_and_plus() {
        assert_eq!(percent_decode("a%2Fb+c"), "a/b c");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("%"), "%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn remote_config_parses_url_headers_and_oauth() {
        let config: McpRemoteConfig = serde_json::from_str(
            r#"{
                "url": "https://mcp.example.com/mcp",
                "headers": { "X-Api-Key": "secret" },
                "auth": { "type": "oauth", "scopes": ["tools.read"], "callbackPort": 41234 }
            }"#,
        )
        .expect("remote config parses");
        assert_eq!(config.url, "https://mcp.example.com/mcp");
        assert_eq!(
            config.headers.get("X-Api-Key").map(String::as_str),
            Some("secret")
        );
        match config.auth {
            Some(McpAuthConfig::Oauth {
                client_id,
                client_name,
                scopes,
                callback_port,
            }) => {
                assert!(client_id.is_none());
                assert!(
                    client_name.is_none(),
                    "clientName must be optional and absent here"
                );
                assert_eq!(scopes, vec!["tools.read".to_string()]);
                assert_eq!(callback_port, Some(41234));
            }
            other => panic!("expected oauth auth, got {other:?}"),
        }
    }

    #[test]
    fn invalid_header_names_are_dropped_from_the_transport_config() {
        let config = McpRemoteConfig {
            url: "https://mcp.example.com/mcp".into(),
            headers: HashMap::from([
                ("X-Good".to_string(), "1".to_string()),
                ("Bad Header".to_string(), "2".to_string()),
            ]),
            auth: None,
        };
        let transport = config.transport_config("remote", &RemoteAuthState::None);
        assert_eq!(transport.custom_headers.len(), 1);
        assert!(transport.auth_header.is_none());
    }

    #[test]
    fn oauth_token_becomes_the_bearer_auth_header() {
        let config = McpRemoteConfig {
            url: "https://mcp.example.com/mcp".into(),
            headers: HashMap::from([("Authorization".to_string(), "Bearer stale".to_string())]),
            auth: None,
        };
        let transport = config.transport_config("remote", &RemoteAuthState::Token("fresh".into()));
        assert_eq!(transport.auth_header.as_deref(), Some("fresh"));
        // The stale static header must not shadow the OAuth token.
        assert!(transport.custom_headers.is_empty());
    }

    #[tokio::test]
    async fn file_credential_store_round_trips_and_clears() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FileCredentialStore::new(dir.path().join("server.json"));
        assert!(store.load().await.expect("load empty").is_none());

        let credentials = StoredCredentials {
            client_id: "client".into(),
            token_response: None,
            granted_scopes: vec!["tools.read".into()],
            token_received_at: Some(1),
        };
        store.save(credentials).await.expect("save");
        let loaded = store.load().await.expect("load").expect("present");
        assert_eq!(loaded.client_id, "client");
        assert_eq!(loaded.granted_scopes, vec!["tools.read".to_string()]);

        store.clear().await.expect("clear");
        assert!(store.load().await.expect("load after clear").is_none());
        store.clear().await.expect("clear is idempotent");
    }

    #[tokio::test]
    async fn callback_listener_extracts_code_and_state() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let client = async {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("connect");
            stream
                .write_all(b"GET /callback?code=abc%2F1&state=xyz HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .expect("write");
            let mut response = String::new();
            let _ = stream.read_to_string(&mut response).await;
            assert!(response.contains("200 OK"), "response: {response}");
        };
        let (callback, ()) = tokio::join!(
            await_oauth_callback(listener, Duration::from_secs(5)),
            client
        );
        let (code, state) = callback.expect("callback");
        assert_eq!(code, "abc/1");
        assert_eq!(state, "xyz");
    }

    #[tokio::test]
    async fn callback_listener_surfaces_denied_authorization() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let client = async {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("connect");
            let _ = stream
                .write_all(b"GET /callback?error=access_denied HTTP/1.1\r\n\r\n")
                .await;
        };
        let (result, ()) = tokio::join!(
            await_oauth_callback(listener, Duration::from_secs(5)),
            client
        );
        let error = result.expect_err("denied");
        assert!(error.to_string().contains("access_denied"), "{error}");
    }

    #[tokio::test]
    async fn callback_listener_times_out_without_a_request() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let error = await_oauth_callback(listener, Duration::from_millis(50))
            .await
            .expect_err("timeout");
        assert!(error.to_string().contains("timed out"), "{error}");
    }

    /// Slicing the query by byte offsets used to panic when a `%` escape ended
    /// inside a multi-byte character; any local process can send that.
    #[test]
    fn malformed_percent_escapes_degrade_instead_of_panicking() {
        assert_eq!(percent_decode("x=%√√"), "x=%√√");
        assert_eq!(percent_decode("%aé"), "%aé");
        assert_eq!(percent_decode("%éé"), "%éé");
        assert_eq!(percent_decode("%"), "%");
        assert_eq!(percent_decode("%z"), "%z");
    }

    #[test]
    fn the_csrf_state_is_redacted_before_logging() {
        let url =
            "https://id.example.com/authorize?response_type=code&state=secret-nonce&client_id=abc";
        let redacted = redact_query_value(url, "state");
        assert!(!redacted.contains("secret-nonce"), "{redacted}");
        assert!(redacted.contains("state=[redacted]"), "{redacted}");
        // The parts worth troubleshooting must survive.
        assert!(redacted.contains("client_id=abc"), "{redacted}");
        assert!(redacted.contains("response_type=code"), "{redacted}");

        // A URL without a query, or without the parameter, is returned as-is.
        assert_eq!(redact_query_value("https://x/y", "state"), "https://x/y");
        assert_eq!(
            redact_query_value("https://x/y?a=1", "state"),
            "https://x/y?a=1"
        );
    }

    #[tokio::test]
    async fn a_malformed_escape_on_the_callback_path_is_not_fatal() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let client = async {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("connect");
            stream
                .write_all("GET /callback?code=%aé&state=x HTTP/1.1\r\n\r\n".as_bytes())
                .await
                .expect("write");
            let mut response = String::new();
            let _ = stream.read_to_string(&mut response).await;
        };
        let (callback, ()) = tokio::join!(
            await_oauth_callback(listener, Duration::from_secs(5)),
            client
        );
        let (code, state) = callback.expect("callback");
        // The escape is preserved literally rather than decoded, and the flow
        // continues instead of aborting the task.
        assert_eq!(code, "%aé");
        assert_eq!(state, "x");
    }

    /// A browser prefetch or a second tab used to consume the single accepted
    /// connection and fail the real redirect.
    #[tokio::test]
    async fn a_stray_connection_before_the_callback_is_ignored() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let client = async {
            let mut stray = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("connect");
            stray
                .write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .expect("write");
            let mut response = String::new();
            let _ = stray.read_to_string(&mut response).await;
            assert!(response.contains("404"), "stray response: {response}");

            // A hand visit to the registered path without a code must also be
            // survivable.
            let mut hand_visit = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("connect");
            hand_visit
                .write_all(b"GET /callback HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .expect("write");
            let mut response = String::new();
            let _ = hand_visit.read_to_string(&mut response).await;
            assert!(response.contains("400"), "hand visit response: {response}");

            let mut real = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("connect");
            real.write_all(b"GET /callback?code=abc&state=xyz HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .expect("write");
            let mut response = String::new();
            let _ = real.read_to_string(&mut response).await;
            assert!(response.contains("200 OK"), "callback response: {response}");
        };
        let (callback, ()) = tokio::join!(
            await_oauth_callback(listener, Duration::from_secs(5)),
            client
        );
        assert_eq!(
            callback.expect("callback"),
            ("abc".to_string(), "xyz".to_string())
        );
    }

    /// A request head split across TCP segments must be reassembled, not
    /// parsed from a partial first read.
    #[tokio::test]
    async fn a_fragmented_request_head_is_reassembled() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let client = async {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("connect");
            stream
                .write_all(b"GET /callback?code=ab")
                .await
                .expect("first fragment");
            stream.flush().await.expect("flush");
            tokio::time::sleep(Duration::from_millis(50)).await;
            stream
                .write_all(b"c&state=xyz HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .expect("second fragment");
            let mut response = String::new();
            let _ = stream.read_to_string(&mut response).await;
            assert!(response.contains("200 OK"), "response: {response}");
        };
        let (callback, ()) = tokio::join!(
            await_oauth_callback(listener, Duration::from_secs(5)),
            client
        );
        assert_eq!(
            callback.expect("callback"),
            ("abc".to_string(), "xyz".to_string())
        );
    }

    /// `error_description` is the actionable half of a denial.
    #[tokio::test]
    async fn a_denial_reports_the_error_description() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let client = async {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("connect");
            stream
                .write_all(
                    b"GET /callback?error=access_denied&error_description=User+said+no HTTP/1.1\r\n\r\n",
                )
                .await
                .expect("write");
            let mut response = String::new();
            let _ = stream.read_to_string(&mut response).await;
        };
        let (result, ()) = tokio::join!(
            await_oauth_callback(listener, Duration::from_secs(5)),
            client
        );
        let text = format!("{:#}", result.expect_err("denied"));
        assert!(text.contains("access_denied"), "{text}");
        assert!(text.contains("User said no"), "{text}");
    }
}
