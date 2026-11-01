//! Transcript identity, structural diffing, and retained-layout invalidation.

use super::*;

pub(super) fn message_ids_are_unique(messages: &[ChatMessage]) -> bool {
    let mut ids = HashSet::with_capacity(messages.len());
    messages.iter().all(|message| ids.insert(&message.id))
}

pub(super) fn structural_splice(old: &[ChatMessage], new: &[ChatMessage]) -> (Range<usize>, usize) {
    let prefix = old
        .iter()
        .zip(new)
        .take_while(|(old, new)| old.id == new.id)
        .count();
    let max_suffix = old.len().min(new.len()).saturating_sub(prefix);
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(old, new)| old.id == new.id)
        .count();
    (
        prefix..old.len().saturating_sub(suffix),
        new.len().saturating_sub(prefix + suffix),
    )
}

impl Chat {
    pub(super) fn resolve_layout(&mut self, rem_size: Pixels, cx: &mut Context<Self>) {
        if !self.resolved_layout.observe(rem_size) {
            return;
        }
        let was_following = self.is_pinned_to_bottom();
        let offset = self.list_state.logical_scroll_top();
        let anchor = self
            .messages
            .get(offset.item_ix)
            .map(|message| (message.id.clone(), offset.offset_in_item));

        self.list_state.remeasure();
        if was_following {
            self.list_state.set_follow_mode(FollowMode::Tail);
            self.list_state.scroll_to_end();
            self.pinned_to_bottom = true;
        } else if let Some((anchor_id, offset_in_item)) = anchor
            && let Some(item_ix) = self
                .messages
                .iter()
                .position(|message| message.id == anchor_id)
        {
            self.list_state.scroll_to(ListOffset {
                item_ix,
                offset_in_item,
            });
        }
        cx.notify();
    }
}
