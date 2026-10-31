#[allow(unused_imports)]
pub use super::{PackagingTarget, TARGET};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_packaging_target_is_explicit() {
        if cfg!(target_os = "macos") {
            assert_eq!(TARGET, Some(PackagingTarget::MacOs));
        }
        assert!(super::super::supported_platform());
    }
}
