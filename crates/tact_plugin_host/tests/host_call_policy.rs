//! What the granted host-call policy lets a plugin reach, and what it refuses.
//!
//! The policy is the only place a plugin's manifest grants meet the Kernel's
//! capabilities, so the interesting cases are the *refusals*: a name the
//! manifest did not declare, a `Tool` (which must keep its per-wave preflight
//! path) and a host control capability (which belongs to the host).

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tact::{
    EventService, FnCapabilityHandler, InvocationContext, KernelError, PermissionService,
    RuntimeContext, RuntimeServices, StorageService, TrajectoryService,
};
use tact_plugin_host::{GrantedHostCalls, HostCallService};
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, PluginId, RequestId, RunId,
    RuntimeEvent, TrajectoryId,
};
use tokio::time::timeout;

/// Every await in this file is bounded: a policy bug that hangs would otherwise
/// hang the whole suite.
const BOUND: Duration = Duration::from_secs(5);

/// Permits everything, standing in for a session policy that has nothing to say
/// about a host call.
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

/// Counts the checks it was asked for, so a test can assert that a refusal
/// happened *before* the ordinary permission path was consulted.
#[derive(Default)]
struct CountingPermission(AtomicUsize);

#[async_trait]
impl PermissionService for CountingPermission {
    async fn check(
        &self,
        _declaration: &CapabilityDeclaration,
        _context: &InvocationContext,
        _input: &Value,
    ) -> Result<(), KernelError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// The host control plane's rule, in the one line this crate needs to pin: a
/// plugin identity is not a host identity, so a control capability is refused.
///
/// The production policy is `crates/tact_ui/src/permission.rs::
/// HostControlPlanePermission`, which lives above `tact_extensions` and so
/// cannot be depended on from here; its own tests pin the allow side (both host
/// identities pass) and this one pins the refusal a plugin gets.
struct HostControlPlaneOnly;

#[async_trait]
impl PermissionService for HostControlPlaneOnly {
    async fn check(
        &self,
        declaration: &CapabilityDeclaration,
        context: &InvocationContext,
        _input: &Value,
    ) -> Result<(), KernelError> {
        if context.plugin_id().as_str() == "tact.chat" {
            return Ok(());
        }
        Err(KernelError::permission_denied(format!(
            "{} is a host control capability; {} may not invoke it",
            declaration.name,
            context.plugin_id()
        )))
    }
}

/// Records the namespaces it was read at, and answers one key.
#[derive(Default)]
struct RecordingStorage {
    namespaces: Mutex<Vec<String>>,
}

#[async_trait]
impl StorageService for RecordingStorage {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Value>, KernelError> {
        self.namespaces.lock().unwrap().push(namespace.to_string());
        Ok((key == "greeting").then(|| json!("hello from the plugin namespace")))
    }

    async fn set(&self, _namespace: &str, _key: &str, _value: Value) -> Result<(), KernelError> {
        Ok(())
    }
}

/// Counts the facts it was asked to publish.
#[derive(Default)]
struct CountingEvents(AtomicUsize);

#[async_trait]
impl EventService for CountingEvents {
    async fn publish(&self, _event: RuntimeEvent) -> Result<(), KernelError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Keeps nothing: these tests assert on the event service and the storage
/// backend, and `storage.get` needs no trajectory.
struct NoopTrajectory;

#[async_trait]
impl TrajectoryService for NoopTrajectory {
    async fn append(
        &self,
        _trajectory_id: Option<&TrajectoryId>,
        _run_id: Option<&RunId>,
        _event: RuntimeEvent,
    ) -> Result<(), KernelError> {
        Ok(())
    }
}

/// A serving context carrying the Kernel's real service capabilities over the
/// given policy and storage.
fn serving(
    permission: Arc<dyn PermissionService>,
    storage: Arc<dyn StorageService>,
    events: Arc<dyn EventService>,
) -> RuntimeContext {
    let context = RuntimeContext::with_services(
        tact::CapabilityRouter::new(),
        RuntimeServices::new(events, Arc::new(NoopTrajectory), permission, storage),
    );
    tact::services::register(context.router()).expect("the Kernel services register");
    context
}

/// The invocation the outer capability call arrived in. The policy ignores it
/// on purpose (its own fresh invocation is what the service sees), but the
/// trait hands one over, so the tests pass a realistic one: a caller that is
/// not the plugin.
fn outer(serving: &RuntimeContext) -> InvocationContext {
    serving.invocation(
        RequestId::from("outer-request"),
        PluginId::from("tact.agent"),
        "agent",
    )
}

/// A granted `Service` capability is invoked, and the service sees the
/// *plugin's* identity, not the caller's.
///
/// `storage.*` is the sharp case: its handler authorizes the caller's
/// `plugins/<id>` namespace, so a policy that let the outer caller's identity
/// through would be refused here — the read only succeeds as the plugin.
#[tokio::test]
async fn a_granted_service_is_invoked_as_the_plugin() {
    let storage = Arc::new(RecordingStorage::default());
    let context = serving(
        Arc::new(AllowAll),
        Arc::clone(&storage) as Arc<dyn StorageService>,
        Arc::new(CountingEvents::default()),
    );
    let policy = GrantedHostCalls::new(
        context.clone(),
        PluginId::from("acme.echo"),
        ["storage.get".to_string()],
    );

    let output = timeout(
        BOUND,
        policy.handle(
            &outer(&context),
            context.router(),
            "storage.get",
            json!({ "namespace": "plugins/acme.echo", "key": "greeting" }),
        ),
    )
    .await
    .expect("the host call is bounded")
    .expect("a granted Kernel service answers");

    assert_eq!(output, json!("hello from the plugin namespace"));
    assert_eq!(
        storage.namespaces.lock().unwrap().as_slice(),
        ["plugins/acme.echo"],
        "the read happened in the plugin's own namespace"
    );
}

/// The grant list is the manifest's own bound: a name it does not declare is
/// refused by name, and the capability is never entered.
#[tokio::test]
async fn an_ungranted_capability_is_denied() {
    let events = Arc::new(CountingEvents::default());
    let context = serving(
        Arc::new(AllowAll),
        Arc::new(RecordingStorage::default()),
        Arc::clone(&events) as Arc<dyn EventService>,
    );
    let policy = GrantedHostCalls::new(
        context.clone(),
        PluginId::from("acme.echo"),
        ["storage.get".to_string()],
    );

    let denied = timeout(
        BOUND,
        policy.handle(
            &outer(&context),
            context.router(),
            "events.publish",
            json!({ "event": { "type": "text" } }),
        ),
    )
    .await
    .expect("the host call is bounded")
    .expect_err("an ungranted capability is not a host call");

    assert_eq!(
        denied.category(),
        tact_protocol::ErrorCategory::PermissionDenied
    );
    assert!(
        denied.message().contains("events.publish"),
        "the refusal names the capability: {}",
        denied.message()
    );
    assert_eq!(
        events.0.load(Ordering::SeqCst),
        0,
        "the capability must not be entered for an ungranted name"
    );
}

/// A `Tool` is model-visible, and the per-wave preflight (sensitive-target
/// guard, one-use approval ticket) is the only path that may run it. A serving
/// host call must not become a way around it — not even for a Tool the plugin
/// declared itself and granted itself.
#[tokio::test]
async fn a_granted_tool_is_denied() {
    let context = serving(
        Arc::new(AllowAll),
        Arc::new(RecordingStorage::default()),
        Arc::new(CountingEvents::default()),
    );
    context
        .router()
        .register_handler(
            CapabilityDeclaration {
                name: "acme.echo.search".into(),
                kind: CapabilityKind::Tool,
                version: "1".into(),
                description: None,
                input_schema: None,
                output_schema: None,
                risk: CapabilityRisk::ReadOnly,
            },
            FnCapabilityHandler::new(|_, _| async { Ok(json!(null)) }),
        )
        .expect("the tool registers on this router");
    let policy = GrantedHostCalls::new(
        context.clone(),
        PluginId::from("acme.echo"),
        ["acme.echo.search".to_string()],
    );

    let denied = timeout(
        BOUND,
        policy.handle(
            &outer(&context),
            context.router(),
            "acme.echo.search",
            json!({ "query": "rust" }),
        ),
    )
    .await
    .expect("the host call is bounded")
    .expect_err("a Tool is not a host call");

    assert_eq!(
        denied.category(),
        tact_protocol::ErrorCategory::PermissionDenied
    );
    assert!(
        denied.message().contains("not a Kernel service"),
        "the refusal says why: {}",
        denied.message()
    );
}

/// A granted **`Tool`-kind** name and a granted **host control name** are both
/// refused, and the control capability is refused before the permission path.
///
/// `chat.submit` is declared `App` by the Chat extension, so the kind check is
/// what stops a plugin here: it never reaches `HostControlPlanePermission` at
/// all. The next test pins that second wall for the case the kind check cannot
/// answer (a `Service`-kind control capability).
#[tokio::test]
async fn a_granted_host_control_capability_is_denied_before_the_permission_path() {
    let permission = Arc::new(CountingPermission::default());
    let context = serving(
        Arc::clone(&permission) as Arc<dyn PermissionService>,
        Arc::new(RecordingStorage::default()),
        Arc::new(CountingEvents::default()),
    );
    // The declaration as the Chat extension ships it: kind `App`.
    context
        .router()
        .register_handler(
            CapabilityDeclaration {
                name: "chat.submit".into(),
                kind: CapabilityKind::App,
                version: "1".into(),
                description: None,
                input_schema: None,
                output_schema: None,
                risk: CapabilityRisk::Medium,
            },
            FnCapabilityHandler::new(|_, _| async { Ok(json!(null)) }),
        )
        .expect("chat.submit registers on this router");
    let policy = GrantedHostCalls::new(
        context.clone(),
        PluginId::from("acme.echo"),
        ["chat.submit".to_string(), "runs.start".to_string()],
    );

    for capability in ["chat.submit", "runs.start"] {
        let inputs = json!({ "prompt": "inject a turn" });
        let denied = timeout(
            BOUND,
            policy.handle(&outer(&context), context.router(), capability, inputs),
        )
        .await
        .expect("the host call is bounded")
        .expect_err("a host control capability is not a host call");
        assert_eq!(
            denied.category(),
            tact_protocol::ErrorCategory::PermissionDenied,
            "{capability} must be refused"
        );
    }
    assert_eq!(
        permission.0.load(Ordering::SeqCst),
        0,
        "the control capability is refused by the kind check, before any policy"
    );
}

/// The second wall, for a control name declared as a `Service`: even *then* the
/// plugin invokes as itself, and the host control plane refuses a caller that is
/// not one of the host's own identities.
#[tokio::test]
async fn a_plugin_identity_is_not_a_host_identity() {
    let context = serving(
        Arc::new(HostControlPlaneOnly),
        Arc::new(RecordingStorage::default()),
        Arc::new(CountingEvents::default()),
    );
    context
        .router()
        .register_handler(
            CapabilityDeclaration {
                name: "chat.submit".into(),
                kind: CapabilityKind::Service,
                version: "1".into(),
                description: None,
                input_schema: None,
                output_schema: None,
                risk: CapabilityRisk::Medium,
            },
            FnCapabilityHandler::new(|_, _| async { Ok(json!(null)) }),
        )
        .expect("chat.submit registers on this router");
    let policy = GrantedHostCalls::new(
        context.clone(),
        PluginId::from("acme.echo"),
        ["chat.submit".to_string()],
    );

    let denied = timeout(
        BOUND,
        policy.handle(
            &outer(&context),
            context.router(),
            "chat.submit",
            json!({ "prompt": "inject a turn" }),
        ),
    )
    .await
    .expect("the host call is bounded")
    .expect_err("a plugin identity cannot invoke a host control capability");

    assert_eq!(
        denied.category(),
        tact_protocol::ErrorCategory::PermissionDenied
    );
    assert!(
        denied.message().contains("acme.echo"),
        "the refusal names the caller that was refused: {}",
        denied.message()
    );
}

/// A name that is not on the serving router at all is a denial, not an error
/// the plugin can retry into: the policy never invents a capability.
#[tokio::test]
async fn a_granted_name_the_router_does_not_carry_is_denied() {
    let context = serving(
        Arc::new(AllowAll),
        Arc::new(RecordingStorage::default()),
        Arc::new(CountingEvents::default()),
    );
    let policy = GrantedHostCalls::new(
        context.clone(),
        PluginId::from("acme.echo"),
        ["storage.gone".to_string()],
    );

    let denied = timeout(
        BOUND,
        policy.handle(
            &outer(&context),
            context.router(),
            "storage.gone",
            json!({}),
        ),
    )
    .await
    .expect("the host call is bounded")
    .expect_err("an unknown capability is not a host call");

    assert_eq!(
        denied.category(),
        tact_protocol::ErrorCategory::PermissionDenied
    );
}
