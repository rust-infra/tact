use tact_desktop_core::{QueuedPromptView, Submission};

pub fn submission(text: impl Into<String>) -> Option<Submission> {
    let text = text.into();
    let text = text.trim();
    (!text.is_empty()).then(|| Submission {
        text: text.to_string(),
        attachments: Vec::new(),
        model: None,
        permission_mode: None,
    })
}

pub fn queue_summary(queue: &[QueuedPromptView]) -> String {
    match queue.len() {
        0 => "No queued prompts".into(),
        1 => "1 queued prompt".into(),
        count => format!("{count} queued prompts"),
    }
}
