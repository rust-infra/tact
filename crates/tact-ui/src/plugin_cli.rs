use anyhow::Result;
use tact::config::{MarketplaceSubcommand, PluginSubcommand};
use tact::consts::PluginHome;
use tact::plugin::{PluginRequest, PluginResult, execute_request};

/// Runs a plugin CLI command and prints the result to stdout.
pub async fn run_plugin_cli(command: PluginSubcommand) -> Result<()> {
    let home = PluginHome::from_environment()
        .expect("HOME must be set for plugin operations");

    match command {
        PluginSubcommand::List => {
            let result = execute_request(home, PluginRequest::List)?;
            print_result(&result);
        }
        PluginSubcommand::Install { spec } => {
            // Parse "<name>@<marketplace>" format
            let (plugin, marketplace) = spec
                .split_once('@')
                .map(|(p, m)| (p.to_owned(), m.to_owned()))
                .unwrap_or_else(|| {
                    // Treat bare name as name@claude-plugins-official
                    (spec.clone(), "claude-plugins-official".to_owned())
                });
            let result = execute_request(
                home,
                PluginRequest::Install {
                    plugin,
                    marketplace,
                },
            )?;
            print_result(&result);
        }
        PluginSubcommand::Reload => {
            let result = execute_request(home, PluginRequest::Reload)?;
            print_result(&result);
        }
        PluginSubcommand::Marketplace { command } => match command {
            MarketplaceSubcommand::Add { source } => {
                let result = execute_request(home, PluginRequest::MarketplaceAdd { source })?;
                print_result(&result);
            }
            MarketplaceSubcommand::List => {
                let result = execute_request(home, PluginRequest::MarketplaceList)?;
                print_result(&result);
            }
            MarketplaceSubcommand::Update { name } => {
                let result = execute_request(
                    home,
                    PluginRequest::MarketplaceUpdate { name },
                )?;
                print_result(&result);
            }
            MarketplaceSubcommand::Remove { name } => {
                let result = execute_request(
                    home,
                    PluginRequest::MarketplaceRemove { name },
                )?;
                print_result(&result);
            }
        },
    }
    Ok(())
}

fn print_result(result: &PluginResult) {
    match result {
        PluginResult::Installed { plugin, marketplace } => {
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
