use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// tact — terminal-first AI coding agent
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct CliArgs {
    #[command(subcommand)]
    pub command: Option<CliCommand>,

    /// Path to a TOML config file
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Active LLM provider (built-ins: `anthropic` | `openai` | `deepseek` | `kimi`; any other name = custom OpenAI-compatible); selects `[llm.providers.<name>]`
    #[arg(long)]
    pub provider: Option<String>,

    /// Model name (e.g. "kimi-for-coding", "deepseek-v4-pro", "gpt-4o")
    #[arg(long)]
    pub model: Option<String>,

    /// API key for the provider
    #[arg(long)]
    pub api_key: Option<String>,

    /// Base URL for the provider API
    #[arg(long)]
    pub base_url: Option<String>,

    /// Maximum tokens to generate per LLM call
    #[arg(long)]
    pub max_tokens: Option<u32>,

    /// Budget tokens for extended thinking (Anthropic/Kimi `thinking`)
    #[arg(long)]
    pub thinking_budget: Option<usize>,

    /// Permission mode: "default", "plan", or "auto"
    #[arg(short = 'm', long)]
    pub permission_mode: Option<String>,

    /// Resume a specific session by ID
    #[arg(long = "session")]
    pub session: Option<String>,

    /// Resume the most recent session
    #[arg(long = "resume-last")]
    pub resume_last: bool,

    /// List recent sessions and exit
    #[arg(long = "list-sessions")]
    pub list_sessions: bool,

    /// Enable desktop notifications (macOS only).
    #[arg(long)]
    pub notifications: Option<bool>,

    /// Disable desktop notifications.
    #[arg(long)]
    pub no_notifications: bool,

    /// Model context window in tokens before auto-compaction is triggered.
    #[arg(long)]
    pub model_context_window: Option<usize>,

    /// UI theme name (e.g. "retro", "nord", "dark").
    #[arg(long)]
    pub theme: Option<String>,

    /// Max entries in the system-prompt project structure snapshot.
    #[arg(long)]
    pub snapshot_max_items: Option<usize>,

    /// Disable micro-compaction of old tool results.
    #[arg(long)]
    pub no_micro_compact: bool,

    /// Enable tokio-console debugging subscriber.
    #[arg(long)]
    pub tokio_console: bool,

    /// Auto-inject full skill body into system prompt (default: false).
    #[arg(long)]
    pub skill_body_auto_inject: bool,
}

#[derive(Subcommand, Debug)]
pub enum CliCommand {
    /// Run a single task without the interactive TUI
    Headless {
        /// The task prompt to execute
        prompt: String,
    },
    /// Upgrade tact-ui to the latest GitHub release
    Upgrade {
        /// GitHub repository (owner/name) to check for releases
        #[arg(long, env = "TACT_UPGRADE_REPO", default_value = "rust-infra/tact")]
        repo: String,
        /// Skip the interactive confirmation prompt
        #[arg(long)]
        yes: bool,
        /// Check for a newer version and print it without upgrading
        #[arg(long)]
        check: bool,
    },
    /// Manage plugins and marketplaces
    Plugin {
        #[command(subcommand)]
        command: PluginSubcommand,
    },
    /// Inspect and configure MCP servers
    Mcp {
        #[command(subcommand)]
        command: McpSubcommand,
    },
    /// Review the command hooks that would run, and trust them
    Hooks {
        #[command(subcommand)]
        command: HooksSubcommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum HooksSubcommand {
    /// List every configured hook and whether it has been reviewed
    ///
    /// Reads installed plugins, `~/.tact/hooks.json` and `.tact/hooks.json`.
    /// Nothing is executed and nothing is connected.
    List,
    /// Approve hooks so the next session runs them
    ///
    /// A hook definition that has not been approved is never registered, so a
    /// repository that ships `.tact/hooks.json` cannot execute anything by
    /// being cloned. Approving is keyed on the exact definition: editing a
    /// command means it must be reviewed again.
    ///
    /// Examples: `tact-ui hooks trust --all`
    ///           `tact-ui hooks trust --source ~/.tact/hooks.json`
    Trust {
        /// Approve every hook that still needs review
        #[arg(long)]
        all: bool,
        /// Approve only this source, as `hooks list` names it
        #[arg(long, value_name = "LABEL")]
        source: Option<String>,
    },
    /// Forget every review decision
    ///
    /// Example: `tact-ui hooks forget --all`
    Forget {
        /// Required: revoking every approval is never implicit
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum McpSubcommand {
    /// List every configured MCP server and its status
    ///
    /// Connects to all servers (like startup does), then prints each one as
    /// connected / needs authorization / failed / skipped.
    List,
    /// Show one configured MCP server in detail
    ///
    /// Connects to that server only — never the rest of the configuration —
    /// and prints its transport, source, status and tool list.
    ///
    /// Example: `tact-ui mcp get deepwiki`
    Get {
        /// Server name as declared in `.mcp.json`
        name: String,
    },
    /// Add an MCP server to `.mcp.json`
    ///
    /// Exactly one transport is required: `--url` for a remote Streamable HTTP
    /// server, or `--command` for a local stdio server. The project file
    /// (`<workdir>/.tact/.mcp.json`) is written unless `--user` is given.
    ///
    /// Example: `tact-ui mcp add deepwiki --url https://mcp.deepwiki.com/mcp`
    /// — add `--oauth` when the server requires authorization, and
    /// `--force` to replace an existing declaration of the same name.
    Add {
        /// Server name; the key under `mcpServers`
        name: String,
        /// Remote Streamable HTTP endpoint
        #[arg(long, value_name = "URL", conflicts_with = "command")]
        url: Option<String>,
        /// Local stdio server command
        #[arg(long, value_name = "COMMAND", conflicts_with = "url")]
        command: Option<String>,
        /// Argument passed to the stdio command (repeatable)
        #[arg(long = "arg", value_name = "ARG", requires = "command")]
        args: Vec<String>,
        /// Environment variable for the stdio command, NAME=VALUE (repeatable)
        ///
        /// The value is visible in `ps` and shell history; prefer a variable
        /// the command can read from elsewhere when it holds a secret.
        #[arg(long = "env", value_name = "NAME=VALUE", requires = "command")]
        env: Vec<String>,
        /// Extra request header for a remote server, NAME:VALUE (repeatable)
        ///
        /// The value is visible in `ps` and shell history; prefer a static
        /// token the server can read from the environment when it is secret.
        #[arg(long = "header", value_name = "NAME:VALUE", requires = "url")]
        header: Vec<String>,
        /// Declare OAuth 2.0 (authorize afterwards with `mcp login`)
        #[arg(long, requires = "url")]
        oauth: bool,
        /// Write `$HOME/.tact/.mcp.json` instead of the project file
        #[arg(long)]
        user: bool,
        /// Replace an existing server with the same name
        #[arg(long)]
        force: bool,
    },
    /// Remove an MCP server from `.mcp.json`
    ///
    /// Only the declaration is deleted; stored OAuth credentials are kept (use
    /// `mcp logout` to delete those).
    ///
    /// Example: `tact-ui mcp remove deepwiki --user`
    Remove {
        /// Server name as declared in `.mcp.json`
        name: String,
        /// Remove from `$HOME/.tact/.mcp.json` instead of the project file
        #[arg(long)]
        user: bool,
    },
    /// Authorize a remote MCP server (OAuth), and remember the token
    ///
    /// Prints the authorization URL, then waits for the browser redirect on a
    /// loopback port. The token is stored so later sessions connect silently.
    ///
    /// Example: `tact-ui mcp login linear`
    #[command(visible_alias = "auth")]
    Login {
        /// Server name as declared in `.mcp.json`
        server: String,
    },
    /// Delete the stored OAuth credentials for a server
    ///
    /// The next connection to that server will need `mcp login` again. Works
    /// even if the server is no longer declared.
    ///
    /// Example: `tact-ui mcp logout linear`
    Logout {
        /// Server name whose credentials should be deleted
        server: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum PluginSubcommand {
    /// List installed plugins
    List,
    /// Install a plugin from a marketplace (format: <name>@<marketplace>)
    Install {
        /// Plugin spec, e.g. "my-plugin@claude-plugins-official"
        spec: String,
    },
    /// Uninstall an installed plugin by name
    Uninstall {
        /// Plugin name to uninstall
        name: String,
    },
    /// Update an installed plugin to the latest revision
    Update {
        /// Plugin name to update
        name: String,
    },
    /// Reload all installed plugins
    Reload,
    /// Manage marketplace sources
    Marketplace {
        #[command(subcommand)]
        command: MarketplaceSubcommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum MarketplaceSubcommand {
    /// Add a marketplace from a Git URL or GitHub shorthand
    Add {
        /// Marketplace source URL or GitHub owner/repo shorthand
        source: String,
    },
    /// List all registered marketplaces
    List,
    /// Update (re-fetch) a marketplace catalog
    Update {
        /// Marketplace name to update
        name: String,
    },
    /// Remove a marketplace (cannot remove the built-in one)
    Remove {
        /// Marketplace name to remove
        name: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A top-level flag is accepted *before* the subcommand, and not after it.
    ///
    /// `--model` and its siblings are declared on `CliArgs` and none of them is
    /// marked `global`, so the position is load-bearing for every documented
    /// invocation — and it is what one README example had wrong
    /// (`docs(readme): the example that puts a top-level flag after the
    /// subcommand`). Pinned here rather than in a doc, because a doc does not
    /// fail when it drifts.
    #[test]
    fn a_top_level_flag_belongs_before_the_subcommand() {
        let before = CliArgs::parse_from(["tact-ui", "--model", "m", "headless", "p"]);
        assert_eq!(before.model.as_deref(), Some("m"));
        assert!(matches!(
            before.command,
            Some(CliCommand::Headless { ref prompt }) if prompt == "p"
        ));

        let after = CliArgs::try_parse_from(["tact-ui", "headless", "--model", "m", "p"]);
        assert!(
            after.is_err(),
            "the top-level flags are not `global`: if this now parses, the form the \
             README used to document works again and both should be updated together"
        );
    }

    /// `--session` and `--resume-last` are alternatives, and `--list-sessions`
    /// is a third thing that short-circuits before either is read.
    #[test]
    fn the_session_flags_parse_where_the_dispatcher_reads_them() {
        let named = CliArgs::parse_from(["tact-ui", "--session", "abc", "headless", "p"]);
        assert_eq!(named.session.as_deref(), Some("abc"));
        assert!(!named.resume_last);
        assert!(!named.list_sessions);

        let resumed = CliArgs::parse_from(["tact-ui", "--resume-last", "headless", "p"]);
        assert_eq!(resumed.session, None);
        assert!(resumed.resume_last);

        let listed = CliArgs::parse_from(["tact-ui", "--list-sessions"]);
        assert!(listed.list_sessions);
    }
}
