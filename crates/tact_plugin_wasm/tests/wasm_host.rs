use std::{path::PathBuf, sync::Arc, time::Duration};

use serde_json::json;
use tact::{
    CapabilityRegistration, CapabilityRouter, FnCapabilityHandler, InvocationContext, KernelError,
    PermissionService, PluginState, RuntimeContext, RuntimeServices, StorageService,
};
use tact_plugin_wasm::{WasmHostConfig, WasmHostFunctions, WasmPluginHost, WasmPluginManifest};
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, ErrorCategory, PluginId,
    ProtocolVersion, RequestId, RunId, RuntimeEvent, TrajectoryId,
};
use tokio::sync::Mutex;

fn runner_shim() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm-runner-shim.mjs")
}

fn write_runner_script(path: &std::path::Path, script: &str) {
    std::fs::write(path, format!("#!/usr/bin/env node\n{script}")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }
}

/// Serializes writing a runner script with the plugin spawn that execs it.
///
/// On Linux `execve` rejects a script with `ETXTBSY` ("Text file busy") while
/// any process still holds that file open for writing. These tests run in
/// parallel, and one thread's `Command::spawn` fork briefly inherits another
/// thread's still-open write descriptor to its freshly created runner script,
/// so the write and the spawn that consumes it must not overlap. Holding this
/// lock across both removes the window (and the intermittent failure) without
/// touching any assertion.
static SPAWN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Writes `script` to `runner` and instantiates the host from it while holding
/// [`SPAWN_LOCK`], so the write cannot race another test's spawn fork.
async fn write_runner_and_instantiate(
    runner: &std::path::Path,
    script: &str,
    module: impl AsRef<std::path::Path>,
    manifest: WasmPluginManifest,
    config: WasmHostConfig,
) -> anyhow::Result<WasmPluginHost> {
    let _guard = SPAWN_LOCK.lock().await;
    write_runner_script(runner, script);
    WasmPluginHost::instantiate(runner, module, manifest, config).await
}

fn declaration(name: &str) -> CapabilityDeclaration {
    CapabilityDeclaration {
        name: name.into(),
        kind: CapabilityKind::Tool,
        version: "1".into(),
        description: None,
        input_schema: Some(json!({ "type": "object" })),
        output_schema: None,
        risk: CapabilityRisk::ReadOnly,
    }
}

fn manifest() -> WasmPluginManifest {
    WasmPluginManifest {
        plugin_id: PluginId::from("fixture.wasm"),
        protocol: ProtocolVersion::CURRENT,
        capabilities: vec![
            declaration("wasm.echo"),
            declaration("wasm.slow"),
            declaration("wasm.self"),
        ],
        host_capabilities: vec!["clock.read".into()],
    }
}

fn allow_runtime(router: CapabilityRouter) -> RuntimeContext {
    RuntimeContext::with_services(
        router,
        RuntimeServices::with_permission(Arc::new(AllowPermissions)),
    )
}

async fn instantiate(timeout: Duration) -> (WasmPluginHost, tempfile::TempDir) {
    instantiate_with(manifest(), timeout).await
}

async fn instantiate_with(
    manifest: WasmPluginManifest,
    timeout: Duration,
) -> (WasmPluginHost, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("chat.wasm");
    std::fs::write(&module, b"\0asm\x01\0\0\0").unwrap();
    let runner = dir.path().join("wasmtime-shim");
    let host = write_runner_and_instantiate(
        &runner,
        &std::fs::read_to_string(runner_shim()).unwrap(),
        &module,
        manifest,
        WasmHostConfig {
            memory_limit_bytes: 32 * 1024 * 1024,
            fuel: 100_000,
            timeout,
        },
    )
    .await
    .unwrap();
    (host, dir)
}

#[cfg(unix)]
#[tokio::test]
async fn instantiates_with_limits_and_routes_declared_calls() {
    let (host, _module_dir) = instantiate(Duration::from_secs(2)).await;
    let router = CapabilityRouter::new();
    host.register_with_router(&router).unwrap();
    let runtime = allow_runtime(router);
    let context = runtime
        .invocation(
            RequestId::from("wasm-echo"),
            PluginId::from("fixture.wasm"),
            "test",
        )
        .with_run_id(RunId::from("run-wasm"))
        .with_trajectory_id(TrajectoryId::from("trajectory-wasm"));
    let output = host
        .invoke(&runtime, "wasm.echo", context, json!({ "value": "ok" }))
        .await
        .unwrap();
    assert_eq!(output["value"], "ok");
    assert!(output["clock"].is_string());
    assert_eq!(output["denied"], false);
    assert_eq!(output["run_id"], "run-wasm");
    let args = output["runner_args"].as_array().unwrap();
    let args = args
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(args.contains(&"fuel=100000,max-memory-size=33554432,timeout=2000ms"));
    assert!(args.contains(
        &"inherit-env=false,inherit-stdin=true,inherit-stdout=true,inherit-stderr=true,inherit-network=false,allow-ip-name-lookup=false,tcp=false,udp=false"
    ));
    assert!(!args.contains(&"--dir"));
    host.drop_instance().await.unwrap();
    assert_eq!(host.state(), PluginState::Stopped);
}

#[cfg(unix)]
#[tokio::test]
async fn guest_host_calls_are_denied_when_not_granted() {
    let mut manifest = manifest();
    manifest.host_capabilities.clear();
    let (host, _module_dir) = instantiate_with(manifest, Duration::from_secs(2)).await;
    let router = CapabilityRouter::new();
    host.register_with_router(&router).unwrap();
    let runtime = allow_runtime(router);
    let context = runtime.invocation(
        RequestId::from("wasm-no-clock"),
        PluginId::from("fixture.wasm"),
        "test",
    );
    let output = host
        .invoke(&runtime, "wasm.echo", context, json!({}))
        .await
        .unwrap();
    assert_eq!(output["denied"], true);
    host.drop_instance().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn guest_cannot_recursively_invoke_its_own_capability() {
    let mut manifest = manifest();
    manifest.host_capabilities = vec!["capability:wasm.self".into()];
    let (host, _module_dir) = instantiate_with(manifest, Duration::from_secs(2)).await;
    let router = CapabilityRouter::new();
    host.register_with_router(&router).unwrap();
    let runtime = allow_runtime(router);
    let context = runtime
        .invocation(
            RequestId::from("wasm-self-call"),
            PluginId::from("fixture.wasm"),
            "test",
        )
        .with_timeout(Duration::from_millis(300));
    let output = host
        .invoke(&runtime, "wasm.self", context, json!({}))
        .await
        .unwrap();
    assert_eq!(output["denied"], true);
    assert_eq!(host.state(), PluginState::Running);
    host.drop_instance().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn manifest_is_validated_before_starting_runner() {
    let manifest = WasmPluginManifest {
        protocol: ProtocolVersion::new(9, 0),
        ..manifest()
    };
    let tempdir = tempfile::tempdir().unwrap();
    let marker = tempdir.path().join("runner-started");
    let runner = tempdir.path().join("runner");
    let module = tempdir.path().join("missing.wasm");
    let result = write_runner_and_instantiate(
        &runner,
        &format!(
            "import fs from 'node:fs'; fs.writeFileSync('{}', 'started');",
            marker.display()
        ),
        module,
        manifest,
        WasmHostConfig::default(),
    )
    .await;
    assert!(result.is_err());
    assert!(
        !marker.exists(),
        "invalid manifest must fail before runner startup"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn zero_resource_limits_fail_before_runner_startup() {
    let tempdir = tempfile::tempdir().unwrap();
    let marker = tempdir.path().join("runner-started");
    let runner = tempdir.path().join("runner");
    let module = tempdir.path().join("guest.wasm");
    std::fs::write(&module, b"\0asm\x01\0\0\0").unwrap();
    let result = write_runner_and_instantiate(
        &runner,
        &format!(
            "import fs from 'node:fs'; fs.writeFileSync('{}', 'started');",
            marker.display()
        ),
        module,
        manifest(),
        WasmHostConfig {
            memory_limit_bytes: 0,
            fuel: 1,
            timeout: Duration::from_secs(1),
        },
    )
    .await;
    assert!(result.is_err());
    assert!(
        !marker.exists(),
        "invalid resource limits must fail before startup"
    );
}

#[test]
fn manifest_cannot_grant_filesystem_network_or_process_access() {
    for denied in ["filesystem.read", "network.request", "process.spawn"] {
        let mut manifest = manifest();
        manifest.host_capabilities = vec![denied.into()];
        assert!(manifest.validate().is_err(), "unexpected grant: {denied}");
    }
}

#[test]
fn manifest_rejects_plugin_ids_that_escape_service_namespaces() {
    let manifest = WasmPluginManifest {
        plugin_id: PluginId::from("unsafe/plugin"),
        ..manifest()
    };
    assert!(manifest.validate().is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn required_host_calls_must_be_negotiated_before_registration() {
    let tempdir = tempfile::tempdir().unwrap();
    let runner = tempdir.path().join("wasmtime-legacy-shim");
    let legacy_shim = std::fs::read_to_string(runner_shim())
        .unwrap()
        .replace(", 'host_calls'", "");
    let module = tempdir.path().join("guest.wasm");
    std::fs::write(&module, b"\0asm\x01\0\0\0").unwrap();

    let result = write_runner_and_instantiate(
        &runner,
        &legacy_shim,
        &module,
        manifest(),
        WasmHostConfig::default(),
    )
    .await;
    assert!(
        result.is_err(),
        "missing host_calls feature must fail startup"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_and_interrupt_stop_the_guest_instance() {
    // The host-wide timeout also bounds startup (spawning the Node runner and
    // completing the handshake), so it must not share the short bound this test
    // exercises: on a loaded machine the spawn alone can outlast a few tens of
    // milliseconds and fail the test before the guest is even reached. Keep the
    // host generous and narrow only the invocation under test below.
    let (host, _module_dir) = instantiate(Duration::from_secs(5)).await;
    let host = Arc::new(host);
    let router = CapabilityRouter::new();
    host.register_with_router(&router).unwrap();
    let runtime = allow_runtime(router);
    let request_id = RequestId::from("wasm-cancel");
    // The short timeout that used to be the host-wide setting now applies to
    // exactly the request under test, so the timeout/interrupt semantics are
    // unchanged while startup stays untimed-bound by it.
    let context = runtime
        .invocation(request_id.clone(), PluginId::from("fixture.wasm"), "test")
        .with_timeout(Duration::from_millis(100));
    let call_runtime = runtime.clone();
    let call_host = Arc::clone(&host);
    let call = tokio::spawn(async move {
        call_host
            .invoke(&call_runtime, "wasm.slow", context, json!({}))
            .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(host.interrupt(runtime.cancellation(), &request_id));
    let error = call.await.unwrap().unwrap_err();
    assert_eq!(error.category(), ErrorCategory::Cancelled);
    assert_eq!(host.state(), PluginState::Failed);
    host.drop_instance().await.unwrap();
}

#[tokio::test]
async fn undeclared_storage_and_forged_host_events_are_rejected() {
    let functions = WasmHostFunctions::new(PluginId::from("fixture.wasm"), vec![]).unwrap();
    let context = RuntimeContext::new(CapabilityRouter::new()).invocation(
        RequestId::from("host-call"),
        PluginId::from("fixture.wasm"),
        "test",
    );
    let error = functions.get_storage(&context, "key").await.unwrap_err();
    assert_eq!(error.category(), ErrorCategory::PermissionDenied);

    let functions = WasmHostFunctions::new(
        PluginId::from("fixture.wasm"),
        vec![
            "events.publish".into(),
            "trajectory.append_plugin_event".into(),
        ],
    )
    .unwrap();
    let forged = RuntimeEvent::ToolCallFinished {
        parent_step_id: None,
        run_id: RunId::from("run-1"),
        step_id: tact_protocol::StepId::from("step-1"),
        success: true,
    };
    let error = functions.publish_event(&context, forged).await.unwrap_err();
    assert_eq!(error.category(), ErrorCategory::InvalidRequest);

    let storage = Arc::new(NamespacedStorage::default());
    let services = RuntimeServices::new(
        Arc::new(NoopEvents),
        Arc::new(NoopTrajectory),
        Arc::new(DenyPermissions),
        storage.clone(),
    );
    let context = RuntimeContext::with_services(CapabilityRouter::new(), services).invocation(
        RequestId::from("denied-storage"),
        PluginId::from("fixture.wasm"),
        "test",
    );
    let functions =
        WasmHostFunctions::new(PluginId::from("fixture.wasm"), vec!["storage.get".into()]).unwrap();
    let error = functions
        .get_storage(&context, "private")
        .await
        .unwrap_err();
    assert_eq!(error.category(), ErrorCategory::PermissionDenied);
    assert!(storage.namespaces.lock().await.is_empty());
}

#[tokio::test]
async fn storage_host_function_stays_in_plugin_namespace_and_clock_is_gated() {
    let storage = Arc::new(NamespacedStorage::default());
    let events = Arc::new(RecordedEvents::default());
    let trajectory = Arc::new(RecordedTrajectory::default());
    let services = RuntimeServices::new(
        events.clone(),
        trajectory.clone(),
        Arc::new(AllowPermissions),
        storage.clone(),
    );
    let context = RuntimeContext::with_services(CapabilityRouter::new(), services).invocation(
        RequestId::from("storage-call"),
        PluginId::from("fixture.wasm"),
        "test",
    );
    let functions = WasmHostFunctions::new(
        PluginId::from("fixture.wasm"),
        vec![
            "storage.set".into(),
            "storage.get".into(),
            "clock.read".into(),
            "events.publish".into(),
            "trajectory.append_plugin_event".into(),
        ],
    )
    .unwrap();
    functions
        .set_storage(&context, "k", json!("v"))
        .await
        .unwrap();
    assert_eq!(
        functions.get_storage(&context, "k").await.unwrap(),
        Some(json!("v"))
    );
    assert!(functions.clock_now(&context).await.unwrap().is_string());
    functions
        .publish_event(
            &context,
            RuntimeEvent::Plugin {
                plugin_id: PluginId::from("fixture.wasm"),
                origin: "plugin".into(),
                event_type: "plugin.fixture.wasm.progress".into(),
                payload: json!({ "percent": 50 }),
            },
        )
        .await
        .unwrap();
    assert_eq!(events.0.lock().await.len(), 1);
    assert_eq!(trajectory.0.lock().await.len(), 1);
    assert_eq!(
        storage.namespaces.lock().await.as_slice(),
        &["plugins/fixture.wasm", "plugins/fixture.wasm"]
    );
}

#[tokio::test]
async fn declared_external_host_capability_uses_kernel_router() {
    let router = CapabilityRouter::new();
    router
        .register(CapabilityRegistration::new(
            declaration("external.echo"),
            Arc::new(FnCapabilityHandler::new(
                |_context: InvocationContext, input| async move { Ok(input) },
            )),
        ))
        .unwrap();
    let runtime = allow_runtime(router.clone());
    let context = runtime.invocation(
        RequestId::from("external-host-call"),
        PluginId::from("fixture.wasm"),
        "test",
    );
    let functions = WasmHostFunctions::new(
        PluginId::from("fixture.wasm"),
        vec!["capability:external.echo".into()],
    )
    .unwrap();
    let output = functions
        .handle_call(
            &context,
            &router,
            "capability:external.echo",
            json!({ "value": "routed" }),
        )
        .await
        .unwrap();
    assert_eq!(output["value"], "routed");
}

struct AllowPermissions;

#[async_trait::async_trait]
impl PermissionService for AllowPermissions {
    async fn check(
        &self,
        _declaration: &CapabilityDeclaration,
        _context: &InvocationContext,
        _input: &serde_json::Value,
    ) -> Result<(), KernelError> {
        Ok(())
    }
}

struct DenyPermissions;

#[async_trait::async_trait]
impl PermissionService for DenyPermissions {
    async fn check(
        &self,
        _declaration: &CapabilityDeclaration,
        _context: &InvocationContext,
        _input: &serde_json::Value,
    ) -> Result<(), KernelError> {
        Err(KernelError::permission_denied("denied for test"))
    }
}

#[derive(Default)]
struct RecordedEvents(tokio::sync::Mutex<Vec<RuntimeEvent>>);

#[async_trait::async_trait]
impl tact::EventService for RecordedEvents {
    async fn publish(&self, event: RuntimeEvent) -> Result<(), KernelError> {
        self.0.lock().await.push(event);
        Ok(())
    }
}

#[derive(Default)]
struct RecordedTrajectory(tokio::sync::Mutex<Vec<RuntimeEvent>>);

#[async_trait::async_trait]
impl tact::TrajectoryService for RecordedTrajectory {
    async fn append(
        &self,
        _trajectory_id: Option<&TrajectoryId>,
        _run_id: Option<&RunId>,
        event: RuntimeEvent,
    ) -> Result<(), KernelError> {
        self.0.lock().await.push(event);
        Ok(())
    }
}

#[derive(Default)]
struct NamespacedStorage {
    data: Mutex<std::collections::HashMap<(String, String), serde_json::Value>>,
    namespaces: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl StorageService for NamespacedStorage {
    async fn get(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Option<serde_json::Value>, KernelError> {
        self.namespaces.lock().await.push(namespace.to_owned());
        Ok(self
            .data
            .lock()
            .await
            .get(&(namespace.into(), key.into()))
            .cloned())
    }

    async fn set(
        &self,
        namespace: &str,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), KernelError> {
        self.namespaces.lock().await.push(namespace.to_owned());
        self.data
            .lock()
            .await
            .insert((namespace.into(), key.into()), value);
        Ok(())
    }
}

struct NoopEvents;

#[async_trait::async_trait]
impl tact::EventService for NoopEvents {
    async fn publish(&self, _event: RuntimeEvent) -> Result<(), KernelError> {
        Ok(())
    }
}

struct NoopTrajectory;

#[async_trait::async_trait]
impl tact::TrajectoryService for NoopTrajectory {
    async fn append(
        &self,
        _trajectory_id: Option<&TrajectoryId>,
        _run_id: Option<&RunId>,
        _event: RuntimeEvent,
    ) -> Result<(), KernelError> {
        Ok(())
    }
}
