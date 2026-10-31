//! The flattened visible-row snapshot and the keyboard model that walks it.
//!
//! One projection turns the application's sections, the query, the owned
//! expansion set, and the controlled active item into `[VisibleRow]`, and the
//! ARIA tree keyboard model resolves every key against those same rows. The
//! two belong together: what a row states about its place in the tree is
//! exactly what stepping, bounding, and walking to a parent read back.

use std::{collections::HashSet, sync::Arc};

use gpui::{Context, ListOffset, Pixels, SharedString, Window};

use super::{SidebarNav, SidebarNavEvent, SidebarNavItem, SidebarSection};

fn collect_item_ids(items: &[SidebarNavItem], ids: &mut HashSet<SharedString>) -> bool {
    items
        .iter()
        .all(|item| ids.insert(item.id.clone()) && collect_item_ids(&item.children, ids))
}

pub(super) fn snapshot_ids_are_unique(sections: &[SidebarSection]) -> bool {
    let mut section_ids = HashSet::new();
    let mut item_ids = HashSet::new();
    sections.iter().all(|section| {
        section_ids.insert(section.id.clone()) && collect_item_ids(&section.items, &mut item_ids)
    })
}

pub(super) fn collect_parent_ids(items: &[SidebarNavItem], ids: &mut HashSet<SharedString>) {
    for item in items {
        if !item.children.is_empty() {
            ids.insert(item.id.clone());
            collect_parent_ids(&item.children, ids);
        }
    }
}

fn item_matches(item: &SidebarNavItem, query: &str) -> bool {
    item.label.to_lowercase().contains(query)
        || item
            .badge
            .as_ref()
            .is_some_and(|badge| badge.to_lowercase().contains(query))
}

/// The sections and items one query retains.
///
/// Retention is recorded as identity rather than as a rebuilt tree: the
/// snapshot walk reads the application's own sections and skips what the
/// projection excludes, so filtering never clones a branch.
#[derive(Debug, Default)]
struct FilterProjection {
    sections: HashSet<SharedString>,
    items: HashSet<SharedString>,
}

/// Records `item` and its retained descendants, reporting whether it survives.
///
/// A matching item keeps its whole subtree — someone who searched for a branch
/// expects to see what is inside it — which `inherited` carries downward. Every
/// child is visited regardless of the running result, because retention is
/// being recorded, not merely answered.
fn retain_item(
    item: &SidebarNavItem,
    query: &str,
    inherited: bool,
    retained: &mut HashSet<SharedString>,
) -> bool {
    let matched = inherited || item_matches(item, query);
    let mut keep = matched;
    for child in item.children.iter() {
        keep |= retain_item(child, query, matched, retained);
    }
    if keep {
        retained.insert(item.id.clone());
    }
    keep
}

fn filter_projection(sections: &[SidebarSection], query: &str) -> FilterProjection {
    let mut projection = FilterProjection::default();
    for section in sections {
        let matched = section.label.to_lowercase().contains(query);
        let mut keep = matched;
        for item in section.items.iter() {
            keep |= retain_item(item, query, matched, &mut projection.items);
        }
        if keep {
            projection.sections.insert(section.id.clone());
        }
    }
    projection
}

/// Records the ancestors of `target`, reporting whether the walk found it.
fn collect_ancestors(
    items: &[SidebarNavItem],
    target: &SharedString,
    ancestors: &mut HashSet<SharedString>,
) -> bool {
    for item in items {
        if item.id == *target {
            return true;
        }
        if collect_ancestors(&item.children, target, ancestors) {
            ancestors.insert(item.id.clone());
            return true;
        }
    }
    false
}

fn find_item<'a>(item: &'a SidebarNavItem, id: &SharedString) -> Option<&'a SidebarNavItem> {
    (item.id == *id)
        .then_some(item)
        .or_else(|| item.children.iter().find_map(|child| find_item(child, id)))
}

/// One row of the flattened visible-row snapshot.
///
/// A row states its own place in the tree because the virtual list renders
/// rows as siblings: `level`, `parent`, `position`, and `set_size` are the
/// relationships that element nesting would otherwise carry, and both the
/// keyboard model and the AccessKit nodes read them from here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct VisibleRow {
    /// Stable identity: the section ID for a header, the item ID otherwise.
    pub(super) id: SharedString,
    /// Visible label.
    pub(super) label: SharedString,
    /// Leading icon path, when the item declares one.
    pub(super) icon: Option<SharedString>,
    /// Compact trailing badge text, when the item declares one.
    pub(super) badge: Option<SharedString>,
    /// The row this one hangs from: a root item's parent is its section.
    pub(super) parent: Option<SharedString>,
    /// One-based accessibility level; a section header is level 1.
    pub(super) level: usize,
    /// Indentation steps below the section's root items.
    pub(super) indent: usize,
    /// One-based position among the visible siblings under `parent`.
    pub(super) position: usize,
    /// Number of visible siblings under `parent`.
    pub(super) set_size: usize,
    /// Whether the row labels a section instead of an item.
    pub(super) header: bool,
    /// Whether every activation path is unavailable.
    pub(super) disabled: bool,
    /// Whether the row owns children that the projection retained.
    pub(super) has_children: bool,
    /// Whether this row's children follow it in the snapshot.
    pub(super) expanded: bool,
    /// Whether the row carries the active treatment.
    pub(super) active: bool,
    /// Whether the controlled active item is a descendant.
    pub(super) contains_active: bool,
}

/// The resolved inputs a snapshot walk reads for every row.
struct RowContext<'a> {
    projection: Option<&'a FilterProjection>,
    expanded: &'a HashSet<SharedString>,
    active: Option<&'a SharedString>,
    ancestors: &'a HashSet<SharedString>,
    filtering: bool,
    collapsed: bool,
}

/// The children of one item that survive the query, in application order.
fn visible_children<'a>(
    items: &'a [SidebarNavItem],
    projection: Option<&'a FilterProjection>,
) -> impl Iterator<Item = &'a SidebarNavItem> {
    items
        .iter()
        .filter(move |item| projection.is_none_or(|projection| projection.items.contains(&item.id)))
}

/// Flattens the controlled snapshot into the rows a frame may render.
///
/// This is the component's one projection: it applies the query, the owned
/// expansion set, the controlled active item, and the collapsed rail in a
/// single pass, so no later stage has to re-derive any of them.
pub(super) fn visible_rows(
    sections: &[SidebarSection],
    query: &str,
    expanded: &HashSet<SharedString>,
    active: Option<&SharedString>,
    collapsed: bool,
) -> Arc<[VisibleRow]> {
    let query = query.trim().to_lowercase();
    let filtering = !query.is_empty();
    let projection = filtering.then(|| filter_projection(sections, &query));
    let projection = projection.as_ref();

    // An active item the query excluded cannot be contained by anything on
    // screen, so its ancestors keep their own presentation.
    let active =
        active.filter(|id| projection.is_none_or(|projection| projection.items.contains(*id)));
    let mut ancestors = HashSet::new();
    if let Some(active) = active {
        for section in sections {
            if collect_ancestors(&section.items, active, &mut ancestors) {
                break;
            }
        }
    }

    let context = RowContext {
        projection,
        expanded,
        active,
        ancestors: &ancestors,
        filtering,
        collapsed,
    };

    let visible: Vec<&SidebarSection> = sections
        .iter()
        .filter(|section| {
            projection.is_none_or(|projection| projection.sections.contains(&section.id))
        })
        .collect();
    let section_count = visible.len();
    // The compact rail shows one column of root items, so it has no section
    // headers and its roots are the tree's top level.
    let root_level = if collapsed { 1 } else { 2 };

    let mut rows = Vec::new();
    for (index, section) in visible.into_iter().enumerate() {
        if !collapsed {
            let children = visible_children(&section.items, context.projection)
                .next()
                .is_some();
            rows.push(VisibleRow {
                id: section.id.clone(),
                label: section.label.clone(),
                icon: None,
                badge: None,
                parent: None,
                level: 1,
                indent: 0,
                position: index + 1,
                set_size: section_count,
                header: true,
                disabled: false,
                has_children: children,
                expanded: children,
                active: false,
                contains_active: false,
            });
        }
        push_rows(
            &section.items,
            &section.id,
            root_level,
            0,
            &context,
            &mut rows,
        );
    }
    Arc::from(rows)
}

/// Appends one visible sibling set, and the subtrees the reader can see.
fn push_rows(
    items: &[SidebarNavItem],
    parent: &SharedString,
    level: usize,
    indent: usize,
    context: &RowContext,
    rows: &mut Vec<VisibleRow>,
) {
    let set_size = visible_children(items, context.projection).count();
    for (index, item) in visible_children(items, context.projection).enumerate() {
        let contains_active = context.ancestors.contains(&item.id);
        let has_children = visible_children(&item.children, context.projection)
            .next()
            .is_some();
        // A query exposes the ancestry it matched inside without recording
        // that reveal as expansion the reader chose, and a controlled active
        // descendant stays reachable through a parent the reader collapsed.
        let expanded = has_children
            && (context.filtering || context.expanded.contains(&item.id) || contains_active);
        // A compact sidebar cannot expose descendants, so its visible ancestor
        // carries selected state instead.
        let active = context.active == Some(&item.id) || (context.collapsed && contains_active);
        rows.push(VisibleRow {
            id: item.id.clone(),
            label: item.label.clone(),
            icon: item.icon.clone(),
            badge: item.badge.clone(),
            parent: Some(parent.clone()),
            level,
            indent,
            position: index + 1,
            set_size,
            header: false,
            disabled: item.disabled,
            has_children,
            expanded,
            active,
            contains_active,
        });
        if expanded && !context.collapsed {
            push_rows(
                &item.children,
                &item.id,
                level + 1,
                indent + 1,
                context,
                rows,
            );
        }
    }
}

impl SidebarNav {
    /// Recomputes the flattened snapshot and notifies once.
    ///
    /// Every path that changes sections, query, expansion, active item, or the
    /// collapsed rail ends here, and `Render` ends nowhere near it.
    pub(super) fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let anchor = self.scroll_anchor();
        self.rows = visible_rows(
            &self.sections,
            &self.query,
            &self.expanded,
            self.active_item.as_ref(),
            self.collapsed,
        );
        self.row_list.reset(self.rows.len());
        self.restore_anchor(anchor);
        cx.notify();
    }

    /// The stable ID of the row currently at the top of the viewport.
    fn scroll_anchor(&self) -> Option<(SharedString, Pixels)> {
        let offset = self.row_list.logical_scroll_top();
        self.rows
            .get(offset.item_ix)
            .map(|row| (row.id.clone(), offset.offset_in_item))
    }

    /// Puts the anchored row back where it was, when it is still visible.
    fn restore_anchor(&self, anchor: Option<(SharedString, Pixels)>) {
        let Some((id, offset_in_item)) = anchor else {
            return;
        };
        let Some(item_ix) = self.rows.iter().position(|row| row.id == id) else {
            return;
        };
        self.row_list.scroll_to(ListOffset {
            item_ix,
            offset_in_item,
        });
    }

    /// Re-measures the row list after the window's rem size changed.
    ///
    /// Row heights cache text laid out at the previous rem, and neither a
    /// snapshot nor a collapse reports a zoom change. The row that was first
    /// on screen stays first.
    pub(super) fn resolve_layout(&mut self, rem_size: Pixels, cx: &mut Context<Self>) {
        if !self.resolved_layout.observe(rem_size) {
            return;
        }
        let anchor = self.scroll_anchor();
        self.row_list.remeasure();
        self.restore_anchor(anchor);
        cx.notify();
    }

    /// Index of the retained roving row, when its ID is still visible.
    fn focused_row_index(&self) -> Option<usize> {
        let focused = self.focused_row.as_ref()?;
        self.rows.iter().position(|row| row.id == *focused)
    }

    /// The row the tree treats as current.
    ///
    /// A tree has exactly one entry point. Falling back to the first visible
    /// row rather than to row zero keeps that entry point on screen after the
    /// reader scrolls somewhere else.
    fn roving_row_index(&self) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }
        self.focused_row_index().or_else(|| {
            let first = self.row_list.logical_scroll_top().item_ix;
            (first < self.rows.len()).then_some(first)
        })
    }

    pub(super) fn roving_row_id(&self) -> Option<SharedString> {
        self.roving_row_index()
            .and_then(|index| self.rows.get(index))
            .map(|row| row.id.clone())
    }

    /// Moves the roving row, revealing it and keeping focus on the tree.
    fn focus_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(row) = self.rows.get(index) else {
            return false;
        };
        self.focused_row = Some(row.id.clone());
        self.row_list.scroll_to_reveal_item(index);
        self.tree_focus.focus(window, cx);
        cx.notify();
        true
    }

    /// The next row in `direction` that a reader can land on.
    fn step(&self, from: usize, forward: bool) -> Option<usize> {
        let mut index = from;
        loop {
            index = if forward {
                index.checked_add(1)?
            } else {
                index.checked_sub(1)?
            };
            if !self.rows.get(index)?.disabled {
                return Some(index);
            }
        }
    }

    /// The first or last row a reader can land on.
    fn bound(&self, last: bool) -> Option<usize> {
        let mut candidates = (0..self.rows.len())
            .filter(|index| self.rows.get(*index).is_some_and(|row| !row.disabled));
        if last {
            candidates.next_back()
        } else {
            candidates.next()
        }
    }

    /// The first landable child of the row at `index`.
    fn first_child(&self, index: usize) -> Option<usize> {
        let row = self.rows.get(index)?;
        self.rows
            .iter()
            .enumerate()
            .skip(index + 1)
            .take_while(|(_, candidate)| candidate.level > row.level)
            .find(|(_, candidate)| {
                candidate.parent.as_ref() == Some(&row.id) && !candidate.disabled
            })
            .map(|(child, _)| child)
    }

    /// The row that owns the row at `index`, when a reader can land on it.
    fn parent_row(&self, index: usize) -> Option<usize> {
        let parent = self.rows.get(index)?.parent.as_ref()?;
        self.rows
            .iter()
            .position(|row| row.id == *parent && !row.disabled)
    }

    /// Records expansion the reader chose and reprojects the rows.
    fn set_expanded(&mut self, id: SharedString, expanded: bool, cx: &mut Context<Self>) {
        let changed = if expanded {
            self.expanded.insert(id)
        } else {
            self.expanded.remove(&id)
        };
        if changed {
            self.rebuild_rows(cx);
        }
    }

    /// The ARIA tree keyboard model, resolved against the flattened rows.
    ///
    /// Returns whether the tree consumed the key, so an unhandled one still
    /// reaches the application.
    pub(super) fn navigate(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(current) = self.roving_row_index() else {
            return false;
        };
        let Some(row) = self.rows.get(current).cloned() else {
            return false;
        };
        match key {
            "up" => self
                .step(current, false)
                .is_some_and(|index| self.focus_row(index, window, cx)),
            "down" => self
                .step(current, true)
                .is_some_and(|index| self.focus_row(index, window, cx)),
            "home" => self
                .bound(false)
                .is_some_and(|index| self.focus_row(index, window, cx)),
            "end" => self
                .bound(true)
                .is_some_and(|index| self.focus_row(index, window, cx)),
            "right" => {
                if row.has_children && !row.expanded && !row.header {
                    self.set_expanded(row.id.clone(), true, cx);
                    true
                } else {
                    self.first_child(current)
                        .is_some_and(|index| self.focus_row(index, window, cx))
                }
            }
            "left" => {
                // Only expansion the reader chose collapses again: a branch
                // held open by the query or by the controlled active item
                // would not close, so Left walks to the parent instead.
                if !row.header && row.expanded && self.expanded.contains(&row.id) {
                    self.set_expanded(row.id.clone(), false, cx);
                    true
                } else {
                    self.parent_row(current)
                        .is_some_and(|index| self.focus_row(index, window, cx))
                }
            }
            "enter" | "space" => {
                if row.header {
                    return false;
                }
                self.activate_item(row.id.clone(), window, cx);
                true
            }
            _ => false,
        }
    }

    pub(super) fn activate_item(
        &mut self,
        item_id: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self
            .sections
            .iter()
            .flat_map(|section| section.items.iter())
            .find_map(|item| find_item(item, &item_id))
        else {
            return;
        };
        if item.disabled {
            return;
        }
        let has_children = !item.children.is_empty();
        // Pointer and keyboard activation share one contract, so a click also
        // moves the tree's roving row onto what it activated.
        self.focused_row = Some(item_id.clone());
        self.tree_focus.focus(window, cx);
        if has_children && !self.expanded.remove(&item_id) {
            self.expanded.insert(item_id.clone());
        }
        self.rebuild_rows(cx);
        cx.emit(SidebarNavEvent::Selected {
            id: self.id.clone(),
            item_id,
        });
    }
}
