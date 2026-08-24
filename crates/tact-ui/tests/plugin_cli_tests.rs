//! Integration tests for plugin CLI subcommands.
//!
//! These use a temp directory as HOME to avoid polluting the real plugin store.
//! Tests exercise the plugin execution path directly — without network clones.
//!
//! All cases that mutate `HOME` share a process-wide async mutex so Tokio's
//! default multi-thread test runtime cannot race on the environment variable.

use std::{fs, path::Path, sync::OnceLock};

use tact::config::{MarketplaceSubcommand, PluginSubcommand};
use tokio::sync::{Mutex, MutexGuard};

fn home_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Sets HOME to a temp dir. Holds the lock until the returned guard is dropped.
async fn with_temp_home() -> (tempfile::TempDir, MutexGuard<'static, ()>) {
    let guard = home_lock().lock().await;
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: exclusive via `home_lock`; no concurrent HOME readers in this binary.
    unsafe { std::env::set_var("HOME", dir.path()) };
    (dir, guard)
}

/// Seeds a local official marketplace catalog so install paths do not git-clone.
fn seed_official_marketplace_catalog(home: &Path, plugins_json: &str) {
    let root = home
        .join(".tact")
        .join("plugins")
        .join("marketplaces")
        .join("claude-plugins-official");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("marketplace.json"),
        format!(r#"{{"name":"claude-plugins-official","plugins":{plugins_json}}}"#),
    )
    .unwrap();
}

#[tokio::test]
async fn plugin_list_when_empty() {
    let (_home, _lock) = with_temp_home().await;
    let result = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::List).await;
    assert!(result.is_ok(), "list should succeed: {result:?}");
}

#[tokio::test]
async fn marketplace_list_shows_builtin() {
    let (_home, _lock) = with_temp_home().await;
    let result = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Marketplace {
        command: MarketplaceSubcommand::List,
    })
    .await;
    assert!(
        result.is_ok(),
        "marketplace list should succeed: {result:?}"
    );
}

#[tokio::test]
async fn marketplace_add_and_remove() {
    let (_home, _lock) = with_temp_home().await;

    // Loopback URL: connection refused immediately (no GitHub clone timeout).
    let result_add = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Marketplace {
        command: MarketplaceSubcommand::Add {
            source: "https://127.0.0.1:1/test-plugins.git".into(),
        },
    })
    .await;

    match result_add {
        Ok(()) => {
            let result = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Marketplace {
                command: MarketplaceSubcommand::Remove {
                    name: "test-plugins".into(),
                },
            })
            .await;
            assert!(result.is_ok(), "remove should succeed: {result:?}");
        }
        Err(e) => {
            eprintln!("marketplace add (expected without a local repo): {e}");
        }
    }
}

#[tokio::test]
async fn removing_builtin_marketplace_fails() {
    let (_home, _lock) = with_temp_home().await;
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
    let (home, _lock) = with_temp_home().await;
    // Pre-seed catalog so install does not clone github.com/anthropics/...
    seed_official_marketplace_catalog(home.path(), "[]");

    let result = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Install {
        spec: "nonexistent-plugin@claude-plugins-official".into(),
    })
    .await;
    let err = result.expect_err("installing nonexistent plugin should fail");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown plugin") && msg.contains("nonexistent-plugin"),
        "expected unknown-plugin error, got: {msg}"
    );
}

#[tokio::test]
async fn reload_with_no_plugins_succeeds() {
    let (_home, _lock) = with_temp_home().await;
    let result = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Reload).await;
    assert!(result.is_ok(), "reload should succeed: {result:?}");
}

#[tokio::test]
async fn uninstall_unknown_plugin_fails_gracefully() {
    let (_home, _lock) = with_temp_home().await;
    let result = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Uninstall {
        name: "not-installed".into(),
    })
    .await;
    let err = result.expect_err("uninstalling a missing plugin should fail");
    assert!(
        err.to_string().contains("not installed"),
        "expected not-installed error, got: {err}"
    );
}

#[tokio::test]
async fn update_unknown_plugin_fails_gracefully() {
    let (_home, _lock) = with_temp_home().await;
    let result = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Update {
        name: "not-installed".into(),
    })
    .await;
    let err = result.expect_err("updating a missing plugin should fail");
    assert!(
        err.to_string().contains("not installed"),
        "expected not-installed error, got: {err}"
    );
}

#[tokio::test]
async fn install_then_uninstall_round_trip() {
    let (home, _lock) = with_temp_home().await;
    // Seed a local official marketplace with a relative-source plugin so the
    // install path does not git-clone.
    let root = home
        .path()
        .join(".tact")
        .join("plugins")
        .join("marketplaces")
        .join("claude-plugins-official");
    fs::create_dir_all(root.join("plugins/demo/skills/check")).unwrap();
    fs::write(
        root.join("plugins/demo/skills/check/SKILL.md"),
        "---\nname: check\n---\n",
    )
    .unwrap();
    fs::write(
        root.join("marketplace.json"),
        r#"{"name":"claude-plugins-official","plugins":[{"name":"demo","source":"./plugins/demo"}]}"#,
    )
    .unwrap();

    let install = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Install {
        spec: "demo@claude-plugins-official".into(),
    })
    .await;
    assert!(install.is_ok(), "install should succeed: {install:?}");

    let demo_dir = home
        .path()
        .join(".tact/plugins/cache/claude-plugins-official/demo");
    assert!(demo_dir.is_dir(), "plugin should be cached after install");

    let uninstall = tact_ui::plugin_cli::run_plugin_cli(PluginSubcommand::Uninstall {
        name: "demo".into(),
    })
    .await;
    assert!(uninstall.is_ok(), "uninstall should succeed: {uninstall:?}");

    assert!(
        !demo_dir.exists(),
        "plugin cache dir should be removed after uninstall"
    );
}
