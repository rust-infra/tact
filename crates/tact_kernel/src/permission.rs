//! Permission risk levels.
//!
//! The kernel owns the closed vocabulary; the policy that classifies a concrete
//! input lives behind `PermissionService` in the extensions crate.

use strum::Display;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Display)]
#[strum(serialize_all = "snake_case")]
pub enum CapabilityRisk {
    Read,
    Write,
    High,
}
