use anyhow::Result;
use tact::config::{MarketplaceSubcommand, PluginSubcommand};
use tact::consts::PluginHome;
use tact::plugin::{PluginRequest, PluginResult, execute_request};

/// Runs a plugin CLI command and prints the result to stdout.
pub async fn run_plugin_cli(command: PluginSubcommand) -> Result<()> {
    let home = PluginHome::from_environment().expect("HOME must be set for plugin operations");

    let result = tokio::task::spawn_blocking(move || {
        let (request, no_block_on) = build_request(command)?;
        if no_block_on {
            execute_request(home, request)
        } else {
            execute_request(home, request)
        }
    })
    .await??;

    print_result(&result);
    Ok(())
}

/// Build a PluginRequest from the CLI subcommand.
/// Returns (request, _) where the second element is unused but kept for
/// potential future optimization.
fn build_request(command: PluginSubcommand) -> Result<(PluginRequest, bool)> {
    match command {
        PluginSubcommand::List => Ok((PluginRequest::List, true)),
        PluginSubcommand::Install { spec } => {
            let (plugin, marketplace) = spec
                .split_once('@')
                .map(|(p, m)| (p.to_owned(), m.to_owned()))
                .unwrap_or_else(|| (spec.clone(), "claude-plugins-official".to_owned()));
            Ok((
                PluginRequest::Install {
                    plugin,
                    marketplace,
                },
                false,
            ))
        }
        PluginSubcommand::Reload => Ok((PluginRequest::Reload, true)),
        PluginSubcommand::Marketplace { command } => match command {
            MarketplaceSubcommand::Add { source } => {
                Ok((PluginRequest::MarketplaceAdd { source }, false))
            }
            MarketplaceSubcommand::List => Ok((PluginRequest::MarketplaceList, true)),
            MarketplaceSubcommand::Update { name } => {
                Ok((PluginRequest::MarketplaceUpdate { name }, false))
            }
            MarketplaceSubcommand::Remove { name } => {
                Ok((PluginRequest::MarketplaceRemove { name }, true))
            }
        },
    }
}

fn print_result(result: &PluginResult) {
    match result {
        PluginResult::Installed {
            plugin,
            marketplace,
        } => {
            println!("Installed plugin '{plugin}' from '{marketplace}'");
        }
        PluginResult::ListedInstalled { plugins } if plugins.is_empty() => {
            println!("No plugins installed.");
        }
        PluginResult::ListedInstalled { plugins } => {
            println!("Installed plugins:");
            for p in plugins {
                println!("  {}:{} rev={}", p.marketplace, p.id, p.revision);
            }
        }
        PluginResult::Reloaded { count } => {
            println!("Reloaded {count} plugin(s).");
        }
        PluginResult::MarketplaceAdded { marketplace } => {
            println!("Added marketplace '{marketplace}'");
        }
        PluginResult::ListedMarketplaces { marketplaces } if marketplaces.is_empty() => {
            println!("No marketplaces registered.");
        }
        PluginResult::ListedMarketplaces { marketplaces } => {
            println!("Registered marketplaces:");
            for m in marketplaces {
                println!("  {}: {}", m.name, m.source.git_url());
            }
        }
        PluginResult::MarketplaceUpdated { marketplace, count } => {
            println!("Updated marketplace '{marketplace}': {count} plugins");
        }
        PluginResult::MarketplaceRemoved { marketplace } => {
            println!("Removed marketplace '{marketplace}'");
        }
    }
}
