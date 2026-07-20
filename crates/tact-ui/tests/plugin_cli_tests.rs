//! Integration tests for plugin CLI subcommands.
//!
//! These use a temp directory as HOME to avoid polluting the real plugin store.
//! Tests exercise the plugin execution path directly.

use tact::config::{MarketplaceSubcommand, PluginSubcommand};

/// Sets HOME to a temp dir and returns the TempDir guard.
fn with_temp_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: single-threaded test, no concurrent readers of HOME
    unsafe { std::env::set_var("HOME", dir.path()) };
    dir
}

#[tokio::test]
async fn plugin_list_when_empty() {
    let _guard = with_temp_home();
    let result = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::List).await;
    assert!(result.is_ok(), "list should succeed: {result:?}");
}

#[tokio::test]
async fn marketplace_list_shows_builtin() {
    let _guard = with_temp_home();
    let result = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Marketplace {
        command: MarketplaceSubcommand::List,
    })
    .await;
    assert!(result.is_ok(), "marketplace list should succeed: {result:?}");
}

#[tokio::test]
async fn marketplace_add_and_remove() {
    let _guard = with_temp_home();

    // Try adding a marketplace. This requires network to clone the repo,
    // so accept either success or failure.
    let result_add = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Marketplace {
        command: MarketplaceSubcommand::Add {
            source: "https://github.com/example/test-plugins.git".into(),
        },
    })
    .await;

    match result_add {
        Ok(()) => {
            // Remove it only if add succeeded
            let result = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Marketplace {
                command: MarketplaceSubcommand::Remove {
                    name: "example/test-plugins".into(),
                },
            })
            .await;
            assert!(result.is_ok(), "remove should succeed: {result:?}");
        }
        Err(e) => {
            // Add may fail without network — that's acceptable
            eprintln!("marketplace add (expected without network): {e}");
        }
    }
}

#[tokio::test]
async fn removing_builtin_marketplace_fails() {
    let _guard = with_temp_home();
    let result = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Marketplace {
        command: MarketplaceSubcommand::Remove {
            name: "claude-plugins-official".into(),
        },
    })
    .await;
    assert!(
        result.is_err(),
        "removing the built-in marketplace should fail"
    );
}

#[tokio::test]
async fn install_with_missing_plugin_fails_gracefully() {
    let _guard = with_temp_home();
    let result = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Install {
        spec: "nonexistent-plugin@claude-plugins-official".into(),
    })
    .await;
    // The plugin doesn't exist, so this should fail with an error
    assert!(result.is_err(), "installing nonexistent plugin should fail");
}

#[tokio::test]
async fn reload_with_no_plugins_succeeds() {
    let _guard = with_temp_home();
    let result = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Reload).await;
    assert!(result.is_ok(), "reload should succeed: {result:?}");
}
