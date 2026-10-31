use tact_desktop_core::SurfaceSnapshot;

pub fn prompt_markdown(surface: Option<&SurfaceSnapshot>) -> Option<&str> {
    match surface {
        Some(SurfaceSnapshot::SystemPrompt(prompt)) => Some(prompt),
        _ => None,
    }
}
