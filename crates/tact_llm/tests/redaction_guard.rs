//! Source-level guard for fields that have leaked provider payloads before.

#[test]
fn production_sources_do_not_retain_body_bearing_http_or_trace_fields() {
    let rules = [
        ("../src/error.rs", "body: String"),
        ("../src/error.rs", "body: resp_body"),
        ("../src/account.rs", ".text().await"),
        ("../src/openai/compatible/mod.rs", "args = %"),
        ("../src/openai/compatible/mod.rs", "response.text()"),
        ("../src/openai/responses/mod.rs", "response.text()"),
    ];

    for (file, forbidden) in rules {
        let source = match file {
            "../src/error.rs" => include_str!("../src/error.rs"),
            "../src/account.rs" => include_str!("../src/account.rs"),
            "../src/openai/compatible/mod.rs" => include_str!("../src/openai/compatible/mod.rs"),
            "../src/openai/responses/mod.rs" => include_str!("../src/openai/responses/mod.rs"),
            _ => unreachable!(),
        };
        for (index, line) in source.lines().enumerate() {
            assert!(
                !line.contains(forbidden),
                "body-bearing field {forbidden:?} found at {file}:{}",
                index + 1
            );
        }
    }
}
