#[allow(unused_imports)]
pub use super::{PackagingTarget, TARGET};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_target_is_explicit() {
        if cfg!(target_os = "linux") {
            assert_eq!(TARGET, Some(PackagingTarget::Linux));
        }
    }
}
