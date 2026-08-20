//! Component state types shared by the kit (thinking, tool, stream, …).
//!
//! Phase 3 moves these out of `crates/tui/src/widgets/state` in cluster order.
//! Each type is pure state + methods — no `App` dependency.

pub mod log;
pub mod selection;
pub mod thinking;

pub use log::{LogCoordinator, LogItem, LogItemKind, SystemMsgStyle};
pub use selection::PopupTextSelection;
pub use thinking::{ActiveThinkingBlock, ThinkingBlock, ThinkingPopup, ThinkingState};
