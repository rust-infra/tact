//! `tact-ui mcp` — inspect, configure and authorize MCP servers.
//!
//! `list`, `get`, `add`, `remove`, `login` and `logout` are CLI-only: headless
//! users have no TUI, so this is their entry point for everything except
//! interactive authorization, which the TUI also exposes as
//! `/mcp auth <server>`. Both paths call the same `tact::mcp` functions.
//!
//! Command roles are deliberately separated: `list`/`get` are the only ones
//! that connect, `add`/`remove` are the only ones that write `mcp.json`, and
//! `login`/`logout` are the only ones that touch stored credentials. A command
//! that creates state never silently connects, and a command that inspects
//! never writes.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use tact::{
    config::McpSubcommand,
    mcp::{
        self, McpConfigScope, McpDraftTransport, McpLoadReport, McpServerDraft, McpServerStatus,
    },
};

/// Runs an `mcp` subcommand and prints the result to stdout.
pub async fn run_mcp_cli(command: McpSubcommand) -> Result<()> {
    match command {
        McpSubcommand::List => list_servers().await,
        McpSubcommand::Get { name } => get_server(&name).await,
        McpSubcommand::Add {
            name,
            url,
            command,
            args,
            env,
            header,
            oauth,
            user,
            force,
        } => add(AddArgs {
            name,
            url,
            command,
            args,
            env,
            header,
            oauth,
            user,
            force,
        }),
        McpSubcommand::Remove { name, user } => remove(&name, user),
        McpSubcommand::Login { server } => authorize(&server).await,
        McpSubcommand::Logout { server } => logout(&server).await,
    }
}

/// The `mcp add` arguments, grouped so the handler stays readable.
struct AddArgs {
    name: String,
    url: Option<String>,
    command: Option<String>,
    args: Vec<String>,
    env: Vec<String>,
    header: Vec<String>,
    oauth: bool,
    user: bool,
    force: bool,
}

/// Writes one server declaration, then prints the file and the new entry.
///
/// Deliberately does **not** connect: `add` only edits configuration, and a
/// connect would spawn processes and hit the network for servers the user may
/// not want to run yet. `mcp list` verifies.
fn add(args: AddArgs) -> Result<()> {
    let draft = draft_from_args(&args)?;
    let scope = if args.user {
        McpConfigScope::User
    } else {
        McpConfigScope::Project
    };
    let workdir = std::env::current_dir().context("failed to resolve the current directory")?;

    let outcome = mcp::add_mcp_server(&workdir, scope, &draft, args.force)?;

    let verb = if outcome.replaced {
        "Replaced"
    } else {
        "Added"
    };
    println!(
        "{verb} MCP server '{}' in {}",
        args.name,
        outcome.path.display()
    );
    println!("\n  {}  {}", args.name, draft.transport_kind());

    // A declaration the loader will not reach is a silent no-op from the
    // user's point of view, so say which file actually wins.
    if let Ok(Some((resolved, _))) = mcp::resolved_server_for(&args.name)
        && resolved.source != outcome.path.display().to_string()
    {
        println!(
            "\nWarning: '{}' is also declared in {}, which takes precedence — \
             this change will not take effect until that declaration is removed.",
            args.name, resolved.source
        );
    }

    if args.oauth {
        println!("\nAuthorize it with: tact-ui mcp login {}", args.name);
    }
    println!("\nVerify with: tact-ui mcp list");
    Ok(())
}

/// Removes one declaration, explaining where the server actually lives if the
/// target file does not have it.
fn remove(name: &str, user: bool) -> Result<()> {
    mcp::validate_server_name(name)?;
    let scope = scope_of(user);
    let workdir = std::env::current_dir().context("failed to resolve the current directory")?;

    let outcome = match mcp::remove_mcp_server(&workdir, scope, name) {
        Ok(outcome) => outcome,
        // The common mistakes are removing from the wrong scope and trying to
        // remove a plugin-contributed server (which no file can delete). Fold
        // the explanation into the message instead of stacking `anyhow`
        // contexts, which would render as `Error: : …` when empty.
        Err(error) => bail!("{error:#}{}", scope_hint(&workdir, scope, name)),
    };

    println!(
        "Removed MCP server '{name}' from {}",
        outcome.path.display()
    );
    match tact::mcp::oauth_credential_path(name) {
        Some(path) if path.is_file() => println!(
            "\nStored credentials were kept ({}). Run `tact-ui mcp logout {name}` to delete them.",
            path.display()
        ),
        _ => {}
    }
    Ok(())
}

/// The scope selected by the `--user` flag.
fn scope_of(user: bool) -> McpConfigScope {
    if user {
        McpConfigScope::User
    } else {
        McpConfigScope::Project
    }
}

/// Explains a failed `remove` when the server is declared somewhere else.
///
/// Returns an empty string when there is nothing extra to say, so it can be
/// used as `anyhow` context unconditionally.
fn scope_hint(workdir: &Path, scope: McpConfigScope, name: &str) -> String {
    let Ok(Some((server, _))) = mcp::resolved_server_for(name) else {
        return String::new();
    };
    let source = &server.source;
    let is = |path: &Result<std::path::PathBuf>| {
        path.as_ref()
            .is_ok_and(|path| source == &path.display().to_string())
    };

    if is(&McpConfigScope::User.path(workdir)) {
        if scope == McpConfigScope::User {
            // Already editing that file, so pointing at it again would misdirect.
            return String::new();
        }
        return format!("\n'{name}' is declared in the user config — retry with `--user`.");
    }
    if is(&McpConfigScope::Project.path(workdir)) {
        if scope == McpConfigScope::Project {
            return String::new();
        }
        return format!("\n'{name}' is declared in the project config — retry without `--user`.");
    }
    format!(
        "\n'{name}' is contributed by a source that cannot be edited here ({source}); \
         uninstall the plugin that provides it instead."
    )
}

/// Connects one server only and prints the full detail view.
async fn get_server(name: &str) -> Result<()> {
    mcp::validate_server_name(name)?;
    let inspection = mcp::inspect_server(name)
        .await
        .with_context(|| format!("failed to inspect MCP server '{name}'"))?
        .with_context(|| {
            format!("no MCP server named '{name}' is configured (see `tact-ui mcp list`)")
        })?;

    println!("{}", render_server_detail(&inspection));

    if matches!(inspection.status, McpServerStatus::PendingAuthorization) {
        println!("\nAuthorize it with: tact-ui mcp login {name}");
    }
    Ok(())
}

/// Renders the single-server view.
///
/// Split from printing so the layout is unit-testable without capturing stdout.
#[must_use]
pub fn render_server_detail(inspection: &mcp::McpServerInspection) -> String {
    let name = &inspection.server.name;
    let mut lines = vec![
        format!("{name}  {}", inspection.server.transport),
        format!("  source  {}", inspection.server.source),
        format!("  status  {}", status_text(inspection)),
    ];
    if inspection.tools.is_empty() {
        if matches!(inspection.status, McpServerStatus::Connected) {
            // A connected server with no tools is legal but almost always a
            // mistake worth naming.
            lines.push("  tools   (none — the server reports no tools)".to_string());
        }
    } else {
        lines.push(format!("  tools   {} available:", inspection.tools.len()));
        for tool in &inspection.tools {
            // The full name is what the agent must call, so show it verbatim.
            lines.push(format!("            mcp__{name}__{tool}"));
        }
    }
    lines.join("\n")
}

/// Deletes the stored OAuth credentials for one server.
async fn logout(server: &str) -> Result<()> {
    let removed = mcp::forget_credentials(server)
        .await
        .with_context(|| format!("failed to clear credentials for '{server}'"))?;
    match removed {
        Some(path) => {
            println!("Logged out of '{server}' (deleted {}).", path.display());
            println!("\nThe next connection will need: tact-ui mcp login {server}");
        }
        None => println!(
            "No stored credentials for '{server}' — nothing to delete.\n\
             (Credentials live at ~/.tact/mcp/oauth/<server>.json.)"
        ),
    }
    Ok(())
}

/// Turns the parsed flags into a validated draft.
///
/// Split from [`add`] so the flag mapping is testable without touching the
/// working directory.
fn draft_from_args(args: &AddArgs) -> Result<McpServerDraft> {
    let transport = match (&args.url, &args.command) {
        (Some(url), None) => McpDraftTransport::Remote {
            url: url.clone(),
            headers: parse_pairs(&args.header, ':', "--header", "NAME:VALUE")?,
            oauth: args.oauth,
        },
        (None, Some(command)) => McpDraftTransport::Stdio {
            command: command.clone(),
            args: args.args.clone(),
            env: parse_pairs(&args.env, '=', "--env", "NAME=VALUE")?,
        },
        // clap enforces `--url` XOR `--command`; this only guards a future
        // caller that bypasses the parser.
        _ => bail!("pass exactly one of --url (remote) or --command (stdio)"),
    };

    // Validation lives in `McpServerDraft::new` so the CLI and any other
    // caller reject the same inputs with the same messages.
    McpServerDraft::new(args.name.clone(), transport)
}

/// Parses repeated `NAME<separator>VALUE` CLI flags into a map.
///
/// Values are never echoed in an error, because `--header` and `--env` are the
/// two places a user is most likely to put a secret. Note that the value is
/// still visible in the process arguments (`ps`, `/proc/<pid>/cmdline`) and in
/// shell history — for a secret, prefer a file the server can read via the
/// environment, or a static token kept out of the command line.
fn parse_pairs(
    values: &[String],
    separator: char,
    flag: &str,
    expected: &str,
) -> Result<BTreeMap<String, String>> {
    let mut pairs = BTreeMap::new();
    for value in values {
        let Some((name, pair_value)) = value.split_once(separator) else {
            bail!("{flag} value is not in {expected} form (no '{separator}')");
        };
        let name = name.trim();
        if name.is_empty() {
            bail!("{flag} value is missing a name before '{separator}'");
        }
        // Repeating a name would silently keep only the last value; that is
        // exactly the kind of quiet surprise a credential flag should not have.
        if pairs
            .insert(name.to_owned(), pair_value.trim().to_owned())
            .is_some()
        {
            bail!("{flag} {name} was given more than once");
        }
    }
    Ok(pairs)
}

/// Connects every configured server and prints one line per server.
///
/// Connects exactly like startup does, so the output reflects what the agent
/// would actually see — a server that fails here fails there too.
async fn list_servers() -> Result<()> {
    let (router, report) = mcp::load_mcp_router_with_report()
        .await
        .context("failed to resolve MCP configuration")?;

    println!("{}", render_report(&report));
    println!(
        "\n{} tool(s) available from {} server(s).",
        router.all_tools().len(),
        router.server_summaries().len()
    );
    Ok(())
}

/// Renders the report as a human-readable table.
///
/// Kept separate from the printing so the layout is unit-testable without
/// capturing stdout.
#[must_use]
pub fn render_report(report: &McpLoadReport) -> String {
    if report.configured.is_empty() && report.skipped_remote.is_empty() {
        return "No MCP servers configured.\n\n\
                Declare servers in ~/.tact/mcp.json (user) or .tact/mcp.json (project):\n\
                \x20 { \"mcpServers\": { \"my-server\": { \"command\": \"...\" } } }\n\
                \x20 { \"mcpServers\": { \"remote\": { \"url\": \"https://.../mcp\" } } }"
            .to_string();
    }

    let width = report
        .configured
        .iter()
        .map(|s| s.name.len())
        .chain(report.skipped_remote.iter().map(String::len))
        .max()
        .unwrap_or(0);

    let mut lines = Vec::new();
    for server in &report.configured {
        let status = status_for(report, &server.name);
        lines.push(format!(
            "  {:<width$}  {:<34}  {}",
            server.name,
            server.transport,
            status,
            width = width
        ));
    }
    for name in &report.skipped_remote {
        lines.push(format!(
            "  {name:<width$}  {:<34}  skipped (no usable command or url)",
            "-",
            width = width
        ));
    }

    let mut out = String::new();
    out.push_str(&format!(
        "{} MCP server(s) configured:\n",
        report.configured.len() + report.skipped_remote.len()
    ));
    out.push_str(&lines.join("\n"));

    // Overrides are invisible in the table above (only the winner appears), yet
    // they are exactly what makes `remove` change behavior — so say them out
    // loud, naming the file that actually wins.
    if !report.shadowed.is_empty() {
        let mut notes = Vec::new();
        for (name, displaced) in &report.shadowed {
            let winner = report
                .configured
                .iter()
                .find(|server| &server.name == name)
                .map_or("<unknown>", |server| server.source.as_str());
            notes.push(format!("  {name}  {displaced} is shadowed by {winner}"));
        }
        out.push_str("\n\nOverridden declarations:\n");
        out.push_str(&notes.join("\n"));
    }
    out
}

/// The status cell for one server, chosen from the problem lists.
fn status_for(report: &McpLoadReport, name: &str) -> String {
    if let Some((_, tools)) = report.connected.iter().find(|(n, _)| n == name) {
        return connected_text(*tools);
    }
    if report.pending_auth.iter().any(|n| n == name) {
        return needs_auth_text(name);
    }
    if let Some((_, error)) = report.failures.iter().find(|(n, _)| n == name) {
        return failed_text(error);
    }
    // Resolution kept it but no outcome was recorded: treat as failed rather
    // than silently implying it works.
    "unknown".to_string()
}

/// Status wording, shared by `list` and `get` so the two views cannot drift.
fn connected_text(tools: usize) -> String {
    format!("connected ({tools} tools)")
}

fn needs_auth_text(name: &str) -> String {
    format!("needs authorization — run `tact-ui mcp login {name}`")
}

fn failed_text(error: &str) -> String {
    format!("failed: {error}")
}

/// The status line for the single-server view.
fn status_text(inspection: &mcp::McpServerInspection) -> String {
    match &inspection.status {
        McpServerStatus::Connected => connected_text(inspection.tools.len()),
        McpServerStatus::PendingAuthorization => needs_auth_text(&inspection.server.name),
        McpServerStatus::Failed(error) => failed_text(error),
    }
}

/// Runs the interactive OAuth flow, printing the URL for the user to open.
async fn authorize(server: &str) -> Result<()> {
    // Fail before opening a browser flow for a name that is not configured.
    let config = mcp::remote_config_for(server)?.with_context(|| {
        format!("no remote MCP server named '{server}' is configured (see `tact-ui mcp list`)")
    })?;

    let oauth_declared = matches!(config.auth, Some(tact::mcp::McpAuthConfig::Oauth { .. }));
    if !oauth_declared {
        eprintln!(
            "Note: '{server}' does not declare `auth` in mcp.json. \
             Authorizing anyway — the server's 401 is what requires it."
        );
    }

    let mut printed_url = false;
    let mut notify = |line: &str| {
        printed_url = true;
        println!("\nOpen this URL in your browser to authorize '{server}':\n\n  {line}\n");
        println!("Waiting for the redirect (Ctrl-C to abort)...");
    };

    mcp::authorize_server(server, &mut notify)
        .await
        .with_context(|| format!("authorization failed for '{server}'"))?;

    if !printed_url {
        // Defensive: a successful flow always reports a URL first.
        eprintln!("Warning: no authorization URL was reported for '{server}'.");
    }
    println!("\nAuthorized '{server}'. Credentials saved.");

    // Show the server's new state so the user immediately sees it working
    // (and does not have to re-run `list` to find out).
    let (_router, report) = mcp::load_mcp_router_with_report().await?;
    println!("\n{}", render_report(&report));

    let status = status_for(&report, server);
    if !status.starts_with("connected") {
        anyhow::bail!("authorized, but '{server}' is still not connected: {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn configured(name: &str, oauth: bool) -> mcp::ConfiguredServer {
        mcp::ConfiguredServer {
            name: name.to_string(),
            transport: mcp::McpTransportKind::Remote {
                url: format!("https://example.invalid/{name}"),
                oauth,
            },
            source: "~/.tact/mcp.json".to_string(),
        }
    }

    #[test]
    fn empty_report_explains_how_to_configure() {
        let text = render_report(&McpLoadReport::default());
        assert!(text.contains("No MCP servers configured."), "{text}");
        assert!(text.contains("~/.tact/mcp.json"), "{text}");
        assert!(text.contains("mcpServers"), "{text}");
    }

    #[test]
    fn renders_each_server_with_its_status() {
        let report = McpLoadReport {
            configured: vec![
                configured("ok-server", false),
                configured("needs-auth", true),
                configured("broken", false),
            ],
            connected: vec![("ok-server".to_string(), 3)],
            pending_auth: vec!["needs-auth".to_string()],
            failures: vec![("broken".to_string(), "connection refused".to_string())],
            ..McpLoadReport::default()
        };

        let text = render_report(&report);
        assert!(text.contains("3 MCP server(s) configured:"), "{text}");
        assert!(text.contains("connected (3 tools)"), "{text}");
        assert!(
            text.contains("needs authorization — run `tact-ui mcp login needs-auth`"),
            "{text}"
        );
        assert!(text.contains("failed: connection refused"), "{text}");
        // The remote URL and oauth marker must be visible.
        assert!(
            text.contains("https://example.invalid/needs-auth (oauth)"),
            "{text}"
        );
    }

    #[test]
    fn skipped_servers_are_listed_even_though_they_are_not_configured() {
        let report = McpLoadReport {
            configured: vec![configured("ok-server", false)],
            connected: vec![("ok-server".to_string(), 1)],
            skipped_remote: vec!["typo".to_string()],
            ..McpLoadReport::default()
        };

        let text = render_report(&report);
        assert!(text.contains("2 MCP server(s) configured:"), "{text}");
        assert!(
            text.contains("skipped (no usable command or url)"),
            "{text}"
        );
        assert!(text.contains("typo"), "{text}");
    }

    #[test]
    fn a_server_with_no_recorded_outcome_is_not_reported_as_healthy() {
        // Guards against a resolution/connection mismatch silently looking OK.
        let report = McpLoadReport {
            configured: vec![configured("mystery", false)],
            ..McpLoadReport::default()
        };
        assert_eq!(status_for(&report, "mystery"), "unknown");
    }

    fn add_args(name: &str) -> AddArgs {
        AddArgs {
            name: name.to_owned(),
            url: None,
            command: None,
            args: Vec::new(),
            env: Vec::new(),
            header: Vec::new(),
            oauth: false,
            user: false,
            force: false,
        }
    }

    #[test]
    fn a_url_becomes_a_remote_draft_with_headers_and_oauth() {
        let args = AddArgs {
            url: Some("https://mcp.figma.com/mcp".into()),
            header: vec!["Authorization: Bearer sekret".into()],
            oauth: true,
            ..add_args("figma")
        };

        let draft = draft_from_args(&args).unwrap();
        assert_eq!(
            draft.transport,
            McpDraftTransport::Remote {
                url: "https://mcp.figma.com/mcp".into(),
                headers: BTreeMap::from([("Authorization".to_owned(), "Bearer sekret".to_owned())]),
                oauth: true,
            }
        );
    }

    #[test]
    fn a_command_becomes_a_stdio_draft_with_args_and_env() {
        let args = AddArgs {
            command: Some("uvx".into()),
            args: vec!["basic-memory".into(), "mcp".into()],
            env: vec!["LOG_LEVEL=debug".into()],
            ..add_args("basic-memory")
        };

        let draft = draft_from_args(&args).unwrap();
        assert_eq!(
            draft.transport,
            McpDraftTransport::Stdio {
                command: "uvx".into(),
                args: vec!["basic-memory".into(), "mcp".into()],
                env: BTreeMap::from([("LOG_LEVEL".to_owned(), "debug".to_owned())]),
            }
        );
    }

    #[test]
    fn overridden_declarations_name_the_file_that_wins() {
        // `remove` exists to change which declaration wins, so the losing one
        // must be visible — otherwise its effect is a silent surprise.
        let report = McpLoadReport {
            configured: vec![mcp::ConfiguredServer {
                name: "shared".to_string(),
                transport: mcp::McpTransportKind::Stdio {
                    command: "/bin/project".to_string(),
                },
                source: "/proj/.tact/mcp.json".to_string(),
            }],
            shadowed: vec![("shared".to_string(), "/home/me/.tact/mcp.json".to_string())],
            ..McpLoadReport::default()
        };

        let text = render_report(&report);
        assert!(text.contains("Overridden declarations:"), "{text}");
        assert!(
            text.contains("/home/me/.tact/mcp.json is shadowed by /proj/.tact/mcp.json"),
            "{text}"
        );
    }

    #[test]
    fn a_report_without_overrides_has_no_override_section() {
        let report = McpLoadReport {
            configured: vec![configured("solo", false)],
            connected: vec![("solo".to_string(), 1)],
            ..McpLoadReport::default()
        };
        assert!(
            !render_report(&report).contains("Overridden"),
            "{}",
            render_report(&report)
        );
    }

    #[test]
    fn detail_view_shows_transport_source_status_and_qualified_tool_names() {
        let inspection = mcp::McpServerInspection {
            server: configured("figma", false),
            status: McpServerStatus::Connected,
            tools: vec!["get_file".into(), "list_files".into()],
        };

        let text = render_server_detail(&inspection);
        assert!(
            text.contains("figma  remote https://example.invalid/figma"),
            "{text}"
        );
        assert!(text.contains("source  ~/.tact/mcp.json"), "{text}");
        assert!(text.contains("status  connected (2 tools)"), "{text}");
        // Full names are what the agent must call, so they are qualified here.
        assert!(text.contains("mcp__figma__get_file"), "{text}");
        assert!(text.contains("mcp__figma__list_files"), "{text}");
    }

    #[test]
    fn detail_view_reuses_the_list_wording_for_pending_and_failed_servers() {
        // Both views must phrase a status identically, or a user comparing
        // them has to guess whether they describe the same state.
        let pending = mcp::McpServerInspection {
            server: configured("linear", true),
            status: McpServerStatus::PendingAuthorization,
            tools: Vec::new(),
        };
        let report = McpLoadReport {
            configured: vec![configured("linear", true)],
            pending_auth: vec!["linear".to_string()],
            ..McpLoadReport::default()
        };
        let detail_status = status_text(&pending);
        assert_eq!(detail_status, status_for(&report, "linear"));
        assert!(
            detail_status.contains("tact-ui mcp login linear"),
            "{detail_status}"
        );

        let failed = mcp::McpServerInspection {
            server: configured("broken", false),
            status: McpServerStatus::Failed("connection refused".into()),
            tools: Vec::new(),
        };
        let report = McpLoadReport {
            configured: vec![configured("broken", false)],
            failures: vec![("broken".to_string(), "connection refused".to_string())],
            ..McpLoadReport::default()
        };
        assert_eq!(status_text(&failed), status_for(&report, "broken"));
    }

    #[test]
    fn detail_view_never_prints_an_empty_tool_list_as_success() {
        let connected = mcp::McpServerInspection {
            server: configured("quiet", false),
            status: McpServerStatus::Connected,
            tools: Vec::new(),
        };
        let text = render_server_detail(&connected);
        assert!(text.contains("reports no tools"), "{text}");
    }

    #[test]
    fn scope_of_maps_the_user_flag() {
        assert_eq!(scope_of(false), McpConfigScope::Project);
        assert_eq!(scope_of(true), McpConfigScope::User);
    }

    #[test]
    fn neither_or_both_transports_are_rejected() {
        assert!(draft_from_args(&add_args("bare")).is_err());

        let both = AddArgs {
            url: Some("https://example.invalid/mcp".into()),
            command: Some("/bin/echo".into()),
            ..add_args("ambiguous")
        };
        let message = format!("{:#}", draft_from_args(&both).unwrap_err());
        assert!(message.contains("exactly one"), "{message}");
    }

    #[test]
    fn malformed_pairs_fail_without_echoing_the_value() {
        let bad_header = AddArgs {
            url: Some("https://example.invalid/mcp".into()),
            header: vec!["Authorization Bearer sekret".into()],
            ..add_args("figma")
        };
        let message = format!("{:#}", draft_from_args(&bad_header).unwrap_err());
        assert!(message.contains("--header"), "{message}");
        assert!(!message.contains("sekret"), "secret leaked: {message}");

        let bad_env = AddArgs {
            command: Some("/bin/echo".into()),
            env: vec!["LOG_LEVEL:debug".into()],
            ..add_args("local")
        };
        let message = format!("{:#}", draft_from_args(&bad_env).unwrap_err());
        assert!(message.contains("--env"), "{message}");
    }

    #[test]
    fn pair_parsing_trims_the_name_and_value() {
        let pairs = parse_pairs(
            &["X-Api-Key:  spaced out  ".to_owned()],
            ':',
            "--header",
            "NAME:VALUE",
        )
        .unwrap();
        assert_eq!(
            pairs.get("X-Api-Key").map(String::as_str),
            Some("spaced out")
        );

        // A value containing the separator keeps everything after the first.
        let pairs = parse_pairs(&["A=b=c".to_owned()], '=', "--env", "NAME=VALUE").unwrap();
        assert_eq!(pairs.get("A").map(String::as_str), Some("b=c"));
    }

    #[test]
    fn a_repeated_pair_name_is_rejected() {
        // Silently keeping the last value would let a stale header shadow the
        // intended one with no hint, so a repeat is an error.
        let error = parse_pairs(
            &["X-Api-Key:first".to_owned(), "X-Api-Key:second".to_owned()],
            ':',
            "--header",
            "NAME:VALUE",
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("--header X-Api-Key"), "{message}");
        assert!(message.contains("more than once"), "{message}");
        // Neither value may be echoed: a header is where a secret lives.
        assert!(!message.contains("first"), "value leaked: {message}");
        assert!(!message.contains("second"), "value leaked: {message}");
    }
}
