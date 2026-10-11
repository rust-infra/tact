//! Installed plugins that declare a runtime half, hosted for one session.
//!
//! The `.codex-plugin/plugin.json` an installed package already carries may add
//! a `runtime` object naming a program to spawn. At session bootstrap every such
//! plugin is started through [`StdioPluginHost`], the capabilities it declares at
//! handshake are registered on the session's serving router, and its manifest is
//! registered in the session's Kernel plugin registry; the session stops it at
//! teardown. The manifest names the command, so one stdio path serves a Node
//! plugin (`node index.mjs`), a configured WASM runner, and a compiled binary
//! alike — the language host crates stay thin layers over the same envelope
//! protocol.
//!
//! Nothing here makes a plugin capability a model-visible tool: tool dispatch is
//! a separate per-wave path. A hosted capability is reachable through the serving
//! router, which is the product path for the `Command` / `App` kinds.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use serde::Deserialize;
use tact::{CapabilityRouter, KernelError, PluginRegistry, RuntimePluginManifest};
use tact_plugin_host::{DenyHostCalls, PluginHost, StdioPluginHost, register_host_with_router};
use tact_protocol::{PluginId, ProtocolVersion};

use crate::consts::PluginHome;

use super::PluginStore;

/// The `runtime` object of an installed plugin's manifest.
///
/// `deny_unknown_fields` matches the other protocol-adjacent formats: a typo'd
/// key is a package bug to surface at startup, not a setting to ignore silently.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginRuntimeSpec {
    /// The protocol [`PluginId`]; its deserializer validates the shape.
    pub id: PluginId,
    /// A program on `PATH` or a path, passed to the spawner verbatim.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

impl PluginRuntimeSpec {
    /// The command and args to spawn, with relative args pinned to the plugin
    /// root.
    ///
    /// A package ships its own entry point, so `args: ["plugin.mjs"]` must
    /// resolve inside the package and not against the session's working
    /// directory. An arg is rewritten only when it names a file that exists
    /// under the root; a flag or a bare value (`--port`, `serve`) is left as-is,
    /// so no heuristic has to guess which args are paths.
    #[must_use]
    pub fn resolve(&self, root: &Path) -> (PathBuf, Vec<String>) {
        let args = self
            .args
            .iter()
            .map(|arg| {
                let candidate = root.join(arg);
                if Path::new(arg).is_relative() && candidate.is_file() {
                    candidate.to_string_lossy().into_owned()
                } else {
                    arg.clone()
                }
            })
            .collect();
        (PathBuf::from(&self.command), args)
    }
}

/// Reads the `runtime` half of an installed plugin's manifest, if it declares
/// one.
///
/// `None` when the plugin ships no manifest or no `runtime` object — the normal
/// case for a plugin that only contributes skills, commands, hooks or MCP
/// servers. The manifest's other keys are ignored here; only `runtime` is
/// parsed, and a malformed one is an error the caller can surface.
pub fn read_runtime_spec(root: &Path) -> Result<Option<PluginRuntimeSpec>> {
    #[derive(Deserialize)]
    struct Manifest {
        #[serde(default)]
        runtime: Option<PluginRuntimeSpec>,
    }

    let manifest_path = root.join(".codex-plugin").join("plugin.json");
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let content = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read plugin manifest {}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_str(&content).with_context(|| {
        format!(
            "failed to parse plugin manifest {}",
            manifest_path.display()
        )
    })?;
    Ok(manifest.runtime)
}

/// Every installed plugin under `home` that declares a runtime half, paired with
/// its package root.
///
/// `PluginStore::installed_plugin_roots` already validates the cache path, so a
/// malformed install record never reaches here. A plugin whose `runtime` object
/// does not parse is *skipped with a message* rather than failing the scan: one
/// mistyped key in one package must not stop the session hosting every other
/// plugin, the same rule `start` and `register` follow per plugin.
pub fn discover_runtime_plugins(
    home: &PluginHome,
) -> (Vec<(PluginRuntimeSpec, PathBuf)>, Vec<String>) {
    let store = PluginStore::new(home.clone());
    let roots = match store.installed_plugin_roots() {
        Ok(roots) => roots,
        Err(error) => {
            return (
                Vec::new(),
                vec![format!("plugin state unreadable: {error:#}")],
            );
        }
    };
    let mut found = Vec::new();
    let mut failures = Vec::new();
    for root in roots {
        match read_runtime_spec(&root.root) {
            Ok(Some(spec)) => found.push((spec, root.root)),
            Ok(None) => {}
            Err(error) => failures.push(format!("{}: {error:#}", root.plugin_id)),
        }
    }
    (found, failures)
}

/// One started host and the manifest it declares.
pub struct HostedPlugin {
    host: Arc<tokio::sync::Mutex<dyn PluginHost>>,
    manifest: RuntimePluginManifest,
}

impl HostedPlugin {
    #[must_use]
    pub fn new(
        host: Arc<tokio::sync::Mutex<dyn PluginHost>>,
        manifest: RuntimePluginManifest,
    ) -> Self {
        Self { host, manifest }
    }

    #[must_use]
    pub fn plugin_id(&self) -> &PluginId {
        &self.manifest.id
    }
}

/// The installed plugins one session is hosting.
///
/// Built once at bootstrap and carried by the Agent so both frontends can stop
/// it. It is deliberately not a supervisor: a crashed plugin stays failed and
/// nothing restarts it — that is a product decision this does not make.
pub struct HostedPlugins {
    hosts: Vec<HostedPlugin>,
    /// One line per plugin skipped during `start` / `register`. The bootstrap
    /// drains these into the session's notices; a broken plugin must be visible
    /// without ever failing the session.
    notices: Vec<String>,
    /// The session registry, kept so `shutdown` can mark each plugin stopped.
    /// `None` until `register` runs.
    registry: Option<PluginRegistry>,
}

impl HostedPlugins {
    /// Starts every plugin that declares a runtime half.
    ///
    /// A plugin whose spawn, handshake or capability registration fails is
    /// recorded in [`Self::notice_lines`] and skipped: one broken package must
    /// not take the session down, so this cannot fail.
    pub async fn start(specs: impl IntoIterator<Item = (PluginRuntimeSpec, PathBuf)>) -> Self {
        let mut hosts = Vec::new();
        let mut notices = Vec::new();
        for (spec, root) in specs {
            let (program, args) = spec.resolve(&root);
            match StdioPluginHost::start(program, &args, spec.id.clone(), ProtocolVersion::CURRENT)
                .await
            {
                Ok(host) => hosts.push(hosted_from_stdio(host)),
                Err(error) => notices.push(format!(
                    "plugin {} failed to start from {}: {error:#}",
                    spec.id,
                    root.display()
                )),
            }
        }
        Self {
            hosts,
            notices,
            registry: None,
        }
    }

    /// No hosts: discovery found none, or the plugin home is not resolvable.
    #[must_use]
    pub fn empty() -> Self {
        Self::from_hosts(Vec::new())
    }

    /// Assembles hosts directly, bypassing `start`.
    ///
    /// `start` uses it once each host has handshaken; a test injects a
    /// [`PluginHost`] double through it so registration runs through the same
    /// [`register_host_with_router`] path production does.
    #[must_use]
    pub fn from_hosts(hosts: Vec<HostedPlugin>) -> Self {
        Self {
            hosts,
            notices: Vec::new(),
            registry: None,
        }
    }

    /// Registers each host's manifest and capabilities on the session.
    ///
    /// The manifest registers first: a duplicate plugin id is caught before the
    /// router is touched, so there is nothing to roll back. If capability
    /// registration then fails (a name already on the router), the manifest is
    /// unregistered and the host stopped — a plugin that supplies no working
    /// capability is not left running, and the router's duplicate rules stay
    /// intact. Either failure is recorded, never returned: the rest of the
    /// session must still start.
    pub async fn register(&mut self, router: &CapabilityRouter, registry: &PluginRegistry) {
        self.registry = Some(registry.clone());
        let mut kept = Vec::with_capacity(self.hosts.len());
        for hosted in std::mem::take(&mut self.hosts) {
            match register_host(router, registry, &hosted) {
                Ok(()) => kept.push(hosted),
                Err(error) => {
                    self.notices.push(format!(
                        "plugin {} was not registered: {error}",
                        hosted.manifest.id
                    ));
                    hosted.host.lock().await.shutdown().await.ok();
                }
            }
        }
        self.hosts = kept;
    }

    /// The startup and registration findings, in the order they occurred, for
    /// the session's notices.
    pub fn notice_lines(&self) -> impl Iterator<Item = &str> {
        self.notices.iter().map(String::as_str)
    }

    /// Stops every hosted plugin and marks it stopped in the registry.
    ///
    /// Called at session teardown next to `shutdown_mcp`: a plugin process must
    /// not outlive the session.
    pub async fn shutdown(&self) {
        for hosted in &self.hosts {
            hosted.host.lock().await.shutdown().await.ok();
            if let Some(registry) = &self.registry {
                registry.stop(&hosted.manifest.id).ok();
            }
        }
    }
}

/// Wraps a started stdio host behind the [`PluginHost`] boundary and lifts the
/// capabilities it declared at handshake into a manifest.
fn hosted_from_stdio(host: StdioPluginHost) -> HostedPlugin {
    let manifest = RuntimePluginManifest {
        id: host.plugin_id().clone(),
        // The installed-plugin runtime declaration carries no plugin version;
        // the registry requires a non-empty one, so the negotiated protocol
        // revision serves as a stable, truthful stand-in.
        version: format!("{}.{}", host.protocol().major, host.protocol().minor),
        protocol: host.protocol(),
        capabilities: host.capabilities().to_vec(),
        dependencies: Vec::new(),
    };
    let host: Arc<tokio::sync::Mutex<dyn PluginHost>> = Arc::new(tokio::sync::Mutex::new(host));
    HostedPlugin::new(host, manifest)
}

/// Registers one host on the session, rolling the manifest back if its
/// capabilities cannot be registered.
fn register_host(
    router: &CapabilityRouter,
    registry: &PluginRegistry,
    hosted: &HostedPlugin,
) -> Result<(), KernelError> {
    registry.register(hosted.manifest.clone())?;
    if let Err(error) =
        register_host_with_router(Arc::clone(&hosted.host), router, Arc::new(DenyHostCalls))
    {
        registry.unregister(&hosted.manifest.id).ok();
        return Err(error);
    }
    registry.start(&hosted.manifest.id)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };

    use async_trait::async_trait;
    use serde_json::{Value, json};
    use tact::{
        InvocationContext, PermissionService, PluginState, RuntimeContext, RuntimeServices,
    };
    use tact_protocol::{
        CapabilityDeclaration, CapabilityKind, CapabilityRisk, PluginRequest, PluginResponse,
        RequestId,
    };
    use tempfile::tempdir;

    use super::*;
    use crate::plugin::{InstalledPlugin, InstalledState};

    fn declaration(name: &str) -> CapabilityDeclaration {
        CapabilityDeclaration {
            name: name.into(),
            kind: CapabilityKind::Command,
            version: "1".into(),
            description: None,
            input_schema: None,
            output_schema: None,
            risk: CapabilityRisk::ReadOnly,
        }
    }

    /// Writes `<root>/.codex-plugin/plugin.json` with the given body.
    fn write_manifest(root: &Path, body: &str) {
        fs::create_dir_all(root.join(".codex-plugin")).unwrap();
        fs::write(root.join(".codex-plugin/plugin.json"), body).unwrap();
    }

    // ── manifest parsing ────────────────────────────────────────────────

    #[test]
    fn read_runtime_spec_parses_a_declared_runtime_half() {
        let root = tempdir().unwrap();
        write_manifest(
            root.path(),
            r#"{"name":"echo","runtime":{"id":"acme.echo","command":"node","args":["plugin.mjs"]}}"#,
        );

        let spec = read_runtime_spec(root.path())
            .unwrap()
            .expect("runtime half");

        assert_eq!(spec.id, PluginId::from("acme.echo"));
        assert_eq!(spec.command, "node");
        assert_eq!(spec.args, vec!["plugin.mjs".to_string()]);
    }

    #[test]
    fn read_runtime_spec_is_none_without_a_manifest_or_a_runtime_key() {
        let absent = tempdir().unwrap();
        assert!(read_runtime_spec(absent.path()).unwrap().is_none());

        let skills_only = tempdir().unwrap();
        write_manifest(
            skills_only.path(),
            r#"{"name":"review","hooks":"./hooks.json"}"#,
        );
        assert!(read_runtime_spec(skills_only.path()).unwrap().is_none());
    }

    #[test]
    fn read_runtime_spec_rejects_an_unknown_field_inside_runtime() {
        let root = tempdir().unwrap();
        // The top-level manifest may carry hooks/mcpServers/...; `runtime` may
        // not carry anything this reader does not understand.
        write_manifest(
            root.path(),
            r#"{"name":"echo","runtime":{"id":"acme.echo","command":"node","typo":true}}"#,
        );

        assert!(read_runtime_spec(root.path()).is_err());
    }

    #[test]
    fn relative_args_resolve_against_the_plugin_root_only_where_a_file_exists() {
        let root = tempdir().unwrap();
        fs::write(root.path().join("plugin.mjs"), "// entry").unwrap();
        let spec = PluginRuntimeSpec {
            id: PluginId::from("acme.echo"),
            command: "node".into(),
            args: vec![
                "plugin.mjs".into(),
                "--flag".into(),
                "serve".into(),
                "/etc/hosts".into(),
            ],
        };

        let (program, args) = spec.resolve(root.path());

        assert_eq!(program, PathBuf::from("node"));
        assert_eq!(
            args,
            vec![
                root.path()
                    .join("plugin.mjs")
                    .to_string_lossy()
                    .into_owned(),
                "--flag".to_string(),
                "serve".to_string(),
                "/etc/hosts".to_string(),
            ]
        );
    }

    // ── discovery over a temp plugin home ───────────────────────────────

    fn installed(id: &str, cache_path: &Path, revision: &str) -> InstalledPlugin {
        InstalledPlugin {
            id: id.to_string(),
            marketplace: "acme".to_string(),
            revision: revision.to_string(),
            cache_path: cache_path.to_path_buf(),
            skill_count: 0,
            command_count: 0,
            has_hooks: false,
            has_mcp: false,
        }
    }

    #[test]
    fn discover_runtime_plugins_returns_only_plugins_that_declare_a_runtime_half() {
        let home = tempdir().unwrap();
        let plugin_home = PluginHome::from_home(home.path());
        // The store only trusts a cache root that resolves inside the cache, so
        // the fixture is the real layout: cache/acme/<plugin>/<revision>.
        let echo_root = home.path().join(".tact/plugins/cache/acme/echo/abc123");
        fs::create_dir_all(&echo_root).unwrap();
        write_manifest(
            &echo_root,
            r#"{"name":"echo","runtime":{"id":"acme.echo","command":"node","args":["plugin.mjs"]}}"#,
        );
        let plain_root = home.path().join(".tact/plugins/cache/acme/plain/def456");
        fs::create_dir_all(plain_root.join("skills")).unwrap();

        let store = PluginStore::new(plugin_home.clone());
        let state = InstalledState {
            plugins: BTreeMap::from([
                (
                    "acme/echo".to_string(),
                    installed("echo", &echo_root, "abc123"),
                ),
                (
                    "acme/plain".to_string(),
                    installed("plain", &plain_root, "def456"),
                ),
            ]),
        };
        store.commit_install(&state, &echo_root).unwrap();

        let (found, failures) = discover_runtime_plugins(&plugin_home);

        assert!(
            failures.is_empty(),
            "neither plugin's manifest is malformed: {failures:?}"
        );
        assert_eq!(found.len(), 1, "only the runtime-declaring plugin is found");
        assert_eq!(found[0].0.id, PluginId::from("acme.echo"));
        assert_eq!(found[0].1, echo_root.canonicalize().unwrap());
    }

    #[test]
    fn a_malformed_manifest_is_skipped_but_the_healthy_plugins_are_still_found() {
        let home = tempdir().unwrap();
        let plugin_home = PluginHome::from_home(home.path());
        let good_root = home.path().join(".tact/plugins/cache/acme/good/abc123");
        fs::create_dir_all(&good_root).unwrap();
        write_manifest(
            &good_root,
            r#"{"runtime":{"id":"acme.good","command":"node"}}"#,
        );
        let bad_root = home.path().join(".tact/plugins/cache/acme/bad/def456");
        fs::create_dir_all(&bad_root).unwrap();
        write_manifest(
            &bad_root,
            r#"{"runtime":{"id":"acme.bad","command":"node","typo":true}}"#,
        );

        let store = PluginStore::new(plugin_home.clone());
        let state = InstalledState {
            plugins: BTreeMap::from([
                (
                    "acme/good".to_string(),
                    installed("good", &good_root, "abc123"),
                ),
                (
                    "acme/bad".to_string(),
                    installed("bad", &bad_root, "def456"),
                ),
            ]),
        };
        store.commit_install(&state, &good_root).unwrap();

        let (found, failures) = discover_runtime_plugins(&plugin_home);

        assert_eq!(
            found.len(),
            1,
            "one bad manifest must not hide the healthy plugin"
        );
        assert_eq!(found[0].0.id, PluginId::from("acme.good"));
        assert_eq!(failures.len(), 1, "the bad package is reported");
        assert!(
            failures[0].contains("acme/bad"),
            "the report names the package: {failures:?}"
        );
    }

    // ── registration wiring with a host double ──────────────────────────

    /// A `PluginHost` double. Registration reaches it through the *production*
    /// path (`register_host_with_router`), so this exercises the same code a
    /// spawned stdio host would.
    struct DoubleHost {
        id: PluginId,
        capabilities: Vec<CapabilityDeclaration>,
        state: PluginState,
        stopped: Arc<AtomicBool>,
    }

    impl DoubleHost {
        fn new(id: &str, capabilities: Vec<CapabilityDeclaration>) -> Self {
            Self {
                id: PluginId::from(id),
                capabilities,
                state: PluginState::Running,
                stopped: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    #[async_trait]
    impl PluginHost for DoubleHost {
        fn plugin_id(&self) -> &PluginId {
            &self.id
        }

        fn protocol(&self) -> ProtocolVersion {
            ProtocolVersion::CURRENT
        }

        fn capabilities(&self) -> &[CapabilityDeclaration] {
            &self.capabilities
        }

        fn state(&self) -> PluginState {
            self.state
        }

        async fn request(&mut self, request: PluginRequest) -> anyhow::Result<PluginResponse> {
            match request {
                PluginRequest::Invoke { capability, input } => Ok(PluginResponse::Result {
                    output: json!({ "capability": capability, "echo": input }),
                }),
                PluginRequest::Shutdown => Ok(PluginResponse::Result {
                    output: Value::Null,
                }),
                other => Err(anyhow::anyhow!("double cannot handle {other:?}")),
            }
        }

        async fn shutdown(&mut self) -> anyhow::Result<()> {
            self.state = PluginState::Stopped;
            self.stopped.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Builds a hosted double plus the flag its stop records.
    fn hosted_double(
        id: &str,
        capabilities: Vec<CapabilityDeclaration>,
    ) -> (HostedPlugin, Arc<AtomicBool>) {
        let double = DoubleHost::new(id, capabilities);
        let stopped = Arc::clone(&double.stopped);
        let manifest = RuntimePluginManifest {
            id: double.id.clone(),
            version: "1".into(),
            protocol: ProtocolVersion::CURRENT,
            capabilities: double.capabilities.clone(),
            dependencies: Vec::new(),
        };
        let host: Arc<tokio::sync::Mutex<dyn PluginHost>> =
            Arc::new(tokio::sync::Mutex::new(double));
        (HostedPlugin::new(host, manifest), stopped)
    }

    fn registry() -> PluginRegistry {
        PluginRegistry::new(ProtocolVersion::CURRENT)
    }

    #[tokio::test]
    async fn register_puts_the_declared_capabilities_on_the_router() {
        let router = CapabilityRouter::new();
        let registry = registry();
        let (echo, _) = hosted_double("acme.echo", vec![declaration("acme.echo.run")]);
        let mut hosted = HostedPlugins::from_hosts(vec![echo]);

        hosted.register(&router, &registry).await;

        assert!(router.describe("acme.echo.run").is_some());
        assert_eq!(
            registry.state(&PluginId::from("acme.echo")),
            Some(PluginState::Running)
        );
        assert_eq!(hosted.notice_lines().count(), 0);
    }

    #[tokio::test]
    async fn a_duplicate_capability_fails_only_the_later_plugin() {
        let router = CapabilityRouter::new();
        let registry = registry();
        let (first, first_stopped) = hosted_double("acme.first", vec![declaration("acme.shared")]);
        let (second, second_stopped) =
            hosted_double("acme.second", vec![declaration("acme.shared")]);
        let mut hosted = HostedPlugins::from_hosts(vec![first, second]);

        hosted.register(&router, &registry).await;

        // The first was kept, the second reported and skipped, and the router
        // still carries exactly the first host's capability.
        assert!(router.describe("acme.shared").is_some());
        assert_eq!(
            registry.state(&PluginId::from("acme.first")),
            Some(PluginState::Running)
        );
        assert!(registry.state(&PluginId::from("acme.second")).is_none());
        assert!(!first_stopped.load(Ordering::SeqCst));
        assert!(
            second_stopped.load(Ordering::SeqCst),
            "a plugin that could not register is stopped, not left running"
        );
        assert_eq!(hosted.notice_lines().count(), 1);
    }

    #[tokio::test]
    async fn teardown_stops_every_started_host() {
        let router = CapabilityRouter::new();
        let registry = registry();
        let (one, one_stopped) = hosted_double("acme.one", vec![declaration("acme.one.run")]);
        let (two, two_stopped) = hosted_double("acme.two", vec![declaration("acme.two.run")]);
        let mut hosted = HostedPlugins::from_hosts(vec![one, two]);
        hosted.register(&router, &registry).await;

        hosted.shutdown().await;

        assert!(one_stopped.load(Ordering::SeqCst));
        assert!(two_stopped.load(Ordering::SeqCst));
        assert_eq!(
            registry.state(&PluginId::from("acme.one")),
            Some(PluginState::Stopped)
        );
        assert_eq!(
            registry.state(&PluginId::from("acme.two")),
            Some(PluginState::Stopped)
        );
    }

    // ── the serving-router pin ──────────────────────────────────────────

    /// Permits every capability, standing in for the session policy this wiring
    /// test does not configure.
    struct AllowAll;

    #[async_trait]
    impl PermissionService for AllowAll {
        async fn check(
            &self,
            _declaration: &CapabilityDeclaration,
            _context: &InvocationContext,
            _input: &Value,
        ) -> Result<(), KernelError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_registered_plugin_capability_is_invocable_through_the_serving_router() {
        let context = RuntimeContext::with_services(
            CapabilityRouter::new(),
            RuntimeServices::with_permission(Arc::new(AllowAll)),
        );
        let (echo, _) = hosted_double("acme.echo", vec![declaration("acme.echo.run")]);
        let mut hosted = HostedPlugins::from_hosts(vec![echo]);
        hosted.register(context.router(), context.plugins()).await;

        let output = tokio::time::timeout(
            Duration::from_secs(5),
            context.router().invoke(
                "acme.echo.run",
                context.invocation(
                    RequestId::from("acme-request"),
                    PluginId::from("acme.caller"),
                    "test",
                ),
                json!({ "hello": "world" }),
            ),
        )
        .await
        .expect("invocation is bounded")
        .expect("the hosted capability answers");

        assert_eq!(output["capability"], json!("acme.echo.run"));
        assert_eq!(output["echo"], json!({ "hello": "world" }));
    }
}
