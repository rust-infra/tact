use tact_kernel::capability::{CapabilityFilter, CapabilityId, CapabilityKind};

#[test]
fn capability_id_round_trips() {
    let id: CapabilityId = "tool:bash".parse().unwrap();
    assert_eq!(id.kind, CapabilityKind::Tool);
    assert_eq!(id.name, "bash");
    assert_eq!(id.to_string(), "tool:bash");
}

#[test]
fn capability_id_rejects_unknown_kind_and_empty_name() {
    assert!("unknown:x".parse::<CapabilityId>().is_err());
    assert!("tool:".parse::<CapabilityId>().is_err());
    assert!("tool".parse::<CapabilityId>().is_err());
}

#[test]
fn filter_allows_unlisted_and_rejects_listed() {
    let disabled: CapabilityId = "tool:bash".parse().unwrap();
    let filter = CapabilityFilter::new([disabled.clone()]);
    assert!(filter.allows(&"tool:read_file".parse().unwrap()));
    assert!(!filter.allows(&disabled));
    assert_eq!(filter.disabled().collect::<Vec<_>>(), vec![&disabled]);
}
