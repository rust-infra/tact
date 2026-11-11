//! Trajectory page — app-layer wrapper.
//!
//! The page takes the whole main area rather than a centred popup: three rows
//! of overview plus a ledger need the height, and a popup's `Clear` would hide
//! the log the reader may want to cross-reference. Same hosting form as the
//! subagent view.
//!
//! Everything here is wiring: record the hit area the page uses to learn its
//! own height, then hand the pure kit renderer the theme and the state.

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::{App, SurfaceId};

pub(crate) fn render_trajectory_view(frame: &mut Frame, area: Rect, app: &mut App) {
    app.mouse.set_area(SurfaceId::TrajectoryPage, area);
    app.mouse.trajectory_page_area = area;

    let log = app.trajectory_log();
    let state = app.trajectory_page_state();
    let theme = app.theme;
    agent_tui_kit::render::trajectory::render_trajectory_page(
        area,
        frame.buffer_mut(),
        log,
        state,
        &theme,
    );
}
