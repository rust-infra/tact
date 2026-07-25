fn main() {
    let src = r#"flowchart TD
      T23["[x] #23 init"] --> T24["[>] #24 user"]
      T23 --> T25["[ ] #25 base"]
      T24 --> T27["[ ] #27 api"]
      T25 --> T27
    "#;
    // Try common theme APIs
    let themes: &[fn() -> Result<String, String>] = &[];
    let _ = themes;

    // Inspect via compile errors if needed
    match meraid::render(src, meraid::theme::ThemeType::Mono) {
        Ok(s) => {
            println!("len={} lines={}", s.len(), s.lines().count());
            println!("{s}");
            let has_esc = s.contains('\u{1b}');
            println!("has_ansi_esc={has_esc}");
        }
        Err(e) => eprintln!("err: {e}"),
    }
}
