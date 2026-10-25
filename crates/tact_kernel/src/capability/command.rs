/// Where a command is available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandScope {
    Runtime,
    View(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandMeta {
    pub name: String,
    pub aliases: Vec<String>,
    pub summary: String,
    pub usage: String,
    pub scope: CommandScope,
    pub needs_args: bool,
}

/// What a command was asked to do.
///
/// The kernel routes; it never parses. `args` is the raw text after the command
/// name, because argument grammar belongs to whoever owns the command and a
/// shared parser would have to know every verb of every plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInvocation {
    pub name: String,
    pub args: String,
}

/// A command's answer.
///
/// A plain enum rather than `Result<(), Error>`: "the user asked for something
/// this command does not do" is not an error, and a plugin must be able to say
/// so without the host having to interpret a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    /// The command ran; nothing to show.
    Done,
    /// The command ran and produced text for the reader.
    Message(String),
    /// The command did not run because its arguments were unusable. The text
    /// names what was expected, so the host can show it verbatim.
    Usage(String),
    /// The command ran and the thing it asked on its behalf failed.
    ///
    /// Distinct from [`Self::Message`] because the host renders it differently —
    /// a failure is an error line, not a read-out popup — and distinct from
    /// [`Self::Usage`] because the user's input was fine. Collapsing these was
    /// how a manager error used to surface as a help message.
    Failed(String),
}

/// One entry the popup offers.
///
/// Produced by the owning plugin rather than by a central table: a subcommand
/// list that lives away from its handler is the drift this design removes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCandidate {
    /// What goes after the leading `/`; completing inserts it.
    pub path: String,
    /// The right-hand column.
    pub detail: String,
    /// More input is expected: Enter completes instead of running.
    pub incomplete: bool,
}

#[async_trait::async_trait]
pub trait Command: Send + Sync {
    fn meta(&self) -> &CommandMeta;

    /// Run this command.
    ///
    /// Defaulted to `Usage` so a command that has been declared but not wired
    /// up yet says so instead of silently succeeding.
    fn run(&self, invocation: &CommandInvocation) -> CommandOutcome {
        CommandOutcome::Usage(format!(
            "{} is not runnable yet ({} given)",
            invocation.name, invocation.args
        ))
    }

    /// Run this command when it has to await something.
    ///
    /// Defaulted to [`Self::run`], so a pure command implements one method. A
    /// command that reads live state (`/background` asks a manager that owns
    /// running processes; an MCP command asks a server) implements this instead,
    /// because a blocking body would hold the host's loop for the length of a
    /// network call.
    ///
    /// Both live on one trait on purpose: two registries — a sync one and an
    /// async one — is exactly the drift the catalog exists to remove, and the
    /// host has to know which to consult for a name it already has.
    async fn run_async(&self, invocation: &CommandInvocation) -> CommandOutcome {
        self.run(invocation)
    }

    /// The completions this command offers for `args`.
    ///
    /// Default: none. The host adds the command's own name; a command only
    /// describes what follows it.
    fn complete(&self, _args: &str) -> Vec<CommandCandidate> {
        Vec::new()
    }
}
