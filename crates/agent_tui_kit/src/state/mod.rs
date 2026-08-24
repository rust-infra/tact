//! Component state types shared by the kit (thinking, tool, stream, …).
//!
//! Phase 3 moves these out of `crates/tui/src/widgets/state` in cluster order.
//! Each type is pure state + methods — no `App` dependency.

pub mod account;
pub mod log;
pub mod log_scroll;
pub mod mouse_state;
pub mod plan_panel;
pub mod select_popup;
pub mod selection;
pub mod status_bar_state;
pub mod stream_parser;
pub mod stream_state;
pub mod task_panel;
pub mod thinking;
pub mod tool_state;
pub mod ui_types;

pub use account::AccountState;
pub use log::{LogCoordinator, LogItem, LogItemKind, SystemMsgStyle, log_indent_at};
pub use log_scroll::LogScroll;
pub use mouse_state::{LogSelection, MouseState, PopupHitRow, PopupTextHit, TextPosition};
pub use plan_panel::PlanPanel;
pub use select_popup::SelectPopup;
pub use selection::PopupTextSelection;
pub use status_bar_state::StatusBarState;
pub use stream_parser::StreamEvent;
pub use stream_state::StreamState;
pub use task_panel::TaskPanelState;
pub use thinking::{
    ActiveThinkingBlock, ThinkingBlock, ThinkingPopup, ThinkingState, find_thinking_at_logical,
};
pub use tool_state::{
    ActiveToolBlock, DiffPopup, RowRole, SubagentLabels, SubagentLayoutCache, SubagentPopup,
    SubagentRow, SubagentSourceLine, ToolBlock, ToolState,
};
pub use ui_types::{
    CodeBlock, CodePopup, FocusedPanel, HistoryEntry, InputMode, MermaidBlock, MermaidPopup,
    SkillEntry, Status, SystemPromptPopup,
};
