pub trait View: Send + Sync {
    fn id(&self) -> &'static str;
}
