//! Security guards, re-exported from the Runtime Kernel.
//!
//! The sensitive-path registry and the security configuration are the policy
//! the permission ladder consults, so they live in the Kernel beside the
//! permission decision. This module keeps the historical `crate::security`
//! paths working inside the extension crate.

pub use tact::security::*;

/// The redaction engine, under its historical module path.
pub mod redact {
    pub use tact::redact::*;
}
