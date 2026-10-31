#[allow(unused_imports)]
pub use super::{PackagingTarget, TARGET};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_target_is_explicit() {
        if cfg!(target_os = "windows") {
            assert_eq!(TARGET, Some(PackagingTarget::Windows));
        }
    }
}
