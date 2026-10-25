use std::sync::{Arc, Mutex};

use tact_kernel::capability::command::{Command, CommandMeta, CommandScope};
use tact_kernel::capability::prompt::{PromptContext, PromptSection};
use tact_kernel::capability::tool::Tool;
use tact_kernel::capability::view::View;
use tact_kernel::capability::{CapabilityFilter, CapabilityId, CapabilityKind, SkippedCapability};
use tact_kernel::plugin::{Plugin, PluginHost, PluginId, PluginKind, PluginManifest};
use tact_kernel::tool::ToolMetadata;
use tact_kernel::tool::descriptor::ToolDescriptor;

struct EchoTool;
impl Tool for EchoTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor::from_native(
            "echo",
            "echo",
            serde_json::json!({"type": "object"}),
            &ToolMetadata::read_json("echo", "echo", "Echo"),
        )
    }
}

struct EchoCommand;
impl Command for EchoCommand {
    fn meta(&self) -> &CommandMeta {
        static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
        META.get_or_init(|| CommandMeta {
            name: "echo".into(),
            aliases: vec![],
            summary: "echo".into(),
            usage: "echo".into(),
            scope: CommandScope::Runtime,
            needs_args: false,
        })
    }
}

struct EchoPrompt;
impl PromptSection for EchoPrompt {
    fn id(&self) -> &'static str {
        "echo"
    }
    fn priority(&self) -> i32 {
        10
    }
    fn render(&self, _ctx: &PromptContext) -> Option<String> {
        Some("echo".into())
    }
}

struct EchoView;
impl View for EchoView {
    fn id(&self) -> &'static str {
        "echo"
    }
}

fn manifest(id: &str) -> PluginManifest {
    PluginManifest::new(
        PluginId::new(id).unwrap(),
        "Echo",
        "1.0.0",
        PluginKind::Capability,
        "conformance plugin",
    )
}

struct EchoPlugin {
    id: &'static str,
    fail: bool,
    shutdowns: Arc<Mutex<Vec<String>>>,
    contributions: bool,
}

#[async_trait::async_trait]
impl Plugin for EchoPlugin {
    fn manifest(&self) -> PluginManifest {
        manifest(self.id)
    }

    fn register(&self, reg: &mut tact_kernel::plugin::Registrar<'_>) -> anyhow::Result<()> {
        if self.fail {
            anyhow::bail!("boom from {}", self.id);
        }
        if self.contributions {
            reg.tool(EchoTool)?;
            reg.command(EchoCommand)?;
            reg.prompt_section(EchoPrompt)?;
            reg.view(EchoView)?;
        }
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.shutdowns.lock().unwrap().push(self.id.to_string());
        Ok(())
    }
}

#[tokio::test]
async fn fake_plugin_registers_all_four_contributions() {
    let mut host = PluginHost::new(CapabilityFilter::default());
    host.install(EchoPlugin {
        id: "echo.plugin",
        fail: false,
        shutdowns: Arc::default(),
        contributions: true,
    })
    .await
    .unwrap();
    let runtime = host.seal().await.unwrap();
    assert!(runtime.tools().get("echo").is_some());
    assert!(runtime.commands().get("echo").is_some());
    assert_eq!(runtime.prompts().sections().len(), 1);
    assert!(runtime.views().get("echo").is_some());
}

#[tokio::test]
async fn duplicate_tool_fails_install_and_keeps_first_catalog() {
    let mut host = PluginHost::new(CapabilityFilter::default());
    host.install(EchoPlugin {
        id: "first",
        fail: false,
        shutdowns: Arc::default(),
        contributions: true,
    })
    .await
    .unwrap();
    let err = host
        .install(EchoPlugin {
            id: "second",
            fail: false,
            shutdowns: Arc::default(),
            contributions: true,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("second"));
    let runtime = host.seal().await.unwrap();
    assert_eq!(runtime.tools().len(), 1);
}

#[tokio::test]
async fn disabled_capability_is_absent_and_recorded() {
    let disabled: CapabilityId = "tool:echo".parse().unwrap();
    let mut host = PluginHost::new(CapabilityFilter::new([disabled]));
    host.install(EchoPlugin {
        id: "echo.plugin",
        fail: false,
        shutdowns: Arc::default(),
        contributions: true,
    })
    .await
    .unwrap();
    let runtime = host.seal().await.unwrap();
    assert!(runtime.tools().get("echo").is_none());
    assert_eq!(runtime.skipped().len(), 1);
    assert_eq!(runtime.skipped()[0].capability.kind, CapabilityKind::Tool);
}

#[tokio::test]
async fn failed_register_poisons_host_and_shuts_previous_plugins_down() {
    let shutdowns = Arc::new(Mutex::new(Vec::new()));
    let mut host = PluginHost::new(CapabilityFilter::default());
    host.install(EchoPlugin {
        id: "first",
        fail: false,
        shutdowns: shutdowns.clone(),
        contributions: true,
    })
    .await
    .unwrap();
    assert!(
        host.install(EchoPlugin {
            id: "boom",
            fail: true,
            shutdowns: shutdowns.clone(),
            contributions: true
        })
        .await
        .is_err()
    );
    assert!(host.seal().await.is_err());
    assert_eq!(*shutdowns.lock().unwrap(), vec!["first", "boom"]);
}

#[tokio::test]
async fn shutdown_is_reverse_order_and_idempotent() {
    let shutdowns = Arc::new(Mutex::new(Vec::new()));
    let mut host = PluginHost::new(CapabilityFilter::default());
    host.install(EchoPlugin {
        id: "first",
        fail: false,
        shutdowns: shutdowns.clone(),
        contributions: true,
    })
    .await
    .unwrap();
    host.install(EchoPlugin {
        id: "second",
        fail: false,
        shutdowns: shutdowns.clone(),
        contributions: false,
    })
    .await
    .unwrap();
    let mut runtime = host.seal().await.unwrap();
    runtime.shutdown().await.unwrap();
    runtime.shutdown().await.unwrap();
    assert_eq!(*shutdowns.lock().unwrap(), vec!["second", "first"]);
}

struct NamelessTool;
impl Tool for NamelessTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor::from_native(
            "",
            "no name",
            serde_json::json!({"type": "object"}),
            &ToolMetadata::read_json("", "no name", "No Name"),
        )
    }
}

struct NamelessPlugin;
#[async_trait::async_trait]
impl Plugin for NamelessPlugin {
    fn manifest(&self) -> PluginManifest {
        manifest("nameless")
    }

    fn register(&self, reg: &mut tact_kernel::plugin::Registrar<'_>) -> anyhow::Result<()> {
        reg.tool(NamelessTool)
    }
}

#[tokio::test]
async fn empty_capability_name_is_rejected_at_registration() {
    let mut host = PluginHost::new(CapabilityFilter::default());
    let err = host.install(NamelessPlugin).await.unwrap_err();
    assert!(err.to_string().contains("capability name cannot be empty"));
    assert!(host.seal().await.is_err());
}

#[test]
fn skipped_capability_has_owning_plugin() {
    let id = PluginId::new("echo.plugin").unwrap();
    let cap: CapabilityId = "tool:echo".parse().unwrap();
    let skipped = SkippedCapability {
        plugin: id.clone(),
        capability: cap,
    };
    assert_eq!(skipped.plugin, id);
}

#[tokio::test]
async fn duplicate_plugin_id_is_rejected_and_does_not_leak_shutdown() {
    let shutdowns = Arc::new(Mutex::new(Vec::new()));
    let mut host = PluginHost::new(CapabilityFilter::default());
    host.install(EchoPlugin {
        id: "same",
        fail: false,
        shutdowns: shutdowns.clone(),
        contributions: true,
    })
    .await
    .unwrap();
    let err = host
        .install(EchoPlugin {
            id: "same",
            fail: false,
            shutdowns: shutdowns.clone(),
            contributions: true,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("duplicate plugin id"));
    assert_eq!(
        shutdowns.lock().unwrap().len(),
        1,
        "a duplicate install must shut down only the rejected plugin, not the accepted one"
    );
}

#[tokio::test]
async fn seal_cannot_run_twice() {
    let mut host = PluginHost::new(CapabilityFilter::default());
    host.install(EchoPlugin {
        id: "echo.plugin",
        fail: false,
        shutdowns: Arc::default(),
        contributions: true,
    })
    .await
    .unwrap();
    host.seal().await.unwrap();
    assert!(host.seal().await.is_err());
    assert!(
        host.install(EchoPlugin {
            id: "late",
            fail: false,
            shutdowns: Arc::default(),
            contributions: false,
        })
        .await
        .is_err()
    );
}
