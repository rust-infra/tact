//! Real `Component` implementations (step 9 of the plan).
//!
//! Each component owns its state, feeds shared surfaces through `Ctx`, and
//! renders into a plain ratatui `Buffer`. The shell (host) routes updates /
//! keys / renders in `priority()` order.
//!
//! `ThinkingComponent` is the first real implementation (replaces the
//! compile-only trait draft with a working pattern); more components follow
//! as the app layer migrates.

pub mod plan;
pub mod registry;
pub mod status_bar;
pub mod stream;
pub mod task_panel;
pub mod thinking;
pub mod tool;

pub use plan::PlanComponent;
pub use registry::ComponentRegistry;
pub use status_bar::StatusBarComponent;
pub use stream::StreamComponent;
pub use task_panel::TaskPanelComponent;
pub use thinking::ThinkingComponent;
pub use tool::ToolComponent;
