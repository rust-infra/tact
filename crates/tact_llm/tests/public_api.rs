#![cfg(all(not(feature = "legacy-tact"), feature = "openai-responses"))]

#[test]
fn stable_build_rejects_compatibility_and_transport_imports() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/stable-*.rs");
}
