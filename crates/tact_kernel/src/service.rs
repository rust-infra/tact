//! Typed services a plugin provides to its dependants.
//!
//! Deliberately **not** a service locator: a plugin records its handle here at
//! registration, the host exposes the registry after sealing, and the
//! composition root pulls the *specific* handle it needs and passes it to the
//! plugins constructed after it. A `get::<T>()` call inside business logic is
//! what this design avoids; the registry exists so the host does not have to
//! know every concrete type up front.
//!
//! The key is the Rust type, so two plugins cannot silently provide two
//! different things under one string. A duplicate is a registration error, not a
//! silent last-wins: the host is installing an explicit dependency order, and a
//! second provider means the order is wrong.

use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::plugin::PluginId;

/// One registered handle.
#[derive(Clone)]
struct Entry {
    plugin: PluginId,
    value: Arc<dyn Any + Send + Sync>,
}

/// The services installed so far.
#[derive(Default, Clone)]
pub struct ServiceRegistry {
    entries: BTreeMap<TypeId, Entry>,
    /// For diagnostics: `TypeId` is opaque, so the host names what it provides.
    names: BTreeMap<TypeId, &'static str>,
}

impl ServiceRegistry {
    /// Record `service`, naming the plugin that provides it.
    ///
    /// The name is for error messages only; `T` is the key.
    pub fn insert<T: Send + Sync + 'static>(
        &mut self,
        plugin: PluginId,
        service: Arc<T>,
    ) -> anyhow::Result<()> {
        let key = TypeId::of::<T>();
        if let Some(existing) = self.entries.get(&key) {
            anyhow::bail!(
                "{} is already provided by plugin {}",
                std::any::type_name::<T>(),
                existing.plugin
            );
        }
        self.entries.insert(
            key,
            Entry {
                plugin,
                value: service,
            },
        );
        self.names.insert(key, std::any::type_name::<T>());
        Ok(())
    }

    /// The handle for `T`, if a plugin provided one.
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.entries
            .get(&TypeId::of::<T>())
            .and_then(|entry| entry.value.clone().downcast::<T>().ok())
    }

    /// The plugin that provided `T`, for a diagnostic that has to name it.
    pub fn provider<T: Send + Sync + 'static>(&self) -> Option<&PluginId> {
        self.entries.get(&TypeId::of::<T>()).map(|e| &e.plugin)
    }

    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.entries.contains_key(&TypeId::of::<T>())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every provided type name, sorted, for a startup diagnostic.
    pub fn provided(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.names.values().copied().collect();
        names.sort_unstable();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct Greeter(&'static str);

    #[derive(Debug, PartialEq, Eq)]
    struct Clock(u64);

    fn plugin(id: &str) -> PluginId {
        PluginId::new(id).unwrap()
    }

    #[test]
    fn a_service_is_retrievable_by_its_type() {
        let mut registry = ServiceRegistry::default();
        registry
            .insert(plugin("greet"), Arc::new(Greeter("hello")))
            .unwrap();
        assert_eq!(
            registry.get::<Greeter>().map(|greeter| greeter.0),
            Some("hello")
        );
        assert!(registry.get::<Clock>().is_none());
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn two_services_of_different_types_coexist() {
        let mut registry = ServiceRegistry::default();
        registry
            .insert(plugin("greet"), Arc::new(Greeter("hi")))
            .unwrap();
        registry
            .insert(plugin("clock"), Arc::new(Clock(7)))
            .unwrap();
        assert_eq!(registry.get::<Clock>().map(|clock| clock.0), Some(7));
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn a_second_provider_is_an_error_naming_the_first() {
        // The host installs an explicit order; a second provider means that
        // order is wrong, so this must not be silent last-wins.
        let mut registry = ServiceRegistry::default();
        registry
            .insert(plugin("first"), Arc::new(Greeter("a")))
            .unwrap();
        let error = registry
            .insert(plugin("second"), Arc::new(Greeter("b")))
            .unwrap_err();
        let text = error.to_string();
        assert!(text.contains("already provided by plugin first"), "{text}");
        // The original survives a rejected insert.
        assert_eq!(registry.get::<Greeter>().map(|g| g.0), Some("a"));
        assert_eq!(
            registry.provider::<Greeter>().map(PluginId::as_str),
            Some("first")
        );
    }

    #[test]
    fn the_provided_list_names_every_type() {
        let mut registry = ServiceRegistry::default();
        registry
            .insert(plugin("greet"), Arc::new(Greeter("hi")))
            .unwrap();
        registry
            .insert(plugin("clock"), Arc::new(Clock(1)))
            .unwrap();
        let provided = registry.provided();
        assert_eq!(provided.len(), 2);
        assert!(provided.iter().any(|name| name.ends_with("Greeter")));
        assert!(provided.iter().any(|name| name.ends_with("Clock")));
    }

    #[test]
    fn a_clone_shares_the_handles() {
        let mut registry = ServiceRegistry::default();
        registry
            .insert(plugin("greet"), Arc::new(Greeter("shared")))
            .unwrap();
        let clone = registry.clone();
        assert_eq!(clone.get::<Greeter>().map(|g| g.0), Some("shared"));
        assert!(clone.contains::<Greeter>());
    }
}
