//! AI-native UI components for GPUI applications.
//!
//! Composed, opinionated components for the surfaces an AI application keeps
//! rebuilding — streaming text, thinking traces, tool calls, approval gates —
//! built on [`gpui_component`] and inheriting its semantic-token theming.
//! Every component resolves all of its presentation through the active theme,
//! so light/dark modes, bundled themes, and custom JSON themes work without
//! extra wiring.
//!
//! # Design rules
//!
//! - Stateless components are fluent [`gpui::RenderOnce`] builders; stateful
//!   composites are entities. Callbacks use `on_*` methods.
//! - Progressive output flows through one model: [`stream::StreamedContent`].
//!   Components render snapshots; applications own the state and the clock.
//! - No component runs a clock over the state it is given, or keeps fixture
//!   data. A component may time something it alone owns — a "Copied"
//!   confirmation clearing itself — but nothing a snapshot describes is ever
//!   advanced from inside.
//!
//! Further components land phase by phase; see the repository roadmap.

#![deny(missing_docs)]

pub mod approval;
pub mod artifact;
pub mod attachment;
mod button_label;
pub mod chat;
pub mod chip;
pub mod code_block;
pub mod code_diff;
pub mod command_search;
pub mod comparison_table;
pub mod context_card;
pub mod context_meter;
mod control;
pub mod cues;
pub mod decoration;
pub mod diff_table;
pub mod filter_table;
pub mod fine_tune;
pub mod form;
mod glide;
pub mod image_generation;
pub mod insight;
pub mod loading;
pub mod motion;
pub mod orbs;
pub mod plan;
pub mod popup;
pub mod prompt_bar;
pub mod question_flow;
pub mod queue;
pub mod recommendation;
pub mod records_table;
mod resolved_layout;
pub mod scrolling;
pub mod search_results;
pub mod selection_actions;
pub mod sidebar_nav;
pub mod sizing;
pub mod status;
pub mod stream;
pub mod streaming_text;
pub mod suggestions;
mod surface;
pub mod task;
mod theme;
pub mod thinking;
pub mod thread_list;
pub mod todo_list;
pub mod tool_call;
pub mod voice;

pub use button_label::ButtonLabelExt;

/// Convenient single-import surface: `use gpui_ai::prelude::*;`.
pub mod prelude {
    pub use crate::ButtonLabelExt;
    pub use crate::approval::{ApprovalCard, ApprovalDecision, ApprovalEvent, ApprovalTone};
    pub use crate::artifact::{
        Artifact, ArtifactAction, ArtifactKind, ArtifactPanel, ArtifactPanelEvent, ArtifactVersion,
        ArtifactView,
    };
    pub use crate::attachment::{
        Attachment, AttachmentEvent, AttachmentKind, AttachmentPreview, AttachmentStrip,
        format_bytes,
    };
    pub use crate::chat::{
        BranchPosition, Chat, ChatEvent, ChatMessage, ChatMessageAppearance, ChatRole, ChatWelcome,
        MessageActions, MessageAlignment, MessageBubble,
    };
    pub use crate::chip::{ToolChip, ToolChipEvent, ToolStatus};
    pub use crate::code_block::CodeBlock;
    pub use crate::code_diff::{
        CodeDiff, CodeDiffEvent, DiffFile, DiffHunk, DiffLine, DiffLineKind, DiffStats, HunkReview,
    };
    pub use crate::command_search::{
        CommandSearch, CommandSearchEvent, CommandSearchItem, CommandSearchLabels,
    };
    pub use crate::comparison_table::{
        ComparisonFeature, ComparisonItem, ComparisonItemState, ComparisonSnapshot,
        ComparisonSnapshotError, ComparisonTable, ComparisonTableEvent, ComparisonValue,
        MAX_COMPARISON_FEATURES, MAX_COMPARISON_ITEMS,
    };
    pub use crate::context_card::{ContextCard, ContextCardEvent};
    pub use crate::context_meter::{ContextMeter, ContextMeterVariant, ContextUsage, UsageLevel};
    pub use crate::cues::{Cue, CueSubscription};
    pub use crate::decoration::{self as decoration, Decoration};
    pub use crate::diff_table::{
        DiffCell, DiffChangeKind, DiffColumn, DiffColumnAlignment, DiffProposalAction,
        DiffProposalState, DiffRow, DiffSortDirection, DiffTable, DiffTableEvent,
    };
    pub use crate::filter_table::{
        FilterCell, FilterColumn, FilterColumnAlignment, FilterDefinition, FilterRow,
        FilterSortDirection, FilterTable, FilterTableEvent,
    };
    pub use crate::fine_tune::{FineTuneCard, FineTuneEvent, FineTuneTypeface, FineTuneValues};
    pub use crate::form::{
        ChoiceEvent, ChoiceGroup, ChoiceOption, Toggle, ToggleEvent, ToggleShape,
    };
    pub use crate::image_generation::ImageGeneration;
    pub use crate::insight::{
        InsightCard, InsightEvent, InsightMetric, InsightPoint, InsightTrend,
    };
    pub use crate::loading::LoadingState;
    pub use crate::motion::{
        MotionPreference, MotionTokens, Shimmer, breathing, reveal, reveal_staggered,
    };
    pub use crate::orbs::{OrbVariant, Orbs};
    pub use crate::plan::{
        PlanCard, PlanCardLabels, PlanEvent, PlanState, PlanStep, PlanStepStatus,
    };
    pub use crate::prompt_bar::{
        PromptActions, PromptAttachment, PromptBar, PromptBarEvent, PromptBarLabels,
        PromptCommand, PromptMention, PromptModel, PromptSubmission, PromptSubmit,
    };
    pub use crate::question_flow::{Question, QuestionFlow, QuestionFlowEvent};
    pub use crate::queue::{MessageQueue, MessageQueueLabels, QueueEvent, QueuedMessage};
    pub use crate::recommendation::{RecommendationCard, RecommendationEvent};
    pub use crate::records_table::{
        RecordCell, RecordCellKind, RecordColumn, RecordColumnAlignment, RecordRow,
        RecordSortDirection, RecordStatusTone, RecordsTable, RecordsTableEvent, RowActionPlacement,
        RowActionVisibility,
    };
    // The types, not the tuning. A wheel accelerator's five coefficients are
    // reachable at `gpui_ai::scrolling::*` for anyone reproducing its feel;
    // a prelude is for the names an application writes every day, and
    // `WHEEL_ACCEL_DECAY_SECONDS` is not one of them.
    pub use crate::scrolling::{
        AUTOSCROLL_FULL_SPEED_DISTANCE_PX, Autoscroll, MAX_AUTOSCROLL_SPEED_PX_PER_SEC, ScrollRoom,
        WheelAccelerator,
    };
    pub use crate::search_results::{SearchResult, SearchResults, SearchResultsEvent};
    pub use crate::selection_actions::{SelectionAction, SelectionActions, SelectionActionsEvent};
    pub use crate::sidebar_nav::{
        SidebarNav, SidebarNavEvent, SidebarNavItem, SidebarNavLabels, SidebarNavPresentation,
        SidebarSection,
    };
    pub use crate::sizing::SizeTokens;
    pub use crate::status::{StatusBadge, StatusTone};
    pub use crate::stream::{ProgressState, Progressive, StreamedContent};
    pub use crate::streaming_text::{
        CitationRef, FollowUp, SourceRef, StreamingText, StreamingTextEvent,
    };
    pub use crate::suggestions::{Suggestion, Suggestions, SuggestionsEvent};
    pub use crate::task::{TaskRow, TaskSnapshot};
    pub use crate::thinking::{
        StepStatus, Thinking, ThinkingEvent, ThinkingLabels, ThinkingStep, ThinkingTrace,
    };
    pub use crate::thread_list::{ThreadItem, ThreadList, ThreadListEvent, ThreadSection};
    pub use crate::todo_list::{TodoItem, TodoList, TodoListEvent, TodoStatus};
    pub use crate::tool_call::{
        ToolApproval, ToolCall, ToolCallEvent, ToolCallLabels, ToolGroup, ToolGroupEvent,
        ToolInvocation,
    };
    pub use crate::voice::{VoiceControls, VoiceEvent, VoiceState};
}

pub(crate) mod handlers {
    use gpui::{App, Window};
    use std::rc::Rc;

    /// Boxed event handler stored by builder components.
    pub(crate) type Handler<E> = Box<dyn Fn(&E, &mut Window, &mut App)>;
    /// Ref-counted event handler for components that clone handlers per child.
    pub(crate) type SharedHandler<E> = Rc<dyn Fn(&E, &mut Window, &mut App)>;
}

use gpui::App;

/// Initializes gpui-ai.
///
/// Call once at application startup, before creating any windows that use
/// these components. This also initializes the underlying [`gpui_component`]
/// state (theme, global settings), so applications do not need to call
/// `gpui_component::init` separately.
pub fn init(cx: &mut App) {
    gpui_component::init(cx);
    // The motion policy, before any component renders — and only unless the
    // application already chose one, so either order of init and
    // customization lands on the application's values.
    motion::install(cx);
    sizing::install(cx);
    scrolling::install(cx);
    popup::install(cx);
    records_table::init(cx);
}
