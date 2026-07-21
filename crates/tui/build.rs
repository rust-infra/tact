fn main() {
    // Read workspace version from the root Cargo.toml
    let manifest = std::fs::read_to_string(
        std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("Cargo.toml"),
    )
    .expect("failed to read workspace Cargo.toml");

    let toml: toml::Value =
        toml::from_str(&manifest).expect("failed to parse workspace Cargo.toml");
    if let Some(ver) = toml
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
    {
        println!("cargo:rustc-env=TACT_VERSION={}", ver);
    }
}
