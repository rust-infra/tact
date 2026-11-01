use tact_desktop_core::{BackgroundTaskView, DesktopIntent};

pub fn stop_intent(task: &BackgroundTaskView) -> DesktopIntent {
    DesktopIntent::CancelBackgroundTask {
        task_id: task.id.to_string(),
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
