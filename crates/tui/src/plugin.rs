//! The terminal view, as a plugin.
//!
//! Per the design's crate layout, `TuiPlugin` lives here — in the view crate, not
//! in the composition root — so a different host can attach this view without
//! naming any `tact` type. The plugin's contribution is the view capability: the
//! host can ask "which view is attached" and get an answer, and a second view
//! cannot be installed silently.

use tact_kernel::capability::view::View;
use tact_kernel::plugin::{Plugin, PluginId, PluginKind, PluginManifest, Registrar};

/// The terminal view.
pub struct TuiView;

impl View for TuiView {
    fn id(&self) -> &'static str {
        "tui"
    }
}

/// Contributes the terminal view.
pub struct TuiPlugin;

impl TuiPlugin {
    /// A plugin contributing the terminal view.
    pub fn new() -> Self {
        Self
    }
}

impl Default for TuiPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Plugin for TuiPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            PluginId::new("builtin.view.tui").expect("a literal id"),
            "Terminal view",
            env!("CARGO_PKG_VERSION"),
            PluginKind::View,
            "The interactive terminal frontend",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> anyhow::Result<()> {
        // The view itself is the contribution: the palette's view-local commands are
        // dispatched inside the `App`, which owns the state they mutate, so they
        // stay where the state is rather than becoming catalog entries that would
        // have to send effects back.
        registrar.view(TuiView)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::PluginHost;

    /// The plugin contributes the view, and the host can name it.
    #[tokio::test]
    async fn the_plugin_contributes_the_view() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(TuiPlugin::new()).await.unwrap();
        let runtime = host.seal().await.unwrap();
        assert_eq!(runtime.views().ids(), vec!["tui"]);
        assert!(runtime.views().get("tui").is_some());
    }

    /// A second view cannot be installed under the same id.
    ///
    /// Two views in one process is a wiring mistake: the same terminal cannot be
    /// drawn twice, and a silent override would hide which one is live.
    #[tokio::test]
    async fn a_duplicate_view_id_is_refused() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(TuiPlugin::new()).await.unwrap();
        let second = host.install(TuiPlugin::new()).await;
        assert!(second.is_err(), "a duplicate view id must be refused");
    }

    /// The manifest says what it contributes.
    #[test]
    fn the_manifest_says_what_it_is() {
        let manifest = TuiPlugin::new().manifest();
        assert_eq!(manifest.id.as_str(), "builtin.view.tui");
        assert_eq!(manifest.kind, PluginKind::View);
    }
}
