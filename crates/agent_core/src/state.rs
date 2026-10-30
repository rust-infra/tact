use tact_agent_api::{CursorUpdate, ModelCursor};

pub(crate) fn apply_cursor(cursor: &mut Option<ModelCursor>, update: CursorUpdate) {
    if let CursorUpdate::Replace {
        cursor: replacement,
    } = update
    {
        *cursor = replacement;
    }
}
