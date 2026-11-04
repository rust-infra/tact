use tact_desktop_core::{BackgroundTaskView, DesktopIntent};

/// `"420 ms"` / `"1.5s"` / `"2m 05s"` for a span in milliseconds.
pub fn format_duration_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        format!("{}m {:02}s", ms / 60_000, (ms % 60_000) / 1_000)
    }
}

pub fn stop_intent(task: &BackgroundTaskView) -> DesktopIntent {
    DesktopIntent::CancelBackgroundTask {
        task_id: task.id.to_string(),
    }
}

/// The elapsed label for a task, or `None` when it cannot be measured.
///
/// The protocol carries `started_at` as a bare `i64`, so `0` is the absent
/// value. Subtracting it from now would report an elapsed time measured from
/// 1970 — decades of runtime the task never had — so an unrecorded start
/// produces no label instead of a fake one.
pub fn elapsed_label(task: &BackgroundTaskView, now_ms: i64) -> Option<String> {
    if task.started_at <= 0 {
        return None;
    }
    let end = task.finished_at.unwrap_or(now_ms);
    let millis = end.saturating_sub(task.started_at).max(0) as u64;
    Some(format_duration_ms(millis))
}

/// A row's one-line metadata: the status, plus the elapsed time when the
/// producer recorded a start.
pub fn meta_label(task: &BackgroundTaskView, now_ms: i64, status: &str) -> String {
    match elapsed_label(task, now_ms) {
        Some(elapsed) => format!("{status} · {elapsed}"),
        None => status.to_string(),
    }
}

pub fn status_label(task: &BackgroundTaskView) -> &'static str {
    match task.status {
        tact_desktop_core::BackgroundTaskStatusSnapshot::Running => "Running",
        tact_desktop_core::BackgroundTaskStatusSnapshot::Completed => "Completed",
        tact_desktop_core::BackgroundTaskStatusSnapshot::Error => "Error",
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use std::sync::Arc;

    use super::*;

    #[test]
    fn an_unrecorded_start_reports_no_elapsed_time() {
        let undated = BackgroundTaskView {
            id: Arc::from("task-3"),
            started_at: 0,
            ..BackgroundTaskView::default()
        };
        assert_eq!(elapsed_label(&undated, 1_700_000_000_000), None);
        assert_eq!(
            meta_label(&undated, 1_700_000_000_000, "Running"),
            "Running",
            "the status still shows; only the invented duration is dropped"
        );
    }

    #[test]
    fn a_recorded_start_reports_its_span() {
        let running = BackgroundTaskView {
            id: Arc::from("task-4"),
            started_at: 1_700_000_000_000,
            ..BackgroundTaskView::default()
        };
        assert_eq!(
            elapsed_label(&running, 1_700_000_095_000).as_deref(),
            Some("1m 35s")
        );
        // A finished task measures to its end, not to now.
        let done = BackgroundTaskView {
            finished_at: Some(1_700_000_002_000),
            ..running.clone()
        };
        assert_eq!(
            elapsed_label(&done, 1_700_000_099_000).as_deref(),
            Some("2.0s")
        );
    }

    #[gpui::test]
    fn background_stop_targets_one_task(_cx: &mut TestAppContext) {
        let task = BackgroundTaskView {
            id: Arc::from("task-2"),
            ..BackgroundTaskView::default()
        };
        assert_eq!(
            stop_intent(&task),
            DesktopIntent::CancelBackgroundTask {
                task_id: "task-2".into()
            }
        );
    }
}
