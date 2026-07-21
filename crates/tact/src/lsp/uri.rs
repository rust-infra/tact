pub(crate) fn path_to_uri(path: &str) -> String {
    // Simple heuristic; for full correctness callers should pass pre-formed URIs
    if path.starts_with("file://") {
        return path.to_string();
    }
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
    let s = canonical.to_string_lossy();
    if cfg!(target_os = "windows") {
        // Drive letters need a leading slash: file:///C:/...
        format!("file:///{}", s.replace('\\', "/"))
    } else {
        format!("file://{}", s)
    }
}

pub(crate) fn uri_to_path(uri: &str) -> String {
    let stripped = uri
        .strip_prefix("file:///")
        .or_else(|| uri.strip_prefix("file://"))
        .unwrap_or(uri);
    if cfg!(target_os = "windows") {
        stripped.replace('/', "\\")
    } else {
        stripped.to_string()
    }
}
