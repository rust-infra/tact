//! The headless view, as a plugin.
//!
//! Per the design's crate layout, `HeadlessPlugin` lives with the host-side view
//! adapter rather than in the kernel: the kernel knows a view exists, not what kind
//! it is. The plugin's contribution is the view capability, which is what lets a
//! composition root assert exactly one view is attached.

use tact_kernel::capability::view::View;
use tact_kernel::plugin::{Plugin, PluginId, PluginKind, PluginManifest, Registrar};

/// The non-interactive view: one prompt in, one answer out.
pub struct HeadlessView;

impl View for HeadlessView {
    fn id(&self) -> &'static str {
        "headless"
    }
}

/// Contributes the headless view.
pub struct HeadlessPlugin;

impl HeadlessPlugin {
    /// A plugin contributing the headless view.
    pub fn new() -> Self {
        Self
    }
}

impl Default for HeadlessPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Plugin for HeadlessPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            PluginId::new("builtin.view.headless").expect("a literal id"),
            "Headless view",
            env!("CARGO_PKG_VERSION"),
            PluginKind::View,
            "Non-interactive frontend for scripts and pipes",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> anyhow::Result<()> {
        registrar.view(HeadlessView)
    }
}

/// The one view attached to this process.
///
/// # Errors
///
/// Returns an error when no view is attached, or when more than one is. Both are
/// wiring mistakes: the first means nothing can be drawn, the second means the
/// choice of frontend was never made. Reporting them is the point — a process that
/// silently picked one of two would hide which frontend the user is looking at.
pub fn attached(catalog: &tact_kernel::catalog::ViewCatalog) -> anyhow::Result<&'static str> {
    match catalog.ids().as_slice() {
        [] => anyhow::bail!("no view attached; install TuiPlugin or HeadlessPlugin"),
        [only] => Ok(*only),
        many => anyhow::bail!(
            "{} views attached ({:?}); a process attaches exactly one",
            many.len(),
            many
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::PluginHost;
    use tui::plugin::TuiPlugin;

    /// The plugin contributes the view, and the host can name it.
    #[tokio::test]
    async fn the_plugin_contributes_the_view() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(HeadlessPlugin::new()).await.unwrap();
        let runtime = host.seal().await.unwrap();
        assert_eq!(runtime.views().ids(), vec!["headless"]);
    }

    /// A process attaches one view, and `attached` is what enforces it.
    ///
    /// The catalog deliberately allows both — it is a registry, and a duplicate
    /// *id* is the only thing it refuses. "One frontend per process" is the
    /// composition root's rule, so it is checked here rather than assumed.
    #[tokio::test]
    async fn two_attached_views_are_reported_rather_than_silently_resolved() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(TuiPlugin::new()).await.unwrap();
        host.install(HeadlessPlugin::new()).await.unwrap();
        let runtime = host.seal().await.unwrap();
        let error = match attached(runtime.views()) {
            Ok(id) => panic!("two views must not resolve to one silently: {id}"),
            Err(error) => error,
        };
        assert!(format!("{error}").contains("2"), "{error}");
    }

    /// One view resolves to its id.
    #[tokio::test]
    async fn one_attached_view_resolves_to_its_id() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(HeadlessPlugin::new()).await.unwrap();
        let runtime = host.seal().await.unwrap();
        assert_eq!(attached(runtime.views()).unwrap(), "headless");
    }

    /// The manifest says what it contributes.
    #[test]
    fn the_manifest_says_what_it_is() {
        let manifest = HeadlessPlugin::new().manifest();
        assert_eq!(manifest.id.as_str(), "builtin.view.headless");
        assert_eq!(manifest.kind, PluginKind::View);
    }
}
