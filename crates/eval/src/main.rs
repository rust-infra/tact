//! `tact-eval`: run a suite and print a report.

use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand};
use tact_eval::{
    CliRunner, EvalReport, EvalSuite, HostRunner, PermissionMode, RunOptions, Runner, RunnerKind,
    compare_reports, comparison_exit_code, run_suite,
};

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
    /// Compare two evaluation report JSON files
    Compare {
        /// Baseline report JSON
        baseline: PathBuf,
        /// Candidate report JSON
        candidate: PathBuf,
        /// Write the comparison JSON here instead of stdout
        #[arg(long)]
        json_out: Option<PathBuf>,
    },
}

fn parse_runner(value: &str) -> Result<RunnerKind, String> {
    value.parse()
}

#[derive(Parser, Debug)]
struct RunArgs {
    /// Directory of `*.toml` case files
    suite: PathBuf,

    /// Which runner executes the cases
    #[arg(long, value_parser = parse_runner, default_value = "host")]
    runner: RunnerKind,

    /// Attempts per case; pass@k uses this as k
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    attempts: u32,

    /// Write the JSON report here instead of stdout
    #[arg(long)]
    json_out: Option<PathBuf>,

    /// Exit non-zero unless every case passes every attempt
    #[arg(long)]
    strict: bool,

    /// `tact-ui` binary for the CLI runner. Ignored by the host runner.
    #[arg(long)]
    binary: Option<PathBuf>,

    #[command(flatten)]
    options: OptionArgs,
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
    #[arg(long)]
    permission_mode: Option<PermissionMode>,
}

impl OptionArgs {
    fn run_options(&self) -> RunOptions {
        // Start from `RunOptions::default()` so the eval-specific `auto` default
        // lives in exactly one place. `PermissionMode::default()` is the
        // product's `Default` (ask first), which headless cannot honour.
        let defaults = RunOptions::default();
        RunOptions {
            config: self.config.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            permission_mode: self.permission_mode.unwrap_or(defaults.permission_mode),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => run(args).await,
        Command::Validate { suite } => validate(&suite),
        Command::Compare {
            baseline,
            candidate,
            json_out,
        } => {
            let code = match compare(&baseline, &candidate, json_out.as_deref()) {
                Ok(code) => code,
                Err(error) => {
                    eprintln!("tact-eval compare: {error:#}");
                    2
                }
            };
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
    }
}

fn load_report(path: &Path) -> anyhow::Result<EvalReport> {
    let bytes =
        std::fs::read(path).with_context(|| format!("cannot read report {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("cannot parse report {}", path.display()))
}

/// Compare two reports, write the result, and return the mapped exit code.
fn compare(baseline: &Path, candidate: &Path, json_out: Option<&Path>) -> anyhow::Result<i32> {
    let baseline = load_report(baseline)?;
    let candidate = load_report(candidate)?;
    let comparison = compare_reports(&baseline, &candidate);
    let json =
        serde_json::to_string_pretty(&comparison).context("cannot encode comparison as JSON")?;

    eprintln!("{}", comparison.to_text());
    match json_out {
        Some(path) => write_json(path, &json)
            .with_context(|| format!("cannot write comparison {}", path.display()))?,
        None => println!("{json}"),
    }

    Ok(comparison_exit_code(comparison.status))
}

/// Write a JSON artifact, creating the parent directory when it is missing.
///
/// Every documented example writes to `target/eval/...`, and a fresh checkout
/// has no `target/eval`. Refusing there would fail the one command the README
/// tells a new user to run, after the work (and for a model run, the spend) is
/// already done. Creating the directory is the smaller surprise.
///
/// A path with no parent at all (`report.json`) writes into the current
/// directory, which is why the create is conditional.
fn write_json(path: &Path, json: &str) -> anyhow::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create directory {}", parent.display()))?;
    }
    std::fs::write(path, format!("{json}\n"))
        .with_context(|| format!("cannot write file {}", path.display()))
}

/// Parse and validate every case, then report how many there are.
///
/// This is the part of the harness CI can run with no API key and no binary:
/// a broken case file, a fixture that moved, and an invalid pattern all fail
/// here instead of halfway through a paid run.
fn validate(dir: &std::path::Path) -> anyhow::Result<()> {
    let suite = EvalSuite::load(dir)?;
    let fingerprint = suite.fingerprint()?;
    println!("{} case(s) valid in {}", suite.cases().len(), dir.display());
    println!("suite={fingerprint}");
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
            let binary = args.binary.unwrap_or_else(tact_eval::default_cli_binary);
            Box::new(CliRunner::new(binary).with_options(options))
        }
    };

    let report = run_suite(&suite, runner.as_ref(), args.attempts).await?;
    eprintln!("{}", report.to_text());

    match args.json_out {
        Some(path) => write_json(&path, &report.to_json()?)
            .with_context(|| format!("cannot write report {}", path.display()))?,
        None => println!("{}", report.to_json()?),
    }

    if args.strict && report.cases_passing_all() < report.summaries().len() {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run with no `--permission-mode` must be `auto`.
    ///
    /// `PermissionMode::default()` is the product's `Default`, which asks
    /// before a write. Headless cannot answer, so taking that default would
    /// silently deny every tool the suite needs.
    #[test]
    fn an_unstated_permission_mode_is_auto_not_the_product_default() {
        let options = OptionArgs::default().run_options();
        assert_eq!(options.permission_mode, PermissionMode::Auto);
    }

    #[test]
    fn a_stated_permission_mode_wins() {
        let options = OptionArgs {
            permission_mode: Some(PermissionMode::Plan),
            ..Default::default()
        }
        .run_options();
        assert_eq!(options.permission_mode, PermissionMode::Plan);
    }

    #[test]
    fn the_flags_are_the_ones_tact_ui_parses() {
        // The flags this crate builds and the flags `CliArgs` accepts are one
        // contract; a rename on either side fails here rather than at run time.
        let flags = RunOptions {
            provider: Some("openai".into()),
            model: Some("m".into()),
            base_url: Some("http://x".into()),
            permission_mode: PermissionMode::Auto,
            ..Default::default()
        }
        .to_flags();
        tact::config::CliArgs::try_parse_from(std::iter::once("tact-ui".to_string()).chain(flags))
            .expect("every emitted flag is accepted by CliArgs");
    }
}
