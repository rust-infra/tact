use std::{cell::Cell, rc::Rc};

/// Row-aligned scroll state for compact, fixed-height lists.
///
/// Pixel offsets force the whole list to be rebuilt on every wheel event. The
/// pickers only need a top row: clamping and windowing happen in O(1), and the
/// renderer builds at most one screenful of rows.
#[derive(Clone, Default)]
pub struct RowScroll {
    row: Rc<Cell<usize>>,
    residual: Rc<Cell<f32>>,
}

impl RowScroll {
    pub fn reset(&self) {
        self.row.set(0);
        self.residual.set(0.);
    }

    pub fn start(&self, item_count: usize, visible_rows: usize) -> usize {
        let row = self.row.get().min(max_start(item_count, visible_rows));
        if row != self.row.get() {
            self.row.set(row);
        }
        row
    }

    pub fn scroll_to(&self, index: usize, item_count: usize, visible_rows: usize) -> bool {
        let visible_rows = visible_rows.max(1);
        let current = self.start(item_count, visible_rows);
        let target = if index < current {
            index
        } else if index >= current + visible_rows {
            index + 1 - visible_rows
        } else {
            current
        }
        .min(max_start(item_count, visible_rows));
        self.residual.set(0.);
        if target == current {
            return false;
        }
        self.row.set(target);
        true
    }

    /// Accumulates trackpad pixels until a whole row is crossed, then advances
    /// by that many rows. A mouse-wheel notch lands on a row boundary too.
    pub fn wheel(
        &self,
        delta: f32,
        item_count: usize,
        visible_rows: usize,
        row_height: f32,
    ) -> bool {
        if row_height <= 0. || visible_rows == 0 {
            self.reset();
            return false;
        }
        let max = max_start(item_count, visible_rows);
        let mut row = self.start(item_count, visible_rows) as isize;
        let max = max as isize;
        let mut residual = self.residual.get() + delta;
        while residual <= -row_height && row < max {
            row += 1;
            residual += row_height;
        }
        while residual >= row_height && row > 0 {
            row -= 1;
            residual -= row_height;
        }
        if (row == 0 && residual > 0.) || (row == max && residual < 0.) {
            residual = 0.;
        }
        let next = row.max(0) as usize;
        self.residual.set(residual);
        let changed = next != self.row.get();
        self.row.set(next);
        changed
    }
}

fn max_start(item_count: usize, visible_rows: usize) -> usize {
    item_count.saturating_sub(visible_rows.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_moves_one_row_at_a_time_and_clamps() {
        let scroll = RowScroll::default();
        assert!(!scroll.wheel(-10., 20, 8, 30.));
        assert!(scroll.wheel(-20., 20, 8, 30.));
        assert_eq!(scroll.start(20, 8), 1);
        assert!(scroll.wheel(-300., 20, 8, 30.));
        assert_eq!(scroll.start(20, 8), 11);
        assert!(scroll.wheel(-300., 20, 8, 30.));
        assert_eq!(scroll.start(20, 8), 12);
        assert!(!scroll.wheel(-300., 20, 8, 30.));
        assert_eq!(scroll.start(20, 8), 12);
    }

    #[test]
    fn reveal_keeps_the_cursor_inside_the_window() {
        let scroll = RowScroll::default();
        assert!(scroll.scroll_to(10, 20, 8));
        assert_eq!(scroll.start(20, 8), 3);
        assert!(scroll.scroll_to(0, 20, 8));
        assert_eq!(scroll.start(20, 8), 0);
    }
}
