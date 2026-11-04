use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn config() -> EngineConfig {
    let mut config = EngineConfig::new(Root::of::<tact_desktop::App>());
    config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now()).with_frames();
    config
}

#[test]
fn the_shell_renders_its_title_bar_and_run_room_tabs() {
    let _serial = serial();
    let mut app = Headless::start(config()).expect("the Tact shell starts");
    app.wait_for("text:Tact").expect("the title bar shows");
    app.wait_for("text:Chat").expect("the run room tabs show");
    let texts = app.screen().texts();
    assert!(texts.contains(&"Chat"));
    assert!(texts.contains(&"Trajectory"));
}

#[test]
fn the_command_center_opens_from_the_title_bar() {
    let _serial = serial();
    let mut app = Headless::start(config()).expect("the Tact shell starts");
    app.wait_for("label:Commands")
        .expect("the Commands button shows");
    app.press("label:Commands")
        .expect("the Commands button presses");
    app.wait_for("text:Command center")
        .expect("the command center opens");
    app.press("label:Close").expect("the close button shows");
    app.run_for(std::time::Duration::from_millis(100))
        .expect("frames run");
    assert!(!app.screen().texts().contains(&"Command center"));
}

#[test]
fn the_trajectory_page_shows_the_ledger() {
    let _serial = serial();
    let mut app = Headless::start(config()).expect("the Tact shell starts");
    app.wait_for("label:Trajectory").expect("the tab shows");
    app.press("label:Trajectory").expect("the tab presses");
    app.wait_for("text:Select a record")
        .expect("the trajectory detail shows");
}

#[test]
fn a_command_opens_the_reader() {
    let _serial = serial();
    let mut app = Headless::start(config()).expect("the Tact shell starts");
    app.wait_for("label:Commands")
        .expect("the Commands button shows");
    app.press("label:Commands")
        .expect("the command center opens");
    app.wait_for("label:help").expect("the help command shows");
    app.press("label:help").expect("the command runs");
    app.wait_for("id:reader").expect("the reader opens");
}

#[test]
fn slash_opens_the_command_suggestions() {
    let _serial = serial();
    let mut app = Headless::start(config()).expect("the Tact shell starts");
    app.wait_for("id:composer-input")
        .expect("the composer shows");
    app.type_text("id:composer-input", "/")
        .expect("typing the slash works");
    app.wait_for("label:theme")
        .expect("the slash suggestions show");
}

#[test]
fn the_settings_overlay_lists_the_settings() {
    let _serial = serial();
    let mut app = Headless::start(config()).expect("the Tact shell starts");
    app.wait_for("label:Settings")
        .expect("the Settings button shows");
    app.press("label:Settings").expect("the settings open");
    app.wait_for("text:Provider")
        .expect("the provider setting shows");
    app.wait_for("text:Reasoning effort")
        .expect("the effort setting shows");
}

#[test]
fn the_app_menu_opens_and_escape_closes_it() {
    let _serial = serial();
    let mut app = Headless::start(config()).expect("the Tact shell starts");
    app.wait_for("label:File").expect("the File menu shows");
    app.press("label:File").expect("the File menu opens");
    app.wait_for("label:Command Center")
        .expect("the File menu items show");
    app.press_key("escape").expect("escape presses");
    app.run_for(std::time::Duration::from_millis(100))
        .expect("frames run");
    assert!(!app.screen().texts().contains(&"Command Center"));
}
