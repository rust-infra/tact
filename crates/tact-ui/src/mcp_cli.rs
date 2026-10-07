//! `tact-ui mcp` — inspect, configure and authorize MCP servers.
//!
//! `list`, `get`, `add`, `remove`, `login` and `logout` are CLI-only: headless
//! users have no TUI, so this is their entry point for everything except
//! interactive authorization, which the TUI also exposes as
//! `/mcp auth <server>`. Both paths call the same `tact::mcp` functions.
//!
//! Command roles are deliberately separated: `list`/`get` are the only ones
//! that connect, `add`/`remove` are the only ones that write `.mcp.json`, and
//! `login`/`logout` are the only ones that touch stored credentials. A command
//! that creates state never silently connects, and a command that inspects
//! never writes.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use tact::{
    config::McpSubcommand,
    mcp::{
        self, McpConfigScope, McpDraftTransport, McpLiveStatus, McpLoadReport, McpServerDraft,
        McpServerStatus,
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
    if let Ok(Some((resolved, _, _))) = mcp::resolved_server_for(&args.name)
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
    let Ok(Some((server, _, _))) = mcp::resolved_server_for(name) else {
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
    // A Claude Code project file is a third source a user can edit, even though
    // `add`/`remove` never write it; pointing at the plugin path below would
    // send them looking for a plugin that does not exist. Matched by full path
    // rather than by suffix: Tact's own project file, `.tact/.mcp.json`, also
    // ends in `.mcp.json` and is handled above.
    if source == &workdir.join(".mcp.json").display().to_string() {
        return format!(
            "\n'{name}' is declared in {source}, a Claude Code project file — \
             edit that file, or declare it in .tact/.mcp.json to override it."
        );
    }
    format!(
        "\n'{name}' is contributed by a source that cannot be edited here ({source}); \
         uninstall the plugin that provides it instead."
    )
}

/// Connects one server only and prints the full detail view.
async fn get_server(name: &str) -> Result<()> {
    mcp::validate_server_name(name)?;
    // `list` shows a plugin server short, so `get` must accept that form too.
    // The detail view then prints the canonical name, which is what the
    // `mcp__<server>__<tool>` prefix is built from.
    let name = mcp::resolve_server_name(name)?;
    let display = mcp::display_server_name(&name);
    let inspection = mcp::inspect_server(&name)
        .await
        .with_context(|| format!("failed to inspect MCP server '{display}'"))?
        .with_context(|| {
            format!("no MCP server named '{display}' is configured (see `tact-ui mcp list`)")
        })?;

    println!("{}", render_server_detail(&inspection));

    if matches!(inspection.status, McpServerStatus::PendingAuthorization) {
        println!("\nAuthorize it with: tact-ui mcp login {display}");
    }
    Ok(())
}

/// Renders the single-server view.
///
/// Split from printing so the layout is unit-testable without capturing stdout.
#[must_use]
pub fn render_server_detail(inspection: &mcp::McpServerInspection) -> String {
    // The heading shows the short form a plugin server is listed under; the tool
    // lines below keep the full name, because that is the prefix the agent must
    // call. `source` says which plugin the server came from.
    let full_name = &inspection.server.name;
    let name = mcp::display_server_name(full_name);
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
        // Every request re-declares these tools, so the count alone is a poor
        // description of the cost; the sum is what `enabled_tools` trades away.
        let total: usize = inspection.tool_bytes.iter().map(|(_, bytes)| bytes).sum();
        lines.push(format!(
            "  tools   {} available ({}, {} per request — hide unused ones with `enabled_tools`):",
            inspection.tools.len(),
            format_bytes(total),
            format_tokens(total),
        ));
        for tool in &inspection.tools {
            // The full name is what the agent must call, so show it verbatim.
            let mut line = format!("            mcp__{full_name}__{tool}");
            // The effective risk, so a silent entry (High) and a declared one
            // never look alike.
            let declared = inspection
                .declared_risks
                .iter()
                .find(|(declared, _)| declared == tool)
                .map(|(_, risk)| risk.to_string());
            match declared {
                Some(risk) => line.push_str(&format!("  risk {risk} (declared)")),
                None => line.push_str("  risk high (default)"),
            }
            // The server's own claim, marked as a claim. It is evidence for
            // choosing a `tools.<name>.risk`, never a risk Tact applied.
            if inspection.declared_read_only.iter().any(|t| t == tool) {
                line.push_str("  (server-declared read-only)");
            }
            if let Some((_, bytes)) = inspection.tool_bytes.iter().find(|(name, _)| name == tool) {
                line.push_str(&format!("  {}", format_bytes(*bytes)));
            }
            lines.push(line);
        }
    }
    // A filtered server must not look like one that never had these tools.
    if !inspection.filtered.is_empty() {
        lines.push(format!(
            "  hidden  {} by enabled_tools/disabled_tools: {}",
            inspection.filtered.len(),
            inspection.filtered.join(", "),
        ));
    }
    // Same reasoning for the server's own guidance: "sent nothing" and "sent
    // something we dropped" must not look alike.
    if let Some(chars) = inspection.instructions_chars {
        lines.push(format!(
            "  instructions  {chars} chars (injected into the system prompt)"
        ));
    }
    // "publishes none" and "does not answer resources/list" are different
    // states, and only the first is worth reading as a fact about the server.
    match inspection.resources {
        Some(count) => lines.push(format!(
            "  resources  {count} available to `list_mcp_resources`"
        )),
        None if matches!(inspection.status, McpServerStatus::Connected) => {
            lines.push("  resources  (the server did not answer `resources/list`)".to_string());
        }
        None => {}
    }
    // A template-addressed server enumerates nothing above, so this is the line
    // that tells a user their "empty" server is not empty.
    match inspection.resource_templates {
        Some(count) => lines.push(format!(
            "  templates  {count} available to `list_mcp_resource_templates`"
        )),
        None if matches!(inspection.status, McpServerStatus::Connected) => {
            lines.push(
                "  templates  (the server did not answer `resources/templates/list`)".to_string(),
            );
        }
        None => {}
    }
    // Prompts are the primitive nothing else surfaces: without this line a
    // server's templates are invisible until the model happens to ask.
    match inspection.prompts {
        Some(count) => lines.push(format!(
            "  prompts  {count} available to `list_mcp_prompts`"
        )),
        None if matches!(inspection.status, McpServerStatus::Connected) => {
            lines.push("  prompts  (the server did not answer `prompts/list`)".to_string());
        }
        None => {}
    }
    if let Some(draft) = suggested_risk_policy(inspection) {
        lines.push(draft);
    }
    lines.join("\n")
}

/// `34.6 KB` — one decimal, because the difference between 34.6 and 35 is not
/// the point and the difference between 4 and 34 is.
fn format_bytes(bytes: usize) -> String {
    if bytes < 10_000 {
        format!("{:.1} KB", bytes as f64 / 1000.0)
    } else {
        format!("{:.0} KB", bytes as f64 / 1000.0)
    }
}

/// `≈8.6k tokens`, an estimate at four bytes per token.
///
/// Labelled as an estimate on purpose: the real count comes from the provider
/// (`ctx` in the status bar), and this is only here to rank servers and tools
/// against each other.
fn format_tokens(bytes: usize) -> String {
    let tokens = bytes as f64 / 4.0;
    if tokens < 1000.0 {
        format!("≈{tokens:.0} tokens")
    } else {
        format!("≈{:.1}k tokens", tokens / 1000.0)
    }
}

/// A paste-ready `tools` block for the tools the server declared read-only.
///
/// Tact deliberately does **not** apply `readOnlyHint` (see
/// `McpClient::declared_read_only`: a server can lie, and a read-only tool can
/// still be an egress path), so an entry that declares nothing leaves every
/// tool at `High` — a memory server then asks before every lookup. This drafts
/// the boring half of that policy from the server's own claim: the human reads
/// it, pastes it, and has decided. Everything the server did *not* claim
/// read-only is left out, so it keeps the `High` default: the draft can only
/// ever make things stricter than the claim, never looser.
///
/// `None` when there is nothing to suggest or nothing left to add.
fn suggested_risk_policy(inspection: &mcp::McpServerInspection) -> Option<String> {
    let already: Vec<&str> = inspection
        .declared_risks
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    let proposed: Vec<&str> = inspection
        .declared_read_only
        .iter()
        .map(String::as_str)
        .filter(|tool| !already.contains(tool))
        .collect();
    if proposed.is_empty() {
        return None;
    }

    let width = proposed.iter().map(|tool| tool.len()).max().unwrap_or(0);
    let mut out = String::from(
        "\n  suggested risk policy — the server calls these read-only; review, then paste\n  \
         into the entry's \"tools\" (Tact does not apply the claim itself):\n\n    \"tools\": {\n",
    );
    for (i, tool) in proposed.iter().enumerate() {
        let comma = if i + 1 == proposed.len() { "" } else { "," };
        // Pad *after* the quoted key: inside the quotes it would become part of
        // the name the user pastes.
        let pad = " ".repeat(width.saturating_sub(tool.len()));
        out.push_str(&format!(
            "      \"{tool}\":{pad} {{ \"risk\": \"read\" }}{comma}\n"
        ));
    }
    out.push_str("    }");
    Some(out)
}

/// Deletes the stored OAuth credentials for one server.
async fn logout(server: &str) -> Result<()> {
    // Best-effort resolution: a server that is no longer configured can still
    // have stored credentials, and those are keyed by the full name, so a name
    // that resolves to nothing is used verbatim rather than refused.
    let server = mcp::resolve_server_name(server).unwrap_or_else(|_| server.to_owned());
    let display = mcp::display_server_name(&server);
    let removed = mcp::forget_credentials(&server)
        .await
        .with_context(|| format!("failed to clear credentials for '{display}'"))?;
    match removed {
        Some(path) => {
            println!("Logged out of '{display}' (deleted {}).", path.display());
            println!("\nThe next connection will need: tact-ui mcp login {display}");
        }
        None => println!(
            "No stored credentials for '{display}' — nothing to delete.\n\
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
                Declare servers in ~/.tact/.mcp.json (user) or .tact/.mcp.json (project):\n\
                \x20 { \"mcpServers\": { \"my-server\": { \"command\": \"...\" } } }\n\
                \x20 { \"mcpServers\": { \"remote\": { \"url\": \"https://.../mcp\" } } }"
            .to_string();
    }

    // Widths are measured on the displayed (short) names, not the full ones, or
    // a plugin server would pad every other row by its prefix.
    let width = report
        .configured
        .iter()
        .map(|s| mcp::display_server_name(&s.name).len())
        .chain(
            report
                .skipped_remote
                .iter()
                .map(|name| mcp::display_server_name(name).len()),
        )
        .max()
        .unwrap_or(0);

    let mut lines = Vec::new();
    for server in &report.configured {
        // Checked first: a disabled server was never dialled, so
        // `status_for` would report "unknown" instead of "disabled".
        let status = if server.disabled {
            disabled_text()
        } else {
            status_for(report, &server.name)
        };
        lines.push(format!(
            "  {:<width$}  {:<34}  {}",
            mcp::display_server_name(&server.name),
            server.transport,
            status,
            width = width
        ));
    }
    for name in &report.skipped_remote {
        lines.push(format!(
            "  {:<width$}  {:<34}  skipped (no usable command or url)",
            mcp::display_server_name(name),
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
            notes.push(format!(
                "  {}  {displaced} is shadowed by {winner}",
                mcp::display_server_name(name)
            ));
        }
        out.push_str("\n\nOverridden declarations:\n");
        out.push_str(&notes.join("\n"));
    }

    // A policy-only declaration has no transport, so it owns no row in the
    // table above — yet it is what decided these tools' risk. Naming it keeps
    // "who wrote this policy" answerable when the entry that owns the transport
    // is a plugin bundle the user cannot edit.
    if !report.policy_overlays.is_empty() {
        let mut notes = Vec::new();
        for (name, source) in &report.policy_overlays {
            notes.push(format!(
                "  {}  policy overlaid from {source}",
                mcp::display_server_name(name)
            ));
        }
        out.push_str("\n\nPolicy overlays:\n");
        out.push_str(&notes.join("\n"));
    }

    // Hiding a tool is a deliberate configuration, not a problem — but it must
    // be visible somewhere, or a filtered server looks like one that simply
    // lacks those tools.
    if !report.filtered.is_empty() {
        let mut notes = Vec::new();
        for (server, hidden) in &report.filtered {
            notes.push(format!(
                "  {}  {} hidden by enabled_tools/disabled_tools: {}",
                mcp::display_server_name(server),
                hidden.len(),
                hidden.join(", "),
            ));
        }
        out.push_str("\n\nFiltered tools:\n");
        out.push_str(&notes.join("\n"));
    }

    // Codex-only entry fields are parsed but not implemented. Naming them here
    // is the only place a default run can see it: `tracing::warn!` reaches a
    // log file only when `RUST_LOG` (or `tokio_console`) installed a subscriber.
    if !report.unmodelled.is_empty() {
        let mut notes = Vec::new();
        for entry in &report.unmodelled {
            notes.push(format!(
                "  {}  {}  (ignored, from {})",
                mcp::display_server_name(&entry.server),
                entry.keys.join(", "),
                entry.source,
            ));
        }
        out.push_str("\n\nEntry keys Tact does not model:\n");
        out.push_str(&notes.join("\n"));
    }
    out
}

/// Renders the TUI `/mcp list` view: a Markdown table of every configured
/// server with its **live** status.
///
/// Unlike [`render_report`], which starts from a fresh load, this consumes
/// views derived from connections the agent already holds — the caller
/// (`/mcp list`) must never reconnect.
#[must_use]
pub fn render_live_listing(views: &[mcp::McpServerView]) -> String {
    if views.is_empty() {
        return "## 🔌 MCP Servers\n\nNo MCP servers configured.\n\n\
                Declare servers in `~/.tact/.mcp.json` (user) or `.tact/.mcp.json` (project), \
                then restart or run `/mcp auth <server>` for a remote OAuth server."
            .to_string();
    }

    let mut out = String::from(
        "## 🔌 MCP Servers\n\n| Server | Transport | Source | Status |\n|---|---|---|---|\n",
    );
    for view in views {
        let name = mcp::display_server_name(&view.server.name);
        let status = match view.status {
            McpLiveStatus::Connected { tools } => format!("connected ({tools} tools)"),
            McpLiveStatus::NeedsAuthorization => {
                format!("needs authorization — run `/mcp auth {name}`")
            }
            McpLiveStatus::NotConnected => "not connected".to_string(),
            McpLiveStatus::Disabled => disabled_text(),
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            cell(name),
            cell(&view.server.transport.to_string()),
            cell(&view.server.source),
            cell(&status),
        ));
    }
    out
}

/// Escapes a value for a Markdown table cell.
///
/// A raw `|` or newline would split the row into extra cells/rows; both are
/// reachable from user-controlled fields (a source path, a URL).
fn cell(value: &str) -> String {
    value.replace('|', "\\|").replace(['\n', '\r'], " ")
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
    // A plugin server is shown short, so the printed command must be the short
    // form too — `mcp login` resolves it back (see `tact::mcp::resolve_server_name`).
    format!(
        "needs authorization — run `tact-ui mcp login {}`",
        mcp::display_server_name(name)
    )
}

fn failed_text(error: &str) -> String {
    format!("failed: {error}")
}

/// Shown for a server its own declaration switched off (`enabled: false`), so
/// "configured but doing nothing" is never mistaken for a broken server.
fn disabled_text() -> String {
    "disabled (enabled: false)".to_string()
}

/// The status line for the single-server view.
fn status_text(inspection: &mcp::McpServerInspection) -> String {
    match &inspection.status {
        McpServerStatus::Connected => connected_text(inspection.tools.len()),
        McpServerStatus::Disabled => disabled_text(),
        McpServerStatus::PendingAuthorization => needs_auth_text(&inspection.server.name),
        McpServerStatus::Failed(error) => failed_text(error),
    }
}

/// Runs the interactive OAuth flow, printing the URL for the user to open.
async fn authorize(server: &str) -> Result<()> {
    // Fail before opening a browser flow for a name that is not configured. The
    // short form the listings show is accepted, so the printed hint is directly
    // runnable; the canonical name is what the flow and its credential file use.
    let server = mcp::resolve_server_name(server)?;
    let display = mcp::display_server_name(&server);
    let config = mcp::remote_config_for(&server)?.with_context(|| {
        format!("no remote MCP server named '{display}' is configured (see `tact-ui mcp list`)")
    })?;

    let oauth_declared = matches!(config.auth, Some(tact::mcp::McpAuthConfig::Oauth { .. }));
    if !oauth_declared {
        eprintln!(
            "Note: '{display}' does not declare `auth` in .mcp.json. \
             Authorizing anyway — the server's 401 is what requires it."
        );
    }

    let mut printed_url = false;
    let mut notify = |line: &str| {
        printed_url = true;
        println!("\nOpen this URL in your browser to authorize '{display}':\n\n  {line}\n");
        println!("Waiting for the redirect (Ctrl-C to abort)...");
    };

    mcp::authorize_server(&server, &mut notify)
        .await
        .with_context(|| format!("authorization failed for '{display}'"))?;

    if !printed_url {
        // Defensive: a successful flow always reports a URL first.
        eprintln!("Warning: no authorization URL was reported for '{display}'.");
    }
    println!("\nAuthorized '{display}'. Credentials saved.");

    // Show the server's new state so the user immediately sees it working
    // (and does not have to re-run `list` to find out).
    let (_router, report) = mcp::load_mcp_router_with_report().await?;
    println!("\n{}", render_report(&report));

    let status = status_for(&report, &server);
    if !status.starts_with("connected") {
        anyhow::bail!("authorized, but '{display}' is still not connected: {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact::permission::CapabilityRisk;

    pub(super) fn configured(name: &str, oauth: bool) -> mcp::ConfiguredServer {
        mcp::ConfiguredServer {
            name: name.to_string(),
            transport: mcp::McpTransportKind::Remote {
                url: format!("https://example.invalid/{name}"),
                oauth,
            },
            source: "~/.tact/.mcp.json".to_string(),
            disabled: false,
        }
    }

    #[test]
    fn empty_report_explains_how_to_configure() {
        let text = render_report(&McpLoadReport::default());
        assert!(text.contains("No MCP servers configured."), "{text}");
        assert!(text.contains("~/.tact/.mcp.json"), "{text}");
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
    fn report_shows_a_plugin_server_under_its_short_name() {
        let mut server = configured("plugin__canva__canva", true);
        server.transport = mcp::McpTransportKind::Remote {
            url: "https://mcp.canva.com/mcp".into(),
            oauth: true,
        };
        server.source = "plugin openai-curated/canva".to_string();
        let report = McpLoadReport {
            configured: vec![server],
            pending_auth: vec!["plugin__canva__canva".to_string()],
            ..McpLoadReport::default()
        };

        let text = render_report(&report);

        assert!(text.contains("  canva  remote"), "{text}");
        // The status cell names the same short form the row does — and it is a
        // command the user can actually run, which `mcp login` resolves.
        assert!(
            text.contains("needs authorization — run `tact-ui mcp login canva`"),
            "{text}"
        );
        assert!(!text.contains("plugin__canva__canva"), "{text}");
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
    fn report_notes_name_a_plugin_server_short_too() {
        // Every block of the report has to agree with the table above it: a
        // note naming a different string than the row it refers to is worse
        // than either name alone.
        let mut server = configured("plugin__canva__canva", false);
        server.transport = mcp::McpTransportKind::Stdio {
            command: "/bin/canva".to_string(),
        };
        let report = McpLoadReport {
            configured: vec![server],
            shadowed: vec![(
                "plugin__canva__canva".to_string(),
                "/home/me/.tact/.mcp.json".to_string(),
            )],
            filtered: vec![(
                "plugin__canva__canva".to_string(),
                vec!["delete_design".to_string()],
            )],
            unmodelled: vec![mcp::UnmodelledKeys {
                server: "plugin__canva__canva".to_string(),
                source: "/proj/.tact/.mcp.json".to_string(),
                keys: vec!["omit_tools_from".to_string()],
            }],
            ..McpLoadReport::default()
        };

        let text = render_report(&report);

        assert!(text.contains("  canva  /home/me"), "{text}");
        assert!(text.contains("  canva  1 hidden by"), "{text}");
        assert!(text.contains("  canva  omit_tools_from"), "{text}");
        assert!(!text.contains("plugin__canva__canva"), "{text}");
    }

    #[test]
    fn unmodelled_keys_are_named_in_the_listing() {
        // The log line only reaches a file when RUST_LOG or tokio_console
        // installed a subscriber, so the listing is the visible half.
        let report = McpLoadReport {
            configured: vec![configured("codexish", false)],
            unmodelled: vec![mcp::UnmodelledKeys {
                server: "codexish".to_string(),
                source: "/proj/.mcp.json".to_string(),
                keys: vec!["enabled_tools".to_string(), "tools".to_string()],
            }],
            ..McpLoadReport::default()
        };

        let text = render_report(&report);
        assert!(text.contains("Entry keys Tact does not model"), "{text}");
        assert!(text.contains("enabled_tools, tools"), "{text}");
        assert!(text.contains("/proj/.mcp.json"), "{text}");
    }

    #[test]
    fn tools_hidden_by_the_entry_policy_are_named_in_the_listing() {
        // Hiding a tool is deliberate, but it must be visible: otherwise a
        // filtered server is indistinguishable from one that lacks the tool.
        let report = McpLoadReport {
            configured: vec![configured("basic-memory", false)],
            filtered: vec![(
                "basic-memory".to_string(),
                vec!["delete_note".to_string(), "schema_diff".to_string()],
            )],
            ..McpLoadReport::default()
        };

        let text = render_report(&report);
        assert!(text.contains("Filtered tools:"), "{text}");
        assert!(
            text.contains("2 hidden by enabled_tools/disabled_tools"),
            "{text}"
        );
        assert!(text.contains("delete_note, schema_diff"), "{text}");
    }

    /// "21 tools" says nothing about what a server costs; every request
    /// re-declares them, so the view has to add it up for `enabled_tools` to be
    /// a decision the user can make.
    #[test]
    fn the_detail_view_reports_what_the_tools_cost() {
        let inspection = mcp::McpServerInspection {
            tool_bytes: vec![
                ("read_note".to_string(), 2_000),
                ("write_note".to_string(), 4_500),
            ],
            server: configured("bm", false),
            status: McpServerStatus::Connected,
            tools: vec!["read_note".to_string(), "write_note".to_string()],
            filtered: Vec::new(),
            instructions_chars: None,
            resources: None,
            resource_templates: None,
            prompts: None,
            declared_read_only: Vec::new(),
            declared_risks: Vec::new(),
        };

        let text = render_server_detail(&inspection);

        assert!(
            text.contains("2 available (6.5 KB, ≈1.6k tokens per request"),
            "{text}"
        );
        assert!(text.contains("enabled_tools"), "the knob is named: {text}");
        assert!(
            text.contains("read_note  risk high (default)  2.0 KB"),
            "{text}"
        );
        assert!(
            text.contains("write_note  risk high (default)  4.5 KB"),
            "{text}"
        );

        // A server that reported no tools has no cost line to report.
        let empty = mcp::McpServerInspection {
            tools: Vec::new(),
            tool_bytes: Vec::new(),
            ..inspection
        };
        let text = render_server_detail(&empty);
        assert!(!text.contains("per request"), "{text}");
    }

    /// The draft exists so a ten-tool server does not have to be hand-written,
    /// but it is only ever a *suggestion*: it lists what the server claimed
    /// read-only and nothing else, so pasting it cannot loosen anything.
    #[test]
    fn the_detail_view_drafts_a_risk_policy_from_the_servers_own_claim() {
        let mut inspection = mcp::McpServerInspection {
            tool_bytes: Vec::new(),
            server: configured("bm", false),
            status: McpServerStatus::Connected,
            tools: vec![
                "read_content".to_string(),
                "read_note".to_string(),
                "delete_note".to_string(),
            ],
            filtered: Vec::new(),
            instructions_chars: None,
            resources: None,
            resource_templates: None,
            prompts: None,
            declared_read_only: vec!["read_content".to_string(), "read_note".to_string()],
            declared_risks: Vec::new(),
        };

        let text = render_server_detail(&inspection);

        assert!(text.contains("suggested risk policy"), "{text}");
        assert!(
            text.contains("\"read_content\": { \"risk\": \"read\" },"),
            "first row: {text}"
        );
        assert!(
            text.contains("\"read_note\":    { \"risk\": \"read\" }"),
            "padded outside the quoted key, and no trailing comma on the last row: {text}"
        );
        // The writer keeps the default: the draft never invents a tier for a
        // tool the server did not claim.
        assert!(!text.contains("delete_note\": "), "{text}");

        // Nothing left to add once every claim is declared.
        inspection.declared_risks = vec![
            ("read_content".to_string(), CapabilityRisk::Read),
            ("read_note".to_string(), CapabilityRisk::Read),
        ];
        let text = render_server_detail(&inspection);
        assert!(!text.contains("suggested risk policy"), "{text}");

        // And nothing to add when the server claimed nothing.
        inspection.declared_risks = Vec::new();
        inspection.declared_read_only = Vec::new();
        let text = render_server_detail(&inspection);
        assert!(!text.contains("suggested risk policy"), "{text}");
    }

    #[test]
    fn the_detail_view_separates_a_declared_risk_from_the_default() {
        // "The entry said high" and "the entry said nothing" must not look
        // alike, and the server's own read-only claim is marked as a claim
        // rather than being applied.
        let inspection = mcp::McpServerInspection {
            tool_bytes: Vec::new(),
            server: configured("bm", false),
            status: McpServerStatus::Connected,
            tools: vec!["search_notes".to_string(), "delete_project".to_string()],
            filtered: Vec::new(),
            instructions_chars: None,
            resources: None,
            resource_templates: None,
            prompts: None,
            declared_read_only: vec!["search_notes".to_string()],
            declared_risks: vec![("search_notes".to_string(), CapabilityRisk::Read)],
        };

        let text = render_server_detail(&inspection);
        assert!(
            text.contains("mcp__bm__search_notes  risk read (declared)"),
            "{text}"
        );
        assert!(text.contains("(server-declared read-only)"), "{text}");
        assert!(
            text.contains("mcp__bm__delete_project  risk high (default)"),
            "{text}"
        );
    }

    #[test]
    fn the_detail_view_heads_a_plugin_server_short_but_keeps_its_tool_prefix() {
        // The heading is for the user; the tool names are what the agent calls,
        // so those must keep the full server name.
        let inspection = mcp::McpServerInspection {
            tool_bytes: Vec::new(),
            server: configured("plugin__canva__canva", true),
            status: McpServerStatus::Connected,
            tools: vec!["list_designs".to_string()],
            filtered: Vec::new(),
            instructions_chars: None,
            resources: None,
            resource_templates: None,
            prompts: None,
            declared_read_only: Vec::new(),
            declared_risks: Vec::new(),
        };

        let text = render_server_detail(&inspection);
        assert!(text.starts_with("canva  remote "), "{text}");
        assert!(
            text.contains("mcp__plugin__canva__canva__list_designs"),
            "{text}"
        );
    }

    #[test]
    fn a_template_only_server_is_not_reported_as_an_empty_one() {
        // A server that publishes templates enumerates nothing through
        // `resources/list`, so "resources 0" alone reads as "nothing here".
        let inspection = mcp::McpServerInspection {
            tool_bytes: Vec::new(),
            server: configured("memory", false),
            status: McpServerStatus::Connected,
            tools: vec!["read_note".to_string()],
            filtered: Vec::new(),
            instructions_chars: None,
            resources: Some(0),
            resource_templates: Some(3),
            prompts: Some(4),
            declared_read_only: Vec::new(),
            declared_risks: Vec::new(),
        };

        let text = render_server_detail(&inspection);
        assert!(
            text.contains("templates  3 available to `list_mcp_resource_templates`"),
            "{text}"
        );
        // Prompts have no other surface: nothing else in the detail view would
        // tell a user their server offers four of them.
        assert!(
            text.contains("prompts  4 available to `list_mcp_prompts`"),
            "{text}"
        );

        // And "did not answer" stays a different fact from "publishes none".
        let unanswered = mcp::McpServerInspection {
            resource_templates: None,
            prompts: None,
            ..inspection
        };
        let text = render_server_detail(&unanswered);
        assert!(
            text.contains("did not answer `resources/templates/list`"),
            "{text}"
        );
    }

    #[test]
    fn the_detail_view_names_hidden_tools_too() {
        let inspection = mcp::McpServerInspection {
            tool_bytes: Vec::new(),
            server: configured("basic-memory", false),
            status: McpServerStatus::Connected,
            tools: vec!["read_note".to_string()],
            filtered: vec!["delete_note".to_string()],
            instructions_chars: None,
            resources: None,
            resource_templates: None,
            prompts: None,
            declared_read_only: Vec::new(),
            declared_risks: Vec::new(),
        };

        let text = render_server_detail(&inspection);
        assert!(text.contains("mcp__basic-memory__read_note"), "{text}");
        assert!(
            text.contains("hidden  1 by enabled_tools/disabled_tools"),
            "{text}"
        );
        assert!(text.contains("delete_note"), "{text}");
    }

    #[test]
    fn the_detail_view_reports_server_instructions() {
        let inspection = mcp::McpServerInspection {
            tool_bytes: Vec::new(),
            server: configured("basic-memory", false),
            status: McpServerStatus::Connected,
            tools: vec!["read_note".to_string()],
            filtered: Vec::new(),
            instructions_chars: Some(2_043),
            resources: Some(3),
            resource_templates: None,
            prompts: None,
            declared_read_only: Vec::new(),
            declared_risks: Vec::new(),
        };
        let text = render_server_detail(&inspection);
        assert!(
            text.contains("instructions  2043 chars (injected into the system prompt)"),
            "{text}"
        );

        // A server that sent nothing must not look like one whose guidance was
        // dropped, so absence stays silent — only a real payload gets a line.
        let quiet = mcp::McpServerInspection {
            instructions_chars: None,
            resources: None,
            resource_templates: None,
            prompts: None,
            ..inspection
        };
        assert!(
            !render_server_detail(&quiet).contains("instructions"),
            "{quiet:?}"
        );
    }

    #[test]
    fn the_detail_view_separates_no_resources_from_no_answer() {
        let connected = mcp::McpServerInspection {
            tool_bytes: Vec::new(),
            server: configured("memory", false),
            status: McpServerStatus::Connected,
            tools: vec!["read_note".to_string()],
            filtered: Vec::new(),
            instructions_chars: None,
            resources: Some(0),
            resource_templates: None,
            prompts: None,
            declared_read_only: Vec::new(),
            declared_risks: Vec::new(),
        };
        let text = render_server_detail(&connected);
        assert!(
            text.contains("resources  0 available to `list_mcp_resources`"),
            "{text}"
        );

        // "does not answer" is not the same fact as "publishes none", and only
        // the second is a statement about the server's contents.
        let silent = mcp::McpServerInspection {
            resources: None,
            resource_templates: None,
            prompts: None,
            ..connected
        };
        let text = render_server_detail(&silent);
        assert!(text.contains("did not answer `resources/list`"), "{text}");

        let pending = mcp::McpServerInspection {
            status: McpServerStatus::PendingAuthorization,
            resources: None,
            resource_templates: None,
            prompts: None,
            ..silent
        };
        assert!(
            !render_server_detail(&pending).contains("resources"),
            "a server that was never contacted has nothing to say about resources"
        );
    }

    #[test]
    fn a_disabled_server_is_reported_as_disabled_not_unknown() {
        // A disabled server was never dialled, so it has no connection
        // outcome; reporting "unknown" would read as a broken server.
        let mut server = configured("switched-off", false);
        server.disabled = true;
        let report = McpLoadReport {
            configured: vec![server],
            ..McpLoadReport::default()
        };

        let text = render_report(&report);
        assert!(text.contains("disabled (enabled: false)"), "{text}");
        assert!(!text.contains("unknown"), "{text}");
    }

    #[test]
    fn the_live_listing_marks_a_disabled_server() {
        let mut view = live_view(
            "switched-off",
            mcp::McpTransportKind::Stdio {
                command: "/bin/off".to_string(),
            },
            McpLiveStatus::Disabled,
        );
        view.server.disabled = true;

        let text = render_live_listing(&[view]);
        assert!(text.contains("disabled (enabled: false)"), "{text}");
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
                source: "/proj/.tact/.mcp.json".to_string(),
                disabled: false,
            }],
            shadowed: vec![("shared".to_string(), "/home/me/.tact/.mcp.json".to_string())],
            ..McpLoadReport::default()
        };

        let text = render_report(&report);
        assert!(text.contains("Overridden declarations:"), "{text}");
        assert!(
            text.contains("/home/me/.tact/.mcp.json is shadowed by /proj/.tact/.mcp.json"),
            "{text}"
        );
    }

    #[test]
    fn a_policy_overlay_names_the_file_the_policy_came_from() {
        // A policy-only entry owns no row of its own, so without this section
        // a plugin server's risk would read as if the plugin bundle set it —
        // and that file is the one thing the user is told not to edit.
        let report = McpLoadReport {
            configured: vec![mcp::ConfiguredServer {
                name: "plugin__canva__canva".to_string(),
                transport: mcp::McpTransportKind::Remote {
                    url: "https://mcp.canva.com/mcp".to_string(),
                    oauth: true,
                },
                source: "installed plugin (/home/me/.tact/plugins)".to_string(),
                disabled: false,
            }],
            policy_overlays: vec![(
                "plugin__canva__canva".to_string(),
                "/home/me/.tact/.mcp.json".to_string(),
            )],
            ..McpLoadReport::default()
        };

        let text = render_report(&report);
        assert!(text.contains("Policy overlays:"), "{text}");
        assert!(
            text.contains("canva  policy overlaid from /home/me/.tact/.mcp.json"),
            "{text}"
        );
    }

    #[test]
    fn a_report_without_overlays_has_no_overlay_section() {
        let report = McpLoadReport {
            configured: vec![configured("solo", false)],
            connected: vec![("solo".to_string(), 1)],
            ..McpLoadReport::default()
        };
        assert!(
            !render_report(&report).contains("Policy overlays"),
            "{}",
            render_report(&report)
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
            tool_bytes: Vec::new(),
            server: configured("figma", false),
            status: McpServerStatus::Connected,
            tools: vec!["get_file".into(), "list_files".into()],
            filtered: Vec::new(),
            instructions_chars: None,
            resources: None,
            resource_templates: None,
            prompts: None,
            declared_read_only: Vec::new(),
            declared_risks: Vec::new(),
        };

        let text = render_server_detail(&inspection);
        assert!(
            text.contains("figma  remote https://example.invalid/figma"),
            "{text}"
        );
        assert!(text.contains("source  ~/.tact/.mcp.json"), "{text}");
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
            tool_bytes: Vec::new(),
            server: configured("linear", true),
            status: McpServerStatus::PendingAuthorization,
            tools: Vec::new(),
            filtered: Vec::new(),
            instructions_chars: None,
            resources: None,
            resource_templates: None,
            prompts: None,
            declared_read_only: Vec::new(),
            declared_risks: Vec::new(),
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
            tool_bytes: Vec::new(),
            server: configured("broken", false),
            status: McpServerStatus::Failed("connection refused".into()),
            tools: Vec::new(),
            filtered: Vec::new(),
            instructions_chars: None,
            resources: None,
            resource_templates: None,
            prompts: None,
            declared_read_only: Vec::new(),
            declared_risks: Vec::new(),
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
            tool_bytes: Vec::new(),
            server: configured("quiet", false),
            status: McpServerStatus::Connected,
            tools: Vec::new(),
            filtered: Vec::new(),
            instructions_chars: None,
            resources: None,
            resource_templates: None,
            prompts: None,
            declared_read_only: Vec::new(),
            declared_risks: Vec::new(),
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

    fn live_view(
        name: &str,
        transport: mcp::McpTransportKind,
        status: McpLiveStatus,
    ) -> mcp::McpServerView {
        mcp::McpServerView {
            server: mcp::ConfiguredServer {
                name: name.to_string(),
                transport,
                source: "~/.tact/.mcp.json".to_string(),
                disabled: false,
            },
            status,
        }
    }

    #[test]
    fn live_listing_has_a_row_per_server_with_its_status() {
        let views = vec![
            live_view(
                "figma",
                mcp::McpTransportKind::Remote {
                    url: "https://mcp.figma.com/mcp".into(),
                    oauth: true,
                },
                McpLiveStatus::NeedsAuthorization,
            ),
            live_view(
                "local",
                mcp::McpTransportKind::Stdio {
                    command: "node".into(),
                },
                McpLiveStatus::Connected { tools: 4 },
            ),
            live_view(
                "broken",
                mcp::McpTransportKind::Stdio {
                    command: "nope".into(),
                },
                McpLiveStatus::NotConnected,
            ),
        ];

        let text = render_live_listing(&views);
        assert!(
            text.contains("| Server | Transport | Source | Status |"),
            "{text}"
        );
        assert!(text.contains("connected (4 tools)"), "{text}");
        assert!(
            text.contains("needs authorization — run `/mcp auth figma`"),
            "{text}"
        );
        assert!(text.contains("| broken |"), "{text}");
        assert!(text.contains("not connected"), "{text}");
    }

    #[test]
    fn live_listing_explains_how_to_configure_when_empty() {
        let text = render_live_listing(&[]);
        assert!(text.contains("No MCP servers configured."), "{text}");
        assert!(text.contains("~/.tact/.mcp.json"), "{text}");
    }

    #[test]
    fn live_listing_shows_a_plugin_server_under_its_short_name() {
        // The `plugin__<id>__` prefix is how the loader keeps plugin servers
        // apart from the user's own; in a listing it is only noise, and the
        // Source column already says which plugin it came from.
        let mut view = live_view(
            "plugin__canva__canva",
            mcp::McpTransportKind::Remote {
                url: "https://mcp.canva.com/mcp".into(),
                oauth: true,
            },
            mcp::McpLiveStatus::NeedsAuthorization,
        );
        view.server.source = "plugin openai-curated/canva".to_string();

        let text = render_live_listing(&[view]);

        assert!(text.contains("| canva |"), "{text}");
        assert!(
            text.contains("needs authorization — run `/mcp auth canva`"),
            "{text}"
        );
        assert!(!text.contains("plugin__canva__canva"), "{text}");
    }

    #[test]
    fn live_listing_escapes_pipes_so_a_source_path_cannot_break_the_table() {
        let mut view = live_view(
            "weird",
            mcp::McpTransportKind::Stdio {
                command: "/bin/echo".into(),
            },
            McpLiveStatus::Connected { tools: 1 },
        );
        view.server.source = "a|b".to_string();

        let text = render_live_listing(&[view]);
        assert!(text.contains("a\\|b"), "{text}");
    }
}
