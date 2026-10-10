//! Re-export of the protocol-level trajectory fact type.
//!
//! The durable fact is a wire/domain type (`tact_protocol::trajectory`), so
//! the Kernel boundary and the recorder share one shape without this crate
//! leaking Rust internals.

pub use tact_protocol::trajectory::*;
