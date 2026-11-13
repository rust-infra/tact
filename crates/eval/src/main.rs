//! `tact-eval`: run a suite and print a report.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use tact_eval::{CliRunner, EvalSuite, HostRunner, PermissionMode, RunOptions, Runner, run_suite};

#[derive(Parser, Debug)]
#[command(version, about = "Run Tact agent evaluation suites")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run a suite of cases
    Run(RunArgs),
    /// Check that every case parses and validates, without calling a model
    Validate { suite: PathBuf },
}

#[derive(Parser, Debug)]
struct RunArgs {
    /// Directory of `*.toml` case files
    suite: PathBuf,

    /// Which runner executes the cases
    #[arg(long, value_enum, default_value_t = RunnerKind::Host)]
    runner: RunnerKind,

    /// Attempts per case; pass@k uses this as k
    #[arg(long, default_value_t = 1)]
    attempts: u32,

    /// Write the JSON report here instead of stdout
    #[arg(long)]
    json_out: Option<PathBuf>,

    /// Exit non-zero when any case fails every attempt
    #[arg(long)]
    strict: bool,

    /// `tact-ui` binary for the CLI runner. Ignored by the host runner.
    #[arg(long)]
    binary: Option<PathBuf>,

    #[command(flatten)]
    options: OptionArgs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum RunnerKind {
    Host,
    Cli,
}

/// The flags both runners accept, spelled as `tact-ui` spells them.
#[derive(Parser, Debug, Clone, Default)]
struct OptionArgs {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long, value_parser = parse_permission)]
    permission_mode: Option<PermissionMode>,
}

fn parse_permission(value: &str) -> Result<PermissionMode, String> {
    value.parse()
}

impl OptionArgs {
    fn run_options(&self) -> RunOptions {
        RunOptions {
            config: self.config.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            permission_mode: self.permission_mode.unwrap_or_default(),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => run(args).await,
        Command::Validate { suite } => validate(&suite),
    }
}

/// Parse and validate every case, then report how many there are.
///
/// This is the part of the harness CI can run with no API key and no binary:
/// a broken case file, a fixture that moved, and an invalid pattern all fail
/// here instead of halfway through a paid run.
fn validate(dir: &std::path::Path) -> anyhow::Result<()> {
    let suite = EvalSuite::load(dir)?;
    println!("{} case(s) valid in {}", suite.cases().len(), dir.display());
    Ok(())
}

async fn run(args: RunArgs) -> anyhow::Result<()> {
    let suite = EvalSuite::load(&args.suite)?;
    let options = args.options.run_options();

    let runner: Box<dyn Runner> = match args.runner {
        RunnerKind::Host => {
            // The host path reads process-global settings, so installing them
            // is what makes the report's model and endpoint real rather than
            // "unset".
            tact::config::init_from_args(&options.to_cli_args())?;
            Box::new(HostRunner::new(options))
        }
        RunnerKind::Cli => {
            let binary = args
                .binary
                .unwrap_or_else(tact_eval::host_runner::default_cli_binary);
            Box::new(CliRunner::new(binary).with_options(options))
        }
    };

    let report = run_suite(&suite, runner.as_ref(), args.attempts).await?;
    eprintln!("{}", report.to_text());

    match args.json_out {
        Some(path) => std::fs::write(&path, report.to_json()?)?,
        None => println!("{}", report.to_json()?),
    }

    if args.strict && report.cases_passing_all() < report.summaries().len() {
        std::process::exit(1);
    }
    Ok(())
}
