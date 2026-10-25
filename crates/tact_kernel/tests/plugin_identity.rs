use tact_kernel::plugin::{PluginId, PluginKind, PluginManifest};

#[test]
fn plugin_id_rejects_blank_or_control() {
    assert!(PluginId::new("").is_err());
    assert!(PluginId::new("a b").is_err());
    assert!(PluginId::new("a\nb").is_err());
    assert!(PluginId::new("a\tb").is_err());
}

#[test]
fn plugin_id_accepts_a_namespaced_label() {
    assert_eq!(PluginId::new("tool.bash").unwrap().as_str(), "tool.bash");
}

#[test]
fn manifest_exposes_identity_and_kind() {
    let id = PluginId::new("tool.bash").unwrap();
    let manifest = PluginManifest::new(
        id.clone(),
        "Bash",
        "1.0.0",
        PluginKind::Capability,
        "shell tool",
    );
    assert_eq!(manifest.id, id);
    assert_eq!(manifest.kind, PluginKind::Capability);
}
